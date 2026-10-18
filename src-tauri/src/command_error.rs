//! The error a Tauri command rejects with (ADR 0006, #360).
//!
//! A command that returns `Result<T, CommandError>` rejects its `invoke()` with
//! one JSON object, the wire shape the frontend's `BackendError`
//! (src/lib/errors.ts) mirrors:
//!
//! ```json
//! { "code": "model_busy", "message": "Transcription engine is busy …", "retryable": true }
//! ```
//!
//! - `code` is stable snake_case. The frontend branches on it and only on it:
//!   never on `message`. A code is added, never renamed or reused for another
//!   meaning. [`DictationError::code`] names the codes of the workflow errors;
//!   this enum adds the ones only a command can have.
//! - `message` is English text for the user, with absolute paths removed
//!   ([`sagascript_core::path_scrub`]) however the error was built.
//! - `retryable` says whether the user's action may be repeated (record again,
//!   run the file again) and is expected to succeed; not that the same call
//!   may be repeated (ADR 0006).
//!
//! Every command returns `Result<T, CommandError>` (#360 part 2). The codes a
//! command can add to [`DictationError::code`] and [`SettingsError::code`]:
//!
//! | code | meaning |
//! |---|---|
//! | `busy` | Sagascript is working on something else; the user's action can be repeated later (retryable) |
//! | `not_recording`, `cancelled`, `dictionary_changed_elsewhere` | the state a recording or a dictionary save expected is not there |
//! | `hotkey_invalid`, `hotkey_change_failed` | a shortcut that cannot be used, or that the OS refused |
//! | `invalid_input` | the command refused its arguments or the user's choice (a bad id, an empty text, a file that is not allowed) |
//! | `not_found` | the job or item the command names is no longer there |
//! | `unavailable` | the command cannot run in this build, window or state (a feature not built in, a lock that was lost) |
//! | `io_error` | reading or writing a local file or folder failed |
//! | `clipboard_failed` | the system clipboard refused the text |
//! | `export_exists` | the file chosen for an export exists; exports never overwrite |
//! | `launch_failed` | the OS could not open System Settings or a web page |
//! | `accessibility_permission_denied` | the Accessibility permission a setting needs is missing |
//! | `model_download_failed` | a model the command needed could not be downloaded |
//! | `unclassified` | no better code exists; each producer is listed in ADR 0006 |
//!
//! A failure whose text is fixed for the user but that has a cause worth
//! keeping is [`CommandError::because`]: the cause is scrubbed, logged and kept
//! for tests, and never sent to the webview.

use sagascript_core::error::DictationError;
use sagascript_core::path_scrub::scrub_paths;
use sagascript_core::settings::SettingsError;
use serde::ser::{Serialize, SerializeStruct, Serializer};

// TEMPORARY (removed in this PR's last commit): the variants below are built by
// the commits that convert the remaining domains.
#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// Refused because Sagascript is working on something else (`busy`).
    #[error("{0}")]
    Busy(String),
    /// A recording was expected and none is active (`not_recording`).
    #[error("No training recording is active")]
    NotRecording,
    /// Stopped on request (`cancelled`). The one representation of a
    /// cancellation: [`DictationError::Cancelled`] converts into it.
    #[error("Transcription cancelled.")]
    Cancelled,
    /// A dictionary save found the stored source changed since the editor
    /// read it (`dictionary_changed_elsewhere`).
    #[error(
        "Dictionary changed elsewhere: the persisted source no longer matches the editor baseline"
    )]
    DictionaryChangedElsewhere,
    /// A shortcut that cannot be used (`hotkey_invalid`).
    #[error("{0}")]
    HotkeyInvalid(String),
    /// A shortcut change could not be applied: the operating system refused the
    /// registration, a rollback failed or the settings could not be saved
    /// (`hotkey_change_failed`).
    #[error("{0}")]
    HotkeyChangeFailed(String),
    /// A settings operation refused a change or could not read or write the
    /// settings, with the code of its [`SettingsError`] (`invalid_setting_value`,
    /// `model_incompatible`, `dictionary_blocks_language_change`,
    /// `settings_storage_failed`, …).
    #[error(transparent)]
    Settings(SettingsError),
    /// A dictation, transcription, audio or file error, with its own code.
    #[error(transparent)]
    Dictation(DictationError),
    /// The command refused its arguments or the user's choice (`invalid_input`).
    #[error("{0}")]
    InvalidInput(String),
    /// The job or item the command names is no longer there (`not_found`).
    #[error("{0}")]
    NotFound(String),
    /// The command cannot run in this build, window or state (`unavailable`).
    #[error("{0}")]
    Unavailable(String),
    /// Reading or writing a local file or folder failed (`io_error`).
    #[error("{0}")]
    Io(String),
    /// The system clipboard refused the text (`clipboard_failed`).
    #[error("{0}")]
    Clipboard(String),
    /// The file chosen for an export exists already (`export_exists`).
    #[error("{0}")]
    ExportExists(String),
    /// The OS could not open System Settings or a web page (`launch_failed`).
    #[error("{0}")]
    LaunchFailed(String),
    /// A model the command needed could not be downloaded
    /// (`model_download_failed`, the code of
    /// [`DictationError::ModelDownloadFailed`], with this command's text).
    #[error("{0}")]
    DownloadFailed(String),
    /// A setting needs the Accessibility permission and it is missing
    /// (`accessibility_permission_denied`, the code of
    /// [`DictationError::AccessibilityPermissionDenied`], with this command's text).
    #[error("{0}")]
    AccessibilityPermissionDenied(String),
    /// `error`, whose text the user sees, with the cause that produced it. The
    /// cause is scrubbed of absolute paths when it is attached and logged then;
    /// it is never sent to the webview. See [`CommandError::because`].
    #[error("{error}")]
    Caused {
        error: Box<CommandError>,
        cause: String,
    },
    /// An error that is still a plain string (`unclassified`): no better code
    /// exists. Built only through [`CommandError::unclassified`], never by a
    /// bare `String`.
    #[error("{0}")]
    Unclassified(String),
}

#[allow(dead_code)] // TEMPORARY, see the enum
impl CommandError {
    /// The stable wire code.
    pub fn code(&self) -> &str {
        match self {
            Self::Busy(_) => "busy",
            Self::NotRecording => "not_recording",
            Self::Cancelled => "cancelled",
            Self::DictionaryChangedElsewhere => "dictionary_changed_elsewhere",
            Self::HotkeyInvalid(_) => "hotkey_invalid",
            Self::HotkeyChangeFailed(_) => "hotkey_change_failed",
            Self::Settings(error) => error.code(),
            Self::Dictation(error) => error.code(),
            Self::InvalidInput(_) => "invalid_input",
            Self::NotFound(_) => "not_found",
            Self::Unavailable(_) => "unavailable",
            Self::Io(_) => "io_error",
            Self::Clipboard(_) => "clipboard_failed",
            Self::ExportExists(_) => "export_exists",
            Self::LaunchFailed(_) => "launch_failed",
            Self::DownloadFailed(_) => "model_download_failed",
            Self::AccessibilityPermissionDenied(_) => "accessibility_permission_denied",
            Self::Caused { error, .. } => error.code(),
            Self::Unclassified(_) => "unclassified",
        }
    }

    /// An error with no better code (`unclassified`). Prefer a specific
    /// variant; every use is listed in ADR 0006.
    pub fn unclassified(message: impl Into<String>) -> Self {
        Self::Unclassified(message.into())
    }

    /// `self` with the error that caused it. The user sees `self`'s text; the
    /// cause is scrubbed of absolute paths ([`scrub_paths`]), written to the
    /// log at once and kept for [`CommandError::cause`]. Use it where a fixed
    /// message replaces a source error that would otherwise be dropped.
    pub fn because(self, cause: impl std::fmt::Display) -> Self {
        let cause = scrub_paths(&cause.to_string());
        tracing::warn!(code = self.code(), %cause, "command failed");
        Self::Caused {
            error: Box::new(self),
            cause,
        }
    }

    /// A background worker that ended abnormally (a join error). The text is
    /// the complete message; the code is `task_failed`.
    pub fn task_failed(message: impl Into<String>) -> Self {
        Self::Dictation(DictationError::TaskFailed(message.into()))
    }

    /// The scrubbed cause attached by [`CommandError::because`], if any.
    pub fn cause(&self) -> Option<&str> {
        match self {
            Self::Caused { cause, .. } => Some(cause),
            _ => None,
        }
    }

    /// Whether the user's action may be repeated (record again, run the file
    /// again) and is expected to succeed: a busy engine, a model still loading,
    /// nothing captured. Not that the same call may be repeated (a stopped
    /// recording's audio is consumed), and never a timeout (ADR 0006).
    pub fn retryable(&self) -> bool {
        match self {
            Self::Busy(_) => true,
            Self::Dictation(error) => error.retryable(),
            Self::Caused { error, .. } => error.retryable(),
            _ => false,
        }
    }

    /// The text the webview receives: the display text without absolute paths.
    pub fn message(&self) -> String {
        scrub_paths(&self.to_string())
    }
}

impl From<DictationError> for CommandError {
    fn from(error: DictationError) -> Self {
        match error {
            DictationError::Cancelled => Self::Cancelled,
            other => Self::Dictation(other),
        }
    }
}

/// The meeting and evaluation error enums arrive as the [`DictationError`]
/// their `From` impl in core classifies (`meeting_invalid`, `meeting_stale`,
/// `diarization_error`), with the enum's own text.
macro_rules! through_dictation_error {
    ($($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for CommandError {
            fn from(error: $ty) -> Self {
                Self::Dictation(DictationError::from(error))
            }
        }
    )+};
}

through_dictation_error!(
    sagascript_core::meeting::MeetingError,
    sagascript_core::meeting_review::MeetingReviewError,
    sagascript_core::meeting_reprocess_plan::ReprocessingPlanError,
    sagascript_core::meeting_reprocess_proposal::ProposalError,
    sagascript_core::meeting_media::MediaError,
    sagascript_core::diarization_reference::ReferenceError,
    sagascript_core::diarization_evaluation::EvaluationError,
    sagascript_core::diarization_qualification::QualificationError,
    sagascript_core::diarization_strata::StrataError,
    sagascript_core::diarization_report::ReportError,
);

impl From<SettingsError> for CommandError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

// Transitional: removed once the last command is typed (this PR's final commit).
impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::Unclassified(message)
    }
}

impl From<&str> for CommandError {
    fn from(message: &str) -> Self {
        Self::Unclassified(message.to_owned())
    }
}

impl Serialize for CommandError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut wire = serializer.serialize_struct("CommandError", 3)?;
        wire.serialize_field("code", self.code())?;
        wire.serialize_field("message", &self.message())?;
        wire.serialize_field("retryable", &self.retryable())?;
        wire.end()
    }
}

#[cfg(test)]
#[path = "command_error_tests.rs"]
mod tests;
