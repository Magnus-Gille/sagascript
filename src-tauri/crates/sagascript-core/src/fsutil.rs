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
//!    name does not exist yet, so a reader never sees a partial file. A
//!    filesystem that cannot fsync (some network and external volumes) is
//!    tolerated with a warning, as in `download.rs`; a real I/O error is not.
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
//! | `Ok` | holds exactly the bytes, complete | removed, or reported in [`Published::leftover`]: a warning for the caller, never a failure, because the artifact is complete |
//! | [`PublishErrorKind::AlreadyExists`] | untouched: a file, directory or symlink of someone else | removed |
//! | any other error | not created | removed, or reported in [`PublishError::leftover`] |
//!
//! After an error nothing was published, so trying again is safe: a retry can
//! only succeed once the destination is free. Of several publishers racing for
//! one destination exactly one succeeds. A process killed mid-publication
//! leaves at most a staging file, which nothing reads and
//! [`staging_leftovers`] finds; it is complete if the process got past its
//! fsync. A staging file is reported as left over only if it exists.
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

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The longest part of a destination name copied into a staging name, in the
/// unit the filesystem limits: UTF-8 bytes on Unix (255 per name), UTF-16 units
/// elsewhere. A longer name is cut and gets a hash of the whole name appended
/// (`.<cut>-<hash>`, 110 units), so `.<name>.<uuid>.tmp` is then at most 151,
/// and a staging name of a staging name 188, of the 255 allowed.
const MAX_STAGING_NAME_UNITS: usize = 100;
/// The hex digits of the name hash kept in the stem of a cut name.
const NAME_HASH_DIGITS: usize = 8;
/// The length of a hyphenated UUID.
const ID_LEN: usize = 36;

/// The filesystem operations of a publication: a seam so tests can inject the
/// failures a real disk rarely produces (a failed link, a failed unlink, a
/// volume that cannot fsync).
pub trait PublishFs {
    /// Create a new private file (mode `0600` on Unix); fails when it exists.
    fn create_new(&self, path: &Path) -> io::Result<File>;
    /// Flush a file's data to stable storage.
    fn sync_file(&self, file: &File) -> io::Result<()>;
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
    fn create_new(&self, path: &Path) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path)
    }

    fn sync_file(&self, file: &File) -> io::Result<()> {
        sync_durably(file)
    }

    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::hard_link(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            sync_durably(&File::open(dir)?)
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
    publish_new_with(destination, bytes, &RealFs)
}

/// [`publish_new`] on the given filesystem operations.
pub fn publish_new_with(
    destination: &Path,
    bytes: &[u8],
    ops: &dyn PublishFs,
) -> Result<Published, PublishError> {
    StagedFile::create_with(destination, bytes, ops)?.publish_with(ops)
}

/// Check that `destination` names a file in an existing directory, the part of
/// [`publish_new`] that can be checked before the bytes exist.
pub fn check_destination_folder(destination: &Path) -> Result<(), PublishError> {
    destination_parts(destination).map(|_| ())
}

fn destination_parts(destination: &Path) -> Result<(&Path, &OsStr), PublishError> {
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

/// The longest prefix of `name` that fits [`MAX_STAGING_NAME_UNITS`].
fn truncated(name: &str) -> String {
    let mut used = 0;
    let mut kept = String::new();
    for character in name.chars() {
        #[cfg(unix)]
        let width = character.len_utf8();
        #[cfg(not(unix))]
        let width = character.len_utf16();
        if used + width > MAX_STAGING_NAME_UNITS {
            break;
        }
        used += width;
        kept.push(character);
    }
    kept
}

/// The stem every staging name of the file name `name` starts with: `.<name>`.
/// A name that had to be cut keeps a hash of the whole name, so two names that
/// share their first [`MAX_STAGING_NAME_UNITS`] units do not share a stem.
fn staging_stem(name: &str) -> String {
    let kept = truncated(name);
    if kept.len() == name.len() {
        return format!(".{kept}");
    }
    let hash = crate::hash::sha256_hex(name);
    format!(".{kept}-{}", &hash[..NAME_HASH_DIGITS])
}

/// Whether `text` ends in `.<uuid>`.
fn ends_with_id(text: &str) -> bool {
    text.len() > ID_LEN
        && text
            .get(text.len() - ID_LEN - 1..)
            .is_some_and(|tail| tail.starts_with('.') && uuid::Uuid::parse_str(&tail[1..]).is_ok())
}

/// How the staging names of one destination are built and recognised. The
/// producer ([`reserve`](StagedFile::reserve)) and the matcher
/// ([`staging_leftovers`]) both derive it here, so they cannot disagree.
struct StagingNames {
    /// Every staging name is `<prefix>.<uuid>.tmp`, or with a second id.
    prefix: String,
    /// The most ids after the prefix the matcher accepts.
    max_ids: usize,
}

impl StagingNames {
    /// For the destination file name `name`. A destination that is itself a
    /// staging name (a reserved writer publishing its reserved path) keeps
    /// that name as the prefix and gains exactly one id, so the result is still
    /// attributable to the final destination and the destination is never its
    /// own staging file. Any other name `n` has `.<n>.<uuid>.tmp`, and the
    /// staging name of that, `.<n>.<uuid>.<uuid>.tmp`.
    fn of(name: &OsStr) -> Self {
        let lossy = name.to_string_lossy();
        match lossy
            .strip_suffix(".tmp")
            .filter(|base| base.starts_with('.') && ends_with_id(base))
        {
            Some(base) => Self {
                prefix: base.to_string(),
                max_ids: 1,
            },
            None => Self {
                prefix: staging_stem(&lossy),
                max_ids: 2,
            },
        }
    }

    /// A fresh staging file name.
    fn fresh(&self) -> String {
        format!("{}.{}.tmp", self.prefix, uuid::Uuid::new_v4())
    }

    /// Whether `file_name` is a staging name of this destination.
    fn matches(&self, file_name: &str) -> bool {
        let Some(ids) = file_name
            .strip_prefix(&self.prefix)
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .and_then(|ids| ids.strip_prefix('.'))
        else {
            return false;
        };
        let parts = ids.split('.').collect::<Vec<_>>();
        (1..=self.max_ids).contains(&parts.len())
            && parts
                .iter()
                .all(|part| part.len() == ID_LEN && uuid::Uuid::parse_str(part).is_ok())
    }
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
        Self::create_with(destination, bytes, &RealFs)
    }

    /// [`create`](Self::create) on the given filesystem operations.
    pub fn create_with(
        destination: &Path,
        bytes: &[u8],
        ops: &dyn PublishFs,
    ) -> Result<Self, PublishError> {
        let staged = Self::reserve(destination)?;
        match write_private(ops, &staged.staged, bytes) {
            Ok(()) => Ok(staged),
            Err(error) => {
                let mut failure = PublishError::new(PublishErrorKind::Stage(error));
                failure.leftover = staged.discard_reporting(ops);
                Err(failure)
            }
        }
    }

    /// Reserve a private staging path beside `destination` for a writer that
    /// creates the file itself, as a pipeline that produces a large artifact
    /// does. The writer must create it complete and atomically (for example
    /// with [`publish_new`], whose own staging name then carries this one's
    /// as a stem) and never open it for writing again; whatever is at the path
    /// when [`publish`](Self::publish) runs is published.
    pub fn reserve(destination: &Path) -> Result<Self, PublishError> {
        let (parent, name) = destination_parts(destination)?;
        let staged = parent.join(StagingNames::of(name).fresh());
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
                if sync_unsupported(&error) {
                    tracing::debug!(kind = ?error.kind(), "this filesystem cannot sync a folder");
                } else {
                    tracing::warn!(kind = ?error.kind(), "could not sync a published file's folder");
                }
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

    fn discard_reporting(mut self, ops: &dyn PublishFs) -> Option<Leftover> {
        self.live = false;
        remove_staged(ops, &self.staged)
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

/// Remove the staging file. A failure is a [`Leftover`] only if the file is
/// still there: a staging file whose creation failed (a name that is too long,
/// a read-only folder) does not exist, and removing it fails for that same
/// reason. Existence is asked of the real filesystem, not of `ops`: every
/// [`PublishFs`] wraps the real filesystem and injects a fault on top of it, so
/// there is no failure to inject here, and a trait method would only be
/// forwarded by each implementation.
fn remove_staged(ops: &dyn PublishFs, staged: &Path) -> Option<Leftover> {
    match ops.remove_file(staged) {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) if fs::symlink_metadata(staged).is_err() => None,
        Err(error) => Some(Leftover {
            path: staged.to_path_buf(),
            error,
        }),
    }
}

/// Create `path` privately and write `bytes` to it, fsynced. A volume that
/// cannot fsync is tolerated with a warning; any other failure is returned.
fn write_private(ops: &dyn PublishFs, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = ops.create_new(path)?;
    file.write_all(bytes)?;
    match ops.sync_file(&file) {
        Err(error) if sync_unsupported(&error) => {
            tracing::warn!(
                kind = ?error.kind(),
                "this filesystem cannot sync files; publishing without a durability guarantee"
            );
            Ok(())
        }
        result => result,
    }
}

/// Staging files of `destination` that an interrupted process left behind:
/// the siblings named `.<name>.<uuid>.tmp`, and `.<name>.<uuid>.<uuid>.tmp`
/// (the staging file of a reserved staging path). Nothing reads them; each is
/// complete when its writer got past its fsync, so a survivor of an
/// interrupted run is worth inspecting before it is deleted. Unreadable
/// folders give an empty list.
pub fn staging_leftovers(destination: &Path) -> Vec<PathBuf> {
    let Ok((parent, name)) = destination_parts(destination) else {
        return Vec::new();
    };
    let names = StagingNames::of(name);
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut found = entries
        .flatten()
        .filter(|entry| names.matches(&entry.file_name().to_string_lossy()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    found.sort();
    found
}

// The fsync policy of every publication and install (#368, #342).

/// Flush `file` to stable storage: `File::sync_all`, which is `fsync` on
/// Linux and `FlushFileBuffers` on Windows. On Apple platforms std's
/// `sync_all` is only `fcntl(F_FULLFSYNC)`, which SMB and NFS homes, macFUSE
/// and some external drives reject, so there `fsync(2)` is the fallback. That
/// differs from SQLite, which falls back on any `F_FULLFSYNC` failure: here
/// only a failure of the unsupported class ([`sync_unsupported`]) falls back,
/// and EIO, ENOSPC and EDQUOT stay fatal, on purpose.
pub(crate) fn sync_durably(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        sync_with_fallback(|| file.sync_all(), || fsync(file))
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        file.sync_all()
    }
}

/// Run `plain` when `full` fails only because the file or volume cannot do
/// that kind of sync ([`sync_unsupported`]); any other failure is real and is
/// returned as it is.
#[cfg(any(target_vendor = "apple", test))]
pub(crate) fn sync_with_fallback(
    full: impl FnOnce() -> io::Result<()>,
    plain: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    match full() {
        Err(error) if sync_unsupported(&error) => {
            tracing::debug!("Full sync rejected ({error}); falling back to the plain sync");
            plain()
        }
        result => result,
    }
}

/// `fsync(2)` without Apple's full-flush semantics.
#[cfg(target_vendor = "apple")]
pub(crate) fn fsync(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    loop {
        // SAFETY: the descriptor stays open while `file` is borrowed, and
        // `fsync` reads or writes no memory of ours.
        if unsafe { libc::fsync(file.as_raw_fd()) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Whether a failed sync only means that this file or filesystem cannot sync,
/// rather than that written data may be lost. Some network, FUSE and external
/// volumes reject Apple's `F_FULLFSYNC` (and then [`sync_durably`] has tried
/// `fsync(2)` too), and some Windows redirectors reject `FlushFileBuffers`; a
/// models directory there installs without the sync, as every download did
/// before #368. I/O errors, a full disk or an exceeded quota stay fatal.
pub(crate) fn sync_unsupported(error: &io::Error) -> bool {
    use io::ErrorKind;
    // ENOTTY is 25 everywhere; ENOTSUP/EOPNOTSUPP are 95 on Linux, 45 and 102
    // on Apple platforms. std maps none of them to `Unsupported`. On Windows,
    // `FlushFileBuffers` fails with ERROR_INVALID_FUNCTION (1) or
    // ERROR_NOT_SUPPORTED (50), which std leaves uncategorized.
    #[cfg(target_os = "linux")]
    const NOT_SUPPORTED: &[i32] = &[25, 95];
    #[cfg(target_vendor = "apple")]
    const NOT_SUPPORTED: &[i32] = &[25, 45, 102];
    #[cfg(windows)]
    const NOT_SUPPORTED: &[i32] = &[1, 50];
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
    const NOT_SUPPORTED: &[i32] = &[];
    // EINVAL from fsync(2): the descriptor has no sync operation. On Windows
    // `InvalidInput` is ERROR_INVALID_PARAMETER, which is a real bug.
    #[cfg(unix)]
    let no_sync_operation = error.kind() == ErrorKind::InvalidInput;
    #[cfg(not(unix))]
    let no_sync_operation = false;
    error.kind() == ErrorKind::Unsupported
        || no_sync_operation
        || error
            .raw_os_error()
            .is_some_and(|code| NOT_SUPPORTED.contains(&code))
}

/// A sync that the volume cannot do is not an error; any other failure is.
#[cfg(unix)]
pub(crate) fn tolerate_unsupported(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if sync_unsupported(&error) => Ok(()),
        result => result,
    }
}

#[cfg(test)]
#[path = "fsutil_tests.rs"]
mod tests;
