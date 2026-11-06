//! The few filesystem primitives of the recording store, in one place so that
//! the permission, exclusivity and fsync policy is the same everywhere
//! (ADR 0008's `fsutil::sync_durably` is the fsync).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

use crate::fsutil::{sync_durably, sync_unsupported};

/// Create a new file (it must not exist) readable and writable by the owner
/// only: mode `0600` on Unix.
pub(crate) fn create_new_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Create a directory that must not exist yet (`create_dir` semantics: no
/// parents, fails with `AlreadyExists`), mode `0700` on Unix.
pub(crate) fn create_dir_private(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Open an existing file for reading and writing.
pub(crate) fn open_read_write(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

/// Flush a file's data to stable storage. A volume that cannot fsync (some
/// network and external volumes) is tolerated, as in `fsutil`; a real I/O
/// error, a full disk or an exceeded quota is returned.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    match sync_durably(file) {
        Err(error) if sync_unsupported(&error) => {
            tracing::debug!(kind = ?error.kind(), "this filesystem cannot sync a recording file");
            Ok(())
        }
        result => result,
    }
}

/// Flush a directory entry to disk; a no-op where that is not possible
/// (Windows) or not supported by the volume.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        match File::open(dir).and_then(|handle| sync_file(&handle)) {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(()),
            result => result,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

/// Write `bytes` to `path` through a temporary sibling and rename it over
/// `path`, fsyncing the data before the rename and the folder after it. This
/// is the manifest's finish (design §4.5 step 5): replacing our own file is
/// the intent, so this is `rename`, not `fsutil::publish_new`.
pub(crate) fn replace_file_durably(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = Path::new(&temporary);
    let result = (|| {
        // A leftover of an interrupted earlier attempt is ours; replace it.
        let _ = fs::remove_file(temporary);
        let mut file = create_new_private(temporary)?;
        file.write_all(bytes)?;
        sync_file(&file)?;
        drop(file);
        rename_over(temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
        return result;
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        sync_dir(parent)?;
    }
    Ok(())
}

/// `rename` that replaces an existing destination. On Windows an indexer or a
/// virus scanner can hold the destination open for a moment, which fails the
/// rename with an access error; that is retried a few times.
fn rename_over(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let mut attempt = 0;
        loop {
            match fs::rename(from, to) {
                Err(error) if attempt < 5 && error.kind() == io::ErrorKind::PermissionDenied => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                result => return result,
            }
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)
    }
}
