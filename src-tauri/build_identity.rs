//! Shared local build-identity handling for the app and CLI build scripts.

use std::{fs, path::Path, process::Command, str};

const GENERATED_PATHS: &str = include_str!("../scripts/build-identity-generated-paths.txt");

fn git_output(args: &[&str]) -> Option<Vec<u8>> {
    Command::new("git")
        .args(args)
        // Observation must not refresh the index that Cargo itself watches.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
}

fn repository_root() -> Option<String> {
    let output = git_output(&["rev-parse", "--show-toplevel"])?;
    let output = output.strip_suffix(b"\n").unwrap_or(&output);
    let output = output.strip_suffix(b"\r").unwrap_or(output);
    let root = str::from_utf8(output).ok()?;
    (!root.is_empty()).then(|| root.to_string())
}

fn is_allowed_generated_file(repository_root: &str, path: &[u8]) -> bool {
    let Ok(path) = str::from_utf8(path) else {
        return false;
    };
    if !GENERATED_PATHS
        .lines()
        .map(str::trim)
        .any(|allowed| !allowed.is_empty() && !allowed.starts_with('#') && allowed == path)
    {
        return false;
    }

    fs::symlink_metadata(Path::new(repository_root).join(path))
        .is_ok_and(|metadata| metadata.file_type().is_file())
}

fn status_is_clean(status: &[u8], repository_root: &str) -> bool {
    if status.is_empty() {
        return true;
    }
    if !status.ends_with(b"\0") {
        return false;
    }

    let mut offset = 0;
    while offset < status.len() {
        let Some(record_length) = status[offset..].iter().position(|byte| *byte == 0) else {
            return false;
        };
        let record_end = offset + record_length;
        let record = &status[offset..record_end];
        if record.len() < 4 || record[2] != b' ' {
            return false;
        }

        let code = &record[..2];
        let path = &record[3..];
        if path.is_empty() {
            return false;
        }

        if code.contains(&b'R') || code.contains(&b'C') {
            return false;
        }

        let supported_status = code == b"??"
            || (code != b"  "
                && code != b"AA"
                && code.iter().all(|byte| matches!(*byte, b' ' | b'M' | b'A')));
        if !supported_status || !is_allowed_generated_file(repository_root, path) {
            return false;
        }

        offset = record_end + 1;
    }
    true
}

/// Tauri's build script rewrites these, so watching them would invalidate every build.
const BUILD_WRITTEN_PREFIX: &str = "src-tauri/gen/schemas/";

/// Emits a Cargo rerun trigger for every tracked file, so that a warm build observes
/// [`local_source_dirty`] again after any tracked change, not only changes in the source
/// trees each build script watches. Without this, editing an unwatched tracked file
/// (under `scripts/` or `docs/`, for example) leaves a cached `SAGASCRIPT_SOURCE_DIRTY=false`.
/// Untracked files are not watched: Cargo cannot watch the repository root without also
/// watching `target/`. A new untracked file is picked up at the next rerun for any other reason.
/// `src-tauri/gen/schemas/` is not watched either, because Tauri's build rewrites it and
/// watching what the build writes would invalidate every build. The trade-off: a hand edit or
/// deletion there (including the tracked `macOS-schema.json`, which is not on the generated-path
/// allow-list) can leave a cached clean identity until something else reruns the script.
/// A tracked file that is missing (deleted but not yet staged) is not watched, because Cargo
/// treats a missing watched path as changed on every build. The deletion itself already reran
/// the script and marked the tree dirty; restoring the file stays dirty until the next rerun
/// (for example from the index update a `git checkout` makes), which errs on the safe side.
pub fn emit_tracked_file_rerun_triggers() {
    let Some(root) = repository_root() else {
        return;
    };
    let Some(output) = git_output(&["ls-files", "-z", "--full-name", "--", ":/"]) else {
        return;
    };
    for path in output.split(|byte| *byte == 0) {
        let Ok(path) = str::from_utf8(path) else {
            continue;
        };
        if path.is_empty() || path.contains(['\n', '\r']) || path.starts_with(BUILD_WRITTEN_PREFIX)
        {
            continue;
        }
        let path = Path::new(&root).join(path);
        // `exists` follows symlinks, so a dangling tracked symlink is skipped as well.
        if !path.exists() {
            continue;
        }
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Local Git dirtiness, excluding only existing generated files in the shared allow-list.
/// `None` means Git could not provide a trustworthy status; callers must not report clean.
pub fn local_source_dirty() -> Option<bool> {
    let output = git_output(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    if output.is_empty() {
        return Some(false);
    }
    let root = repository_root()?;
    Some(!status_is_clean(&output, &root))
}
