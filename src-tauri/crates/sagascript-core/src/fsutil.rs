//! Publishing new files without ever replacing one (#342, ADR 0008).
//!
//! Every artifact a user names as an output (an export, a proposal, a cache)
//! is published through this module, so one contract answers "what is on disk
//! when this fails, and may I try again?".
//!
//! # How a file is published
//!
//! 1. The bytes are written to a private staging file beside the destination
//!    (`.<name>.<uuid>.tmp`, mode `0600` on Unix) and fsynced. The destination
//!    name does not exist yet, so a reader never sees a partial file.
//! 2. The staging file is hard-linked to the destination. Creating a hard link
//!    fails when the destination exists, so this is the no-replace commit
//!    point, atomic against concurrent publishers. `rename` is not used: it
//!    replaces an existing destination on Unix and on Windows.
//! 3. The staging name is removed and, on Unix, the directory is fsynced
//!    (best effort).
//!
//! # Outcomes
//!
//! | Result | Destination | Staging file |
//! |---|---|---|
//! | `Ok` | holds exactly the bytes, complete | removed, or reported in [`Published::leftover`] |
//! | [`PublishErrorKind::AlreadyExists`] | untouched: a file, directory or symlink of someone else | removed |
//! | any other error | not created | removed, or reported in [`PublishError::leftover`] |
//!
//! After an error nothing was published, so trying again is safe: a retry can
//! only succeed once the destination is free. Of several publishers racing for
//! one destination exactly one succeeds. A process killed mid-publication
//! leaves at most a staging file, which nothing reads and
//! [`staging_leftovers`] finds; it is complete if the process got past its
//! fsync.
//!
//! The commit needs a filesystem with hard links (not FAT, exFAT or most
//! network shares); there the publication fails cleanly and publishes nothing.
//! Like the model downloads (`download.rs`) the file is fsynced before it gets
//! its final name, so a crash cannot leave that name over unwritten data.
//!
//! # Related artifacts
//!
//! One file is atomic; two are not. [`StagedFile`] lets a caller stage every
//! artifact of a set first, validate, and only then publish them one after the
//! other in an order it documents. Dropping a [`StagedFile`] removes it, so
//! every early return is a clean "nothing published".

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The longest destination file name copied into a staging name, so that
/// `.<name>.<uuid>.tmp` stays below the 255-byte limit of common filesystems.
const MAX_STAGING_NAME_CHARS: usize = 80;

/// The filesystem operations of a publication: a seam so tests can inject the
/// failures a real disk rarely produces (a failed link, a failed unlink).
pub trait PublishFs {
    /// Create `to` as a hard link of `from`; fails when `to` exists.
    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Remove a file.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Flush a directory entry to disk; a no-op where that is not possible.
    fn sync_dir(&self, dir: &Path) -> io::Result<()>;
}

/// The real filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

impl PublishFs for RealFs {
    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::hard_link(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            File::open(dir)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            Ok(())
        }
    }
}

/// Why a publication did not happen.
#[derive(Debug, thiserror::Error)]
pub enum PublishErrorKind {
    #[error("the destination has no file name")]
    NoFileName,
    #[error("the destination folder could not be inspected: {0}")]
    ParentUnavailable(#[source] io::Error),
    #[error("the destination folder is not a directory")]
    ParentNotDirectory,
    /// The private staging file could not be created, written or synced.
    #[error("the private staging file could not be written: {0}")]
    Stage(#[source] io::Error),
    /// The destination exists; it was not touched.
    #[error("the destination already exists")]
    AlreadyExists,
    /// [`StagedFile::reserve`] was published but nothing was written there.
    #[error("nothing was staged for publication")]
    NothingStaged,
    /// The commit (the hard link) failed for a reason other than an existing
    /// destination.
    #[error("the file could not be published: {0}")]
    Commit(#[source] io::Error),
}

/// A private staging file that could not be removed. It may be a second name
/// for a published file, so it can hold the same private content.
#[derive(Debug)]
pub struct Leftover {
    pub path: PathBuf,
    pub error: io::Error,
}

/// A publication that did not happen: nothing was created at the destination.
#[derive(Debug, thiserror::Error)]
#[error("{kind}")]
pub struct PublishError {
    pub kind: PublishErrorKind,
    /// The staging file, when it could not be removed after the failure.
    pub leftover: Option<Leftover>,
}

impl PublishError {
    fn new(kind: PublishErrorKind) -> Self {
        Self {
            kind,
            leftover: None,
        }
    }
}

/// A publication that happened: the destination holds the complete bytes.
#[derive(Debug, Default)]
pub struct Published {
    /// The staging name, when it could not be removed. The destination is
    /// complete regardless; remove the staging file when convenient.
    pub leftover: Option<Leftover>,
}

/// Publish `bytes` as the new file `destination`; see the module contract.
pub fn publish_new(destination: &Path, bytes: &[u8]) -> Result<Published, PublishError> {
    StagedFile::create(destination, bytes)?.publish()
}

/// Check that `destination` names a file in an existing directory, the part of
/// [`publish_new`] that can be checked before the bytes exist.
pub fn check_destination_folder(destination: &Path) -> Result<(), PublishError> {
    destination_parts(destination).map(|_| ())
}

fn destination_parts(destination: &Path) -> Result<(&Path, &std::ffi::OsStr), PublishError> {
    let name = destination
        .file_name()
        .ok_or_else(|| PublishError::new(PublishErrorKind::NoFileName))?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::metadata(parent)
        .map_err(|error| PublishError::new(PublishErrorKind::ParentUnavailable(error)))?;
    if !metadata.is_dir() {
        return Err(PublishError::new(PublishErrorKind::ParentNotDirectory));
    }
    Ok((parent, name))
}

/// The prefix of every staging name of `name`: `.<name>.`
fn staging_prefix(name: &std::ffi::OsStr) -> String {
    let name = name.to_string_lossy();
    let name: String = name.chars().take(MAX_STAGING_NAME_CHARS).collect();
    format!(".{name}.")
}

/// A file written privately beside its destination, not yet published.
///
/// Dropping it removes the staging file (if something is there), so an early
/// return anywhere cleans up. [`publish`](Self::publish) commits it;
/// [`discard`](Self::discard) removes it and reports whether that worked.
#[derive(Debug)]
pub struct StagedFile {
    staged: PathBuf,
    destination: PathBuf,
    live: bool,
}

impl StagedFile {
    /// Write `bytes` to a private staging file beside `destination` and fsync
    /// them. Nothing is created at `destination`.
    pub fn create(destination: &Path, bytes: &[u8]) -> Result<Self, PublishError> {
        let staged = Self::reserve(destination)?;
        let written = write_private(&staged.staged, bytes);
        match written {
            Ok(()) => Ok(staged),
            Err(error) => {
                let mut failure = PublishError::new(PublishErrorKind::Stage(error));
                failure.leftover = staged.discard_reporting();
                Err(failure)
            }
        }
    }

    /// Reserve a private staging path beside `destination` for a writer that
    /// creates the file itself, as a pipeline that produces a large artifact
    /// does. The writer must create it complete and atomically (for example
    /// with [`publish_new`]) and never open it for writing again; whatever is
    /// at the path when [`publish`](Self::publish) runs is published.
    pub fn reserve(destination: &Path) -> Result<Self, PublishError> {
        let (parent, name) = destination_parts(destination)?;
        let staged = parent.join(format!(
            "{}{}.tmp",
            staging_prefix(name),
            uuid::Uuid::new_v4()
        ));
        Ok(Self {
            staged,
            destination: destination.to_path_buf(),
            live: true,
        })
    }

    /// Where the staged bytes are (or will be written by the reserved writer).
    pub fn staged_path(&self) -> &Path {
        &self.staged
    }

    /// The path this file will be published at.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// Whether a regular file is staged (always true after [`create`](Self::create)).
    pub fn is_staged(&self) -> bool {
        fs::symlink_metadata(&self.staged).is_ok_and(|metadata| metadata.is_file())
    }

    /// Publish on the real filesystem; see [`publish_with`](Self::publish_with).
    pub fn publish(self) -> Result<Published, PublishError> {
        self.publish_with(&RealFs)
    }

    /// Commit the staged file to its destination without replacing anything.
    ///
    /// On `Ok` the destination holds the complete bytes. On `Err` nothing was
    /// created at the destination and the staging file is removed (or reported
    /// in [`PublishError::leftover`]).
    pub fn publish_with(mut self, ops: &dyn PublishFs) -> Result<Published, PublishError> {
        let linked = ops.hard_link(&self.staged, &self.destination);
        let kind = match linked {
            Ok(()) => None,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Some(PublishErrorKind::AlreadyExists)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && !self.is_staged() => {
                Some(PublishErrorKind::NothingStaged)
            }
            Err(error) => Some(PublishErrorKind::Commit(error)),
        };
        self.live = false;
        let leftover = remove_staged(ops, &self.staged);
        if let Some(kind) = kind {
            return Err(PublishError { kind, leftover });
        }
        if let Some(parent) = self
            .destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            if let Err(error) = ops.sync_dir(parent) {
                // The file is published; only its durability is unconfirmed.
                tracing::debug!(kind = ?error.kind(), "could not sync a published file's folder");
            }
        }
        Ok(Published { leftover })
    }

    /// Remove the staging file now and report whether that worked.
    pub fn discard(mut self) -> io::Result<()> {
        self.live = false;
        match fs::remove_file(&self.staged) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn discard_reporting(mut self) -> Option<Leftover> {
        self.live = false;
        remove_staged(&RealFs, &self.staged)
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Err(error) = fs::remove_file(&self.staged) {
            if error.kind() != io::ErrorKind::NotFound {
                // No path: it is the user's, and logs never carry those.
                tracing::warn!(kind = ?error.kind(), "could not remove a private staging file");
            }
        }
    }
}

fn remove_staged(ops: &dyn PublishFs, staged: &Path) -> Option<Leftover> {
    match ops.remove_file(staged) {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(Leftover {
            path: staged.to_path_buf(),
            error,
        }),
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file: File = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Staging files of `destination` that an interrupted process left behind:
/// the siblings named `.<name>.<uuid>.tmp`. Nothing reads them; each is
/// complete when its writer got past its fsync, so a survivor of an
/// interrupted run is worth inspecting before it is deleted. Unreadable
/// folders give an empty list.
pub fn staging_leftovers(destination: &Path) -> Vec<PathBuf> {
    let Ok((parent, name)) = destination_parts(destination) else {
        return Vec::new();
    };
    let prefix = staging_prefix(name);
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found = entries
        .flatten()
        .filter(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            file_name
                .strip_prefix(&prefix)
                .and_then(|rest| rest.strip_suffix(".tmp"))
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    found.sort();
    found
}

#[cfg(test)]
#[path = "fsutil_tests.rs"]
mod tests;
