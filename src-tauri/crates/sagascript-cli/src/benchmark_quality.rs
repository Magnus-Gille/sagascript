//! Explicit local quality-export I/O. Normal benchmark output stays content-free.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use sagascript_core::error::DictationError;
use serde::Serialize;
use sha2::{Digest, Sha256};

const MAX_REPORT_BYTES: usize = 16 * 1024 * 1024;
const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;

pub(crate) fn ensure_unused_destination(path: &Path) -> Result<(), DictationError> {
    if path.file_name().is_none() {
        return Err(DictationError::SettingsError(
            "Quality output requires a new file name in an existing local directory".into(),
        ));
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Err(DictationError::SettingsError(
            "Quality output already exists; choose an unused path".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DictationError::SettingsError(format!(
            "Could not inspect local quality output destination: {error}"
        ))),
    }
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = std::fs::metadata(parent).map_err(|error| {
        DictationError::SettingsError(format!("Quality output parent is unavailable: {error}"))
    })?;
    if !metadata.is_dir() {
        return Err(DictationError::SettingsError(
            "Quality output parent must be an existing directory".into(),
        ));
    }
    // This is only an early diagnostic. Final create_new handles races.
    Ok(())
}

pub(crate) fn input_digest(path: &Path) -> Result<String, DictationError> {
    let mut file = File::open(path).map_err(|error| {
        DictationError::FileDecodeError(format!("Could not open quality fixture for hashing: {error}"))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 32 * 1024];
    let mut total_bytes = 0_u64;
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            DictationError::FileDecodeError(format!("Could not hash quality fixture: {error}"))
        })?;
        if count == 0 {
            break;
        }
        total_bytes += count as u64;
        if total_bytes > MAX_INPUT_BYTES {
            return Err(DictationError::FileDecodeError(
                "Quality fixtures must not exceed 128 MiB".into(),
            ));
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn decoded_digest(samples: &[f32]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"sagascript-pcm-f32le-16000-v1\0");
    for sample in samples {
        digest.update(sample.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}

pub(crate) fn write_private_new(
    path: &Path,
    value: &impl Serialize,
) -> Result<(), DictationError> {
    // Encode before creating the destination: serialization/size errors must
    // not create an apparently usable evidence file.
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        DictationError::SettingsError(format!("Could not encode local quality report: {error}"))
    })?;
    if bytes.len() >= MAX_REPORT_BYTES {
        return Err(DictationError::SettingsError(
            "Local quality report exceeds the 16 MiB export limit".into(),
        ));
    }
    bytes.push(b'\n');
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // create_new is atomic: existing files, directories and symlinks are never
    // overwritten. Windows inherits the explicitly chosen directory's ACL.
    let mut file = options.open(path).map_err(|error| {
        DictationError::SettingsError(format!(
            "Could not create new local quality output (choose an unused path): {error}"
        ))
    })?;
    file.write_all(&bytes).and_then(|()| file.sync_all()).map_err(|error| {
        // Do not unlink by path on error: a concurrent owner could replace it.
        // A partial file is invalid evidence and is never appended to/reused.
        DictationError::SettingsError(format!(
            "Could not finish local quality output; discard any incomplete report: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sagascript-quality-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove only this test's unique temporary directory");
        }
    }

    #[test]
    fn explicitly_requested_export_is_utf8_json_in_a_new_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("quality.json");
        let value = serde_json::json!({"text": "Åäö\nexample", "schema_version": 1});
        ensure_unused_destination(&path).unwrap();
        write_private_new(&path, &value).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(), value);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn existing_file_directory_and_symlink_are_never_overwritten() {
        let directory = TestDirectory::new();
        let path = directory.0.join("existing.json");
        fs::write(&path, b"preserve original bytes").unwrap();
        let value = serde_json::json!({"text": "replacement"});
        assert!(ensure_unused_destination(&path).is_err());
        assert!(write_private_new(&path, &value).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"preserve original bytes");
        assert!(write_private_new(&directory.0, &value).is_err());
        #[cfg(unix)]
        {
            let link = directory.0.join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(ensure_unused_destination(&link).is_err());
            assert!(write_private_new(&link, &value).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"preserve original bytes");
        }
    }

    #[test]
    fn file_created_after_preflight_is_preserved() {
        let directory = TestDirectory::new();
        let path = directory.0.join("raced.json");
        ensure_unused_destination(&path).unwrap();
        fs::write(&path, b"another writer won").unwrap();
        assert!(write_private_new(&path, &serde_json::json!({"text": "new"})).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"another writer won");
    }

    #[test]
    fn missing_parent_is_not_created_implicitly() {
        let directory = TestDirectory::new();
        let parent = directory.0.join("missing");
        assert!(ensure_unused_destination(&parent.join("quality.json")).is_err());
        assert!(write_private_new(&parent.join("quality.json"), &serde_json::json!({})).is_err());
        assert!(!parent.exists());
    }

    #[test]
    fn oversized_report_fails_before_creating_a_file() {
        let directory = TestDirectory::new();
        let path = directory.0.join("too-large.json");
        let value = "x".repeat(MAX_REPORT_BYTES);
        assert!(write_private_new(&path, &value).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn serialization_error_does_not_create_a_file() {
        struct Unserializable;
        impl Serialize for Unserializable {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("deliberate test serialization error"))
            }
        }
        let directory = TestDirectory::new();
        let path = directory.0.join("invalid.json");
        let error = write_private_new(&path, &Unserializable).unwrap_err();
        assert!(error.to_string().contains("Could not encode local quality report"));
        assert!(!path.exists());
    }

    #[test]
    fn byte_digest_is_independent_of_file_name_and_detects_changes() {
        let directory = TestDirectory::new();
        let first = directory.0.join("a");
        let second = directory.0.join("b");
        fs::write(&first, b"abc").unwrap();
        fs::write(&second, b"abc").unwrap();
        let digest = input_digest(&first).unwrap();
        assert_eq!(digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(digest, input_digest(&second).unwrap());
        fs::write(&second, b"abd").unwrap();
        assert_ne!(digest, input_digest(&second).unwrap());
    }

    #[test]
    fn decoded_digest_tracks_actual_f32_samples() {
        let digest = decoded_digest(&[0.0, 0.25, -0.5]);
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, decoded_digest(&[0.0, 0.25, -0.5]));
        assert_ne!(digest, decoded_digest(&[0.0, 0.25, -0.4]));
        assert_ne!(decoded_digest(&[0.0]), decoded_digest(&[-0.0]));
    }
}
