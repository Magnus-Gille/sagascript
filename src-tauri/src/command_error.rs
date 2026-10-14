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
//! The command domains the GUI matched on message text are converted: dictation
//! and transcription, hotkeys, the dictionary and file transcription. The rest
//! still return `Result<T, String>` until #360 part 2; a `String` converts into
//! [`CommandError::Unclassified`] so a typed command can call them with `?`.
//! The number of `Unclassified` call sites is what part 2 has left to type.

use sagascript_core::error::DictationError;
use sagascript_core::path_scrub::scrub_paths;
use serde::ser::{Serialize, SerializeStruct, Serializer};

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
    /// A dictation, transcription, audio or file error, with its own code.
    #[error(transparent)]
    Dictation(DictationError),
    /// An error that is still a plain string (`unclassified`). Transitional:
    /// each producer gets a code of its own in #360 part 2.
    #[error("{0}")]
    Unclassified(String),
}

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
            Self::Dictation(error) => error.code(),
            Self::Unclassified(_) => "unclassified",
        }
    }

    /// Whether the same action can succeed on a second attempt.
    pub fn retryable(&self) -> bool {
        match self {
            Self::Busy(_) => true,
            Self::Dictation(error) => error.retryable(),
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
