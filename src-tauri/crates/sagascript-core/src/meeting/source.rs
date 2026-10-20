//! The source recording of a meeting run (#343, #374): the hash that binds a meeting document to
//! the exact bytes it was made from, and the check that those bytes did not change meanwhile.

use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

use crate::error::DictationError;
use crate::hash::StreamingSha256;
use crate::meeting::control::{meeting_check, MeetingControl};

const MAX_MEETING_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The identity of a meeting input at one moment: its length and modification time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeetingFileMetadata {
    len: u64,
    modified: SystemTime,
}

/// What a meeting input's metadata must satisfy to be hashed: a regular file, of a size within
/// the limit, with a modification time. `context` says which metadata it is.
pub fn checked_meeting_metadata(
    metadata: std::fs::Metadata,
    context: &str,
) -> Result<MeetingFileMetadata, DictationError> {
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "Meeting input {context} is not a regular file"
        )));
    }
    if metadata.len() > MAX_MEETING_INPUT_BYTES {
        return Err(DictationError::FileDecodeError(
            "Meeting input exceeds the 2 GiB hashing limit".to_string(),
        ));
    }
    let modified = metadata.modified().map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Meeting input {context} modification time is unavailable: {error}"
        ))
    })?;
    Ok(MeetingFileMetadata {
        len: metadata.len(),
        modified,
    })
}

/// The SHA-256 of the file at `file`, read in chunks with a look at cancellation between them.
///
/// The file must be a regular file of at most 2 GiB, and must be the same file before, during and
/// after the read (length and modification time, by path and by the opened handle): a recording
/// that changes while it is hashed is refused, since a hash of it would describe no recording.
pub fn stable_file_sha256(
    file: &Path,
    control: Option<&MeetingControl<'_>>,
) -> Result<String, DictationError> {
    const CHUNK_SIZE: usize = 64 * 1024;
    let path_before = std::fs::metadata(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let path_before = checked_meeting_metadata(path_before, "path")?;
    let mut reader = std::fs::File::open(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to read meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let handle_before = checked_meeting_metadata(
        reader.metadata().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect opened meeting input '{}': {error}",
                file.display()
            ))
        })?,
        "opened handle",
    )?;
    if path_before != handle_before {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed while opening; refusing output".to_string(),
        ));
    }
    let mut hasher = StreamingSha256::new();
    let mut buffer = [0_u8; CHUNK_SIZE];
    let mut bytes_read = 0_u64;
    {
        let mut limited_reader = (&mut reader).take(MAX_MEETING_INPUT_BYTES + 1);
        loop {
            meeting_check(control)?;
            let count = limited_reader.read(&mut buffer).map_err(|error| {
                DictationError::FileDecodeError(format!(
                    "Failed to read meeting input '{}': {error}",
                    file.display()
                ))
            })?;
            if count == 0 {
                break;
            }
            bytes_read = bytes_read.checked_add(count as u64).ok_or_else(|| {
                DictationError::FileDecodeError(
                    "Meeting input size exceeded safe hashing bounds".to_string(),
                )
            })?;
            hasher.update(&buffer[..count]);
            meeting_check(control)?;
        }
    }
    if bytes_read > MAX_MEETING_INPUT_BYTES {
        return Err(DictationError::FileDecodeError(
            "Meeting input exceeds the 2 GiB hashing limit".to_string(),
        ));
    }
    let handle_after = checked_meeting_metadata(
        reader.metadata().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect opened meeting input '{}': {error}",
                file.display()
            ))
        })?,
        "opened handle",
    )?;
    let path_after = std::fs::metadata(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let path_after = checked_meeting_metadata(path_after, "path")?;
    if bytes_read != path_before.len
        || path_before != handle_before
        || handle_before != handle_after
        || path_before != path_after
    {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed while hashing; refusing output".to_string(),
        ));
    }
    Ok(hasher.finish_hex())
}

/// Fail unless the file still hashes to `source_sha256`: the last check before a result that
/// names the recording as its source is returned.
pub fn verify_meeting_source_unchanged(
    file: &Path,
    source_sha256: &str,
    control: Option<&MeetingControl<'_>>,
) -> Result<(), DictationError> {
    let after = stable_file_sha256(file, control)?;
    if after != source_sha256 {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed during transcription; refusing output".to_string(),
        ));
    }
    Ok(())
}
