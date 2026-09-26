//! Durable, opaque recovery snapshots for the in-app updater.
//!
//! The caller owns the location of the snapshot and the meaning of its payload.
//! This module only wraps that payload with the schema version and creation
//! timestamp, and provides guarded atomic file operations.

use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum number of bytes in the compact JSON serialization of the payload.
pub const MAX_SERIALIZED_PAYLOAD_BYTES: usize = 20 * 1024 * 1024;

// The envelope has a fixed, small amount of metadata. Keeping a small bound on
// the complete file lets load reject unexpectedly large files before parsing.
const MAX_SERIALIZED_SNAPSHOT_BYTES: usize = MAX_SERIALIZED_PAYLOAD_BYTES + 4096;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_TEMP_FILE_ATTEMPTS: usize = 128;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The on-disk representation of a recovery snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverySnapshot {
    pub schema_version: u32,
    pub timestamp: u64,
    pub payload: Value,
}

/// Errors returned by recovery snapshot operations.
#[derive(Debug)]
pub enum RecoveryError {
    InvalidPath(PathBuf),
    PathContainsSymlink(PathBuf),
    TargetIsNotRegularFile(PathBuf),
    PayloadTooLarge {
        actual: usize,
        maximum: usize,
    },
    SnapshotTooLarge {
        actual: usize,
        maximum: usize,
    },
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    Serialize(serde_json::Error),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(path) => {
                write!(f, "invalid recovery snapshot path: {}", path.display())
            }
            Self::PathContainsSymlink(path) => {
                write!(
                    f,
                    "recovery snapshot path contains a symlink: {}",
                    path.display()
                )
            }
            Self::TargetIsNotRegularFile(path) => {
                write!(
                    f,
                    "recovery snapshot target is not a regular file: {}",
                    path.display()
                )
            }
            Self::PayloadTooLarge { actual, maximum } => {
                write!(
                    f,
                    "recovery snapshot payload is {actual} bytes; maximum is {maximum} bytes"
                )
            }
            Self::SnapshotTooLarge { actual, maximum } => {
                write!(
                    f,
                    "recovery snapshot is {actual} bytes; maximum is {maximum} bytes"
                )
            }
            Self::Corrupt { path, source } => {
                write!(
                    f,
                    "corrupt recovery snapshot at {}: {source}",
                    path.display()
                )
            }
            Self::Serialize(source) => write!(f, "could not serialize recovery snapshot: {source}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                f,
                "could not {operation} recovery snapshot at {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for RecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Corrupt { source, .. } | Self::Serialize(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Save a recovery snapshot atomically at `path`.
///
/// `payload` is serialized compactly for the size check, then stored inside a
/// versioned envelope. The timestamp is Unix seconds supplied by the caller.
/// The parent directory is created as private, and the destination is replaced
/// only after the complete temporary file has been written and synced.
pub fn save(
    path: impl AsRef<Path>,
    schema_version: u32,
    timestamp: u64,
    payload: &Value,
) -> Result<(), RecoveryError> {
    let path = path.as_ref();
    let parent = parent_dir(path)?;

    let payload_bytes = serde_json::to_vec(payload).map_err(RecoveryError::Serialize)?;
    ensure_payload_size(payload_bytes.len())?;

    let snapshot = RecoverySnapshot {
        schema_version,
        timestamp,
        payload: payload.clone(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(RecoveryError::Serialize)?;
    ensure_snapshot_size(bytes.len())?;

    ensure_private_parent(parent)?;
    reject_symlink_target(path)?;

    let (temporary_path, mut file) = create_temp_file(parent, path)?;
    let write_result = write_temp_file(&temporary_path, &mut file, &bytes);
    // Closing before rename keeps replacement portable to platforms that
    // disallow renaming an open file.
    drop(file);
    let result = write_result.and_then(|()| {
        // A second check closes the common case where the parent was replaced
        // by a symlink while the temporary file was being written.
        reject_symlink_components(parent)?;
        reject_symlink_target(path)?;
        fs::rename(&temporary_path, path).map_err(|source| RecoveryError::Io {
            operation: "atomically replace",
            path: path.to_path_buf(),
            source,
        })
    });

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

/// Load a recovery snapshot from `path`.
///
/// A missing snapshot is represented by `Ok(None)`. Existing malformed JSON or
/// an invalid envelope returns [`RecoveryError::Corrupt`].
pub fn load(path: impl AsRef<Path>) -> Result<Option<RecoverySnapshot>, RecoveryError> {
    let path = path.as_ref();
    let parent = parent_dir(path)?;
    reject_symlink_components(parent)?;

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(RecoveryError::Io {
                operation: "inspect",
                path: path.to_path_buf(),
                source,
            })
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(RecoveryError::PathContainsSymlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(RecoveryError::TargetIsNotRegularFile(path.to_path_buf()));
    }
    if metadata.len() > MAX_SERIALIZED_SNAPSHOT_BYTES as u64 {
        return Err(RecoveryError::SnapshotTooLarge {
            actual: metadata.len().try_into().unwrap_or(usize::MAX),
            maximum: MAX_SERIALIZED_SNAPSHOT_BYTES,
        });
    }

    let bytes = fs::read(path).map_err(|source| RecoveryError::Io {
        operation: "read",
        path: path.to_path_buf(),
        source,
    })?;
    let snapshot: RecoverySnapshot =
        serde_json::from_slice(&bytes).map_err(|source| RecoveryError::Corrupt {
            path: path.to_path_buf(),
            source,
        })?;
    let payload_size = serde_json::to_vec(&snapshot.payload)
        .map_err(RecoveryError::Serialize)?
        .len();
    ensure_payload_size(payload_size)?;
    Ok(Some(snapshot))
}

/// Explicitly remove a recovery snapshot. Missing snapshots are already clear.
pub fn clear(path: impl AsRef<Path>) -> Result<(), RecoveryError> {
    let path = path.as_ref();
    let parent = parent_dir(path)?;
    reject_symlink_components(parent)?;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(RecoveryError::PathContainsSymlink(path.to_path_buf()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(RecoveryError::TargetIsNotRegularFile(path.to_path_buf()))
        }
        Ok(_) => fs::remove_file(path).map_err(|source| RecoveryError::Io {
            operation: "clear",
            path: path.to_path_buf(),
            source,
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RecoveryError::Io {
            operation: "inspect",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parent_dir(path: &Path) -> Result<&Path, RecoveryError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Err(RecoveryError::InvalidPath(path.to_path_buf()));
    };
    if path.file_name().is_none() {
        return Err(RecoveryError::InvalidPath(path.to_path_buf()));
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(RecoveryError::InvalidPath(path.to_path_buf()));
    }
    Ok(parent)
}

fn ensure_payload_size(size: usize) -> Result<(), RecoveryError> {
    if size > MAX_SERIALIZED_PAYLOAD_BYTES {
        return Err(RecoveryError::PayloadTooLarge {
            actual: size,
            maximum: MAX_SERIALIZED_PAYLOAD_BYTES,
        });
    }
    Ok(())
}

fn ensure_snapshot_size(size: usize) -> Result<(), RecoveryError> {
    if size > MAX_SERIALIZED_SNAPSHOT_BYTES {
        return Err(RecoveryError::SnapshotTooLarge {
            actual: size,
            maximum: MAX_SERIALIZED_SNAPSHOT_BYTES,
        });
    }
    Ok(())
}

fn ensure_private_parent(parent: &Path) -> Result<(), RecoveryError> {
    let mut current = PathBuf::new();
    for component in parent.components() {
        if component == Component::ParentDir {
            return Err(RecoveryError::InvalidPath(parent.to_path_buf()));
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(RecoveryError::PathContainsSymlink(current));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(RecoveryError::Io {
                    operation: "create parent directory",
                    path: current,
                    source: io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "path component is not a directory",
                    ),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|source| RecoveryError::Io {
                    operation: "create parent directory",
                    path: current.clone(),
                    source,
                })?;
                set_mode(&current, PRIVATE_DIRECTORY_MODE)?;
            }
            Err(source) => {
                return Err(RecoveryError::Io {
                    operation: "inspect parent directory",
                    path: current,
                    source,
                });
            }
        }
    }
    set_mode(parent, PRIVATE_DIRECTORY_MODE)
}

fn reject_symlink_components(parent: &Path) -> Result<(), RecoveryError> {
    let mut current = PathBuf::new();
    for component in parent.components() {
        if component == Component::ParentDir {
            return Err(RecoveryError::InvalidPath(parent.to_path_buf()));
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(RecoveryError::PathContainsSymlink(current));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(RecoveryError::Io {
                    operation: "inspect parent directory",
                    path: current,
                    source: io::Error::new(
                        io::ErrorKind::NotADirectory,
                        "path component is not a directory",
                    ),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(RecoveryError::Io {
                    operation: "inspect parent directory",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn reject_symlink_target(path: &Path) -> Result<(), RecoveryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(RecoveryError::PathContainsSymlink(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(RecoveryError::Io {
            operation: "inspect target",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_temp_file(parent: &Path, target: &Path) -> Result<(PathBuf, File), RecoveryError> {
    let target_name = target
        .file_name()
        .ok_or_else(|| RecoveryError::InvalidPath(target.to_path_buf()))?
        .to_string_lossy();
    for _ in 0..MAX_TEMP_FILE_ATTEMPTS {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{target_name}.tmp-{}-{counter}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        set_create_mode(&mut options, PRIVATE_FILE_MODE);
        match options.open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(RecoveryError::Io {
                    operation: "create temporary snapshot",
                    path: candidate,
                    source,
                });
            }
        }
    }
    Err(RecoveryError::Io {
        operation: "create temporary snapshot",
        path: parent.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary file name collision limit reached",
        ),
    })
}

fn write_temp_file(path: &Path, file: &mut File, bytes: &[u8]) -> Result<(), RecoveryError> {
    file.write_all(bytes).map_err(|source| RecoveryError::Io {
        operation: "write temporary snapshot",
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| RecoveryError::Io {
        operation: "sync temporary snapshot",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), RecoveryError> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .map_err(|source| RecoveryError::Io {
            operation: "inspect path permissions",
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions).map_err(|source| RecoveryError::Io {
        operation: "set path permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), RecoveryError> {
    Ok(())
}

#[cfg(unix)]
fn set_create_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;

    options.mode(mode);
}

#[cfg(not(unix))]
fn set_create_mode(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_directory(name: &str) -> PathBuf {
        // macOS exposes the temporary directory through `/var`, which is
        // commonly a symlink to `/private/var`. Use the real path so the
        // symlink guard tests only the path components supplied by the test.
        let directory = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "sagascript-update-recovery-{name}-{}-{}",
                std::process::id(),
                TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn remove_test_directory(directory: &Path) {
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn roundtrip_preserves_metadata_and_opaque_payload() {
        let directory = test_directory("roundtrip");
        let path = directory.join("snapshot.json");
        let payload = serde_json::json!({
            "transcript": "opaque text",
            "nested": [true, 7, {"key": null}],
        });

        save(&path, 3, 1_723_456_789, &payload).unwrap();

        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.schema_version, 3);
        assert_eq!(loaded.timestamp, 1_723_456_789);
        assert_eq!(loaded.payload, payload);
        remove_test_directory(&directory);
    }

    #[test]
    fn rejects_payload_over_size_limit_without_creating_target() {
        let directory = test_directory("size-limit");
        let path = directory.join("snapshot.json");
        let payload = Value::String("x".repeat(MAX_SERIALIZED_PAYLOAD_BYTES));

        let error = save(&path, 1, 1, &payload).unwrap_err();
        assert!(matches!(error, RecoveryError::PayloadTooLarge { .. }));
        assert!(!path.exists());
        remove_test_directory(&directory);
    }

    #[test]
    fn corrupt_json_returns_explicit_error() {
        let directory = test_directory("corrupt");
        let path = directory.join("snapshot.json");
        fs::write(&path, b"{not json").unwrap();

        let error = load(&path).unwrap_err();
        assert!(matches!(error, RecoveryError::Corrupt { .. }));
        remove_test_directory(&directory);
    }

    #[test]
    fn replacement_is_atomic_from_the_reader_perspective() {
        let directory = test_directory("replacement");
        let path = directory.join("snapshot.json");
        save(&path, 1, 1, &serde_json::json!({"generation": 1})).unwrap();
        save(&path, 2, 2, &serde_json::json!({"generation": 2})).unwrap();

        let loaded = load(&path).unwrap().unwrap();
        assert_eq!(loaded.schema_version, 2);
        assert_eq!(loaded.timestamp, 2);
        assert_eq!(loaded.payload["generation"], 2);
        let temporary_files = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(temporary_files, 0);
        remove_test_directory(&directory);
    }

    #[test]
    fn clear_is_explicit_and_idempotent() {
        let directory = test_directory("clear");
        let path = directory.join("snapshot.json");
        save(&path, 1, 1, &serde_json::json!({"keep": true})).unwrap();
        assert!(path.exists());
        clear(&path).unwrap();
        assert!(!path.exists());
        clear(&path).unwrap();
        remove_test_directory(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn parent_and_file_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory("permissions");
        let parent = directory.join("private");
        let path = parent.join("snapshot.json");
        save(&path, 1, 1, &serde_json::json!({"private": true})).unwrap();

        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        remove_test_directory(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_target_and_parent() {
        use std::os::unix::fs::symlink;

        let directory = test_directory("symlink");
        let real_target = directory.join("real.json");
        fs::write(&real_target, b"sentinel").unwrap();
        let linked_target = directory.join("snapshot.json");
        symlink(&real_target, &linked_target).unwrap();
        let payload = serde_json::json!({"should": "not follow"});

        assert!(matches!(
            save(&linked_target, 1, 1, &payload),
            Err(RecoveryError::PathContainsSymlink(_))
        ));
        assert!(matches!(
            load(&linked_target),
            Err(RecoveryError::PathContainsSymlink(_))
        ));
        assert!(matches!(
            clear(&linked_target),
            Err(RecoveryError::PathContainsSymlink(_))
        ));
        assert_eq!(fs::read(&real_target).unwrap(), b"sentinel");

        let real_parent = directory.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let linked_parent = directory.join("linked-parent");
        symlink(&real_parent, &linked_parent).unwrap();
        let linked_path = linked_parent.join("snapshot.json");
        assert!(matches!(
            save(&linked_path, 1, 1, &payload),
            Err(RecoveryError::PathContainsSymlink(_))
        ));
        remove_test_directory(&directory);
    }
}
