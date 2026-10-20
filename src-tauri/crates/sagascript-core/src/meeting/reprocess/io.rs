//! Reading and writing the JSON documents of selective reprocessing (#342, #374): reviews,
//! transcripts, plans, proposals and correction operations.
//!
//! Reading is bounded: a regular file, opened once, never larger than the caller's limit.
//! Writing never replaces a file ([`fsutil`], ADR 0008): a document is published complete or not
//! at all, and a private staging file that could not be removed afterwards is reported to the
//! caller (a [`Published`] with a leftover, or the error text that names it), never hidden. Every
//! failure is a fixed [`DictationError::FileDecodeError`] text that names no path of the
//! document itself: a proposal holds a transcript, and these messages end up in logs and in the UI.
//!
//! The CLI's `meeting proposal` and `meeting reprocess` commands and the app's commands read and
//! write through this module; the CLI adds its own warning on stderr for a leftover.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use serde::{de::DeserializeOwned, Serialize};

use crate::error::DictationError;
use crate::fsutil::{self, PublishError, PublishErrorKind, Published};
use crate::meeting::reprocess::proposal::{
    MeetingReprocessingProposal, MAX_SERIALIZED_PROPOSAL_BYTES,
};

const MAX_IO_CONTEXT_CHARS: usize = 160;

/// Read and validate one JSON document of at most `limit` bytes. `kind` names it in the errors
/// ("review", "transcript", "proposal", ...).
pub fn read_json<T: DeserializeOwned>(
    path: &Path,
    limit: usize,
    kind: &str,
) -> Result<T, DictationError> {
    let bytes = read_bounded(path, limit, kind)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        DictationError::FileDecodeError(format!(
            "meeting {kind} input is invalid JSON or fails its schema validation"
        ))
    })
}

/// The bytes of the regular file at `path`, never more than `limit`; a file that grows while it is
/// read is cut off at the limit and refused.
fn read_bounded(path: &Path, limit: usize, kind: &str) -> Result<Vec<u8>, DictationError> {
    let metadata = fs::metadata(path)
        .map_err(|error| io_error("meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input must be a regular file"
        )));
    }
    let file =
        File::open(path).map_err(|error| io_error("meeting input could not be opened", error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("opened meeting input could not be inspected", error))?;
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input must be a regular file"
        )));
    }
    if metadata.len() > limit as u64 {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input exceeds its size limit"
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("meeting input could not be read", error))?;
    if bytes.len() > limit {
        return Err(DictationError::FileDecodeError(format!(
            "meeting {kind} input exceeds its size limit"
        )));
    }
    Ok(bytes)
}

/// Publish `value` as the new file `path`: compact JSON and a newline, never replacing anything.
/// A private staging file that could not be removed is in the receipt's `leftover`; the document
/// is complete regardless.
pub fn write_json_new<T: Serialize>(path: &Path, value: &T) -> Result<Published, DictationError> {
    write_atomic_new_with(path, &json_line(value)?, &fsutil::RealFs)
}

/// Publish `bytes` as the new file `path` ([`fsutil::publish_new_with`]) on the given filesystem
/// operations.
pub fn write_atomic_new_with(
    path: &Path,
    bytes: &[u8],
    ops: &dyn fsutil::PublishFs,
) -> Result<Published, DictationError> {
    fsutil::publish_new_with(path, bytes, ops).map_err(|error| publish_error(&error))
}

/// The serialized document: compact JSON and a newline.
pub fn json_line<T: Serialize>(value: &T) -> Result<Vec<u8>, DictationError> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| serialization_error())?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Serialize `value` and stage it privately beside `path` without publishing
/// anything. Serialization and write errors surface here, before a caller
/// publishes any related artifact (#342).
pub fn stage_json_new<T: Serialize>(
    path: &Path,
    value: &T,
) -> Result<fsutil::StagedFile, DictationError> {
    fsutil::StagedFile::create(path, &json_line(value)?).map_err(|error| publish_error(&error))
}

/// Read a validated proposal selected by a caller such as the desktop UI.
pub fn read_proposal(path: &Path) -> Result<MeetingReprocessingProposal, DictationError> {
    read_json(path, MAX_SERIALIZED_PROPOSAL_BYTES, "proposal")
}

/// Fail when anything, even a dangling symlink, exists at `path`: the cheap
/// check before expensive work that ends in publishing a new file. Publication
/// itself never replaces a file either way.
pub fn ensure_output_absent(path: &Path) -> Result<(), DictationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(DictationError::FileDecodeError(
            "meeting output already exists; no existing file was changed".to_string(),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DictationError::FileDecodeError(format!(
            "meeting output could not be inspected: {}",
            error
        ))),
    }
}

/// The error for a publication that did not happen; no existing file was
/// changed, and a staging file that could not be removed is named.
pub fn publish_error(error: &PublishError) -> DictationError {
    let kind_message = match &error.kind {
        PublishErrorKind::AlreadyExists => {
            "meeting output already exists; no existing file was changed".to_string()
        }
        PublishErrorKind::NoFileName => {
            "meeting output could not be created: the destination has no file name".to_string()
        }
        PublishErrorKind::ParentUnavailable(source) => {
            io_context("meeting output folder could not be inspected", source)
        }
        PublishErrorKind::ParentNotDirectory => {
            "meeting output folder must be a directory".to_string()
        }
        PublishErrorKind::Stage(source) => {
            io_context("meeting output could not be created", source)
        }
        PublishErrorKind::NothingStaged => {
            "meeting output could not be published: nothing was staged".to_string()
        }
        PublishErrorKind::Commit(source) => {
            io_context("meeting output could not be published", source)
        }
    };
    match &error.leftover {
        None => DictationError::FileDecodeError(kind_message),
        Some(leftover) => DictationError::FileDecodeError(format!(
            "{kind_message}; its private temporary file {} could not be removed: {}",
            leftover.path.display(),
            truncated(&leftover.error)
        )),
    }
}

fn serialization_error() -> DictationError {
    DictationError::TranscriptionFailed(
        "meeting proposal could not be serialized by its validated contract".into(),
    )
}

fn io_error(action: &str, error: io::Error) -> DictationError {
    DictationError::FileDecodeError(io_context(action, &error))
}

fn io_context(action: &str, error: &io::Error) -> String {
    format!("{action}: {}", truncated(error))
}

fn truncated(error: &io::Error) -> String {
    error
        .to_string()
        .chars()
        .take(MAX_IO_CONTEXT_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FaultyFs;
    use sagascript_testsupport::TempDir;
    use serde_json::json;

    #[test]
    fn read_json_is_bounded_and_says_what_was_wrong_without_the_path() {
        let temp = TempDir::new();
        let path = temp.join("doc.json");
        fs::write(&path, br#"{"a":1}"#).expect("fixture");

        let value: serde_json::Value = read_json(&path, 64, "review").expect("valid");
        assert_eq!(value["a"], 1);

        let error = read_json::<serde_json::Value>(&path, 3, "review").expect_err("too large");
        assert_eq!(
            error.to_string(),
            "File decode error: meeting review input exceeds its size limit"
        );

        fs::write(&path, b"{not json").expect("fixture");
        let error = read_json::<serde_json::Value>(&path, 64, "plan").expect_err("invalid");
        assert!(error
            .to_string()
            .contains("meeting plan input is invalid JSON"));

        let error = read_json::<serde_json::Value>(temp.path(), 64, "plan").expect_err("a folder");
        assert!(error
            .to_string()
            .contains("meeting plan input must be a regular file"));
        let error = read_json::<serde_json::Value>(&temp.join("absent.json"), 64, "plan")
            .expect_err("absent");
        assert!(error.to_string().contains("could not be inspected"));
        assert!(!error
            .to_string()
            .contains(&temp.path().display().to_string()));
    }

    #[test]
    fn write_json_new_publishes_compact_json_and_never_replaces() {
        let temp = TempDir::new();
        let path = temp.join("out.json");
        let published = write_json_new(&path, &json!({"ok": true})).expect("published");
        assert!(published.leftover.is_none());
        assert_eq!(fs::read(&path).expect("read"), b"{\"ok\":true}\n");

        let error = write_json_new(&path, &json!({"ok": false})).expect_err("exists");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(&path).expect("read"), b"{\"ok\":true}\n");
        assert!(ensure_output_absent(&path).is_err());
        assert!(ensure_output_absent(&temp.join("other.json")).is_ok());
    }

    #[test]
    fn a_staging_file_that_cannot_be_removed_is_reported_not_hidden() {
        let temp = TempDir::new();
        let path = temp.join("out.json");
        let published = write_atomic_new_with(&path, b"bytes", &FaultyFs::new().failing_remove())
            .expect("the document is published");
        assert!(published.leftover.is_some());
        assert_eq!(fs::read(&path).expect("read"), b"bytes");
    }

    #[test]
    fn a_staged_document_is_invisible_until_published() {
        let temp = TempDir::new();
        let path = temp.join("out.json");
        let staged = stage_json_new(&path, &json!([1, 2])).expect("staged");
        assert!(!path.exists());
        staged.publish().expect("published");
        assert_eq!(fs::read(&path).expect("read"), b"[1,2]\n");
    }

    #[test]
    fn a_missing_folder_is_a_fixed_message() {
        let temp = TempDir::new();
        let error = write_json_new(&temp.join("missing").join("out.json"), &json!(1))
            .expect_err("no folder");
        assert!(error.to_string().contains("meeting output folder"));
    }
}
