//! The failures of the recording store (ADR 0006, ADR 0010).
//!
//! Every variant has a stable machine-readable [`code`](RecordingError::code)
//! and a [`retryable`](RecordingError::retryable) hint, the shape the app's
//! `CommandError` and the CLI's JSON error object carry. Phase 1c maps these
//! onto `contracts/transport/command_errors.json`; nothing here is an IPC type.
//!
//! No variant holds a path. Where a failing filesystem call is reported, only
//! what the call was doing (`action`), the [`io::ErrorKind`] and the OS error
//! number are kept, because the OS text of some errors and any path in them
//! must not reach a log or the UI. Text that comes from a library
//! ([`RecordingError::encoder`]) goes through [`scrub_paths`].

use std::io;

use crate::path_scrub::scrub_paths;

/// Why a recording operation did not happen or did not finish.
#[derive(Debug, thiserror::Error)]
pub enum RecordingError {
    /// A filesystem call failed. `action` says which step ("create the
    /// recording folder"); the path is deliberately not kept.
    #[error("could not {action}: {kind:?}{}", os_suffix(.os_code))]
    Io {
        action: &'static str,
        kind: io::ErrorKind,
        os_code: Option<i32>,
    },
    /// The disk (or the user's quota) is full. Free space and try again.
    #[error("the disk is full; could not {action}")]
    DiskFull { action: &'static str },
    /// The operating system refused access.
    #[error("permission was denied; could not {action}")]
    PermissionDenied { action: &'static str },
    /// No free folder name was found after several new ids.
    #[error("no free name for a new recording folder")]
    NameUnavailable,
    /// Another process (or another handle of this one) owns the recording.
    #[error("the recording is in use by another process")]
    Locked,
    /// Recovery was asked for a folder whose manifest does not say `recording`.
    #[error("the recording is not in progress")]
    NotRecording,
    /// `recording.json` is not a manifest this build can read.
    #[error("the recording manifest is not valid: {reason}")]
    ManifestInvalid { reason: &'static str },
    /// The manifest asks for a newer reader (`min_reader_version`).
    #[error("the recording needs a newer version of Sagascript (manifest version {required}, this build reads {supported})")]
    ManifestTooNew { required: u32, supported: u32 },
    /// A sample rate, channel count or bit depth the store cannot write.
    #[error("unsupported audio format: {reason}")]
    UnsupportedFormat { reason: &'static str },
    /// The caller passed something the store cannot accept (a title that is
    /// not a timestamp, an interleaved buffer that is not whole frames).
    #[error("invalid input: {reason}")]
    InvalidInput { reason: &'static str },
    /// A track id the recording does not have.
    #[error("the recording has no such track")]
    UnknownTrack,
    /// A write to a track that already ended.
    #[error("the track has already ended")]
    TrackFinished,
    /// A track file that recovery cannot interpret at all (not FLAC, or not
    /// the format this store writes). It is left exactly as it was.
    #[error("a track file cannot be read as a recording track: {reason}")]
    CorruptTrack { reason: &'static str },
    /// The FLAC encoder library refused a block (a bug or an out-of-range
    /// sample). The text is scrubbed of paths.
    #[error("the audio encoder failed: {message}")]
    Encoder { message: String },
}

fn os_suffix(os_code: &Option<i32>) -> String {
    os_code
        .map(|code| format!(" (os error {code})"))
        .unwrap_or_default()
}

impl RecordingError {
    /// Classify an I/O failure of the step `action`.
    pub fn io(action: &'static str, error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::DiskFull { action },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied { action },
            kind => Self::Io {
                action,
                kind,
                os_code: error.raw_os_error(),
            },
        }
    }

    /// An encoder-library failure; the text is scrubbed of paths.
    pub(crate) fn encoder(error: impl std::fmt::Display) -> Self {
        Self::Encoder {
            message: scrub_paths(&error.to_string()),
        }
    }

    /// The stable code (snake_case, added and never renamed).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "recording_io",
            Self::DiskFull { .. } => "recording_disk_full",
            Self::PermissionDenied { .. } => "recording_permission_denied",
            Self::NameUnavailable => "recording_name_unavailable",
            Self::Locked => "recording_locked",
            Self::NotRecording => "recording_not_recording",
            Self::ManifestInvalid { .. } => "recording_manifest_invalid",
            Self::ManifestTooNew { .. } => "recording_manifest_too_new",
            Self::UnsupportedFormat { .. } => "recording_unsupported_format",
            Self::InvalidInput { .. } => "recording_invalid_input",
            Self::UnknownTrack => "recording_unknown_track",
            Self::TrackFinished => "recording_track_finished",
            Self::CorruptTrack { .. } => "recording_corrupt_track",
            Self::Encoder { .. } => "recording_encoder",
        }
    }

    /// Whether repeating the user's action may succeed (ADR 0006 decision 6):
    /// a full disk once space is freed, a name collision with a new id, a
    /// recording another process still owns.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::DiskFull { .. } | Self::NameUnavailable | Self::Locked
        )
    }

    /// Whether the failure is a full disk (the case §4.3 of the design handles
    /// by cutting the track back to its last complete frame).
    pub fn is_disk_full(&self) -> bool {
        matches!(self, Self::DiskFull { .. })
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
