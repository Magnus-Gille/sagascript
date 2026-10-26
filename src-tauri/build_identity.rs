//! Shared local build-identity handling for the app and CLI build scripts.

use std::{fs, path::Path, process::Command, str};

const GENERATED_PATHS: &str = include_str!("../scripts/build-identity-generated-paths.txt");

fn git_output(args: &[&str]) -> Option<Vec<u8>> {
    Command::new("git")
        .args(args)
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
            let old_path_offset = record_end + 1;
            if old_path_offset >= status.len() {
                return false;
            }
            let Some(old_path_length) =
                status[old_path_offset..].iter().position(|byte| *byte == 0)
            else {
                return false;
            };
            if old_path_length == 0 {
                return false;
            }
            return false;
        }

        let supported_status = code == b"??"
            || (code != b"  " && code.iter().all(|byte| matches!(*byte, b' ' | b'M' | b'A')));
        if !supported_status || !is_allowed_generated_file(repository_root, path) {
            return false;
        }

        offset = record_end + 1;
    }
    true
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
