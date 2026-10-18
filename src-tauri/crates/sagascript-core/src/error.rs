use serde::Serialize;
use thiserror::Error;

use crate::path_scrub::scrub_paths;
use crate::settings::SettingsError;

/// Errors that can occur during dictation workflow.
///
/// Every variant has a stable machine-readable [`code`](Self::code); callers
/// across a boundary (the CLI's JSON error object, the app's command errors)
/// match on that and never on the message text (ADR 0006, #360).
#[derive(Error, Debug, Serialize, Clone)]
#[serde(tag = "kind", content = "message")]
pub enum DictationError {
    #[error("Microphone permission is required. Please enable it in System Settings > Privacy & Security > Microphone.")]
    MicrophonePermissionDenied,

    #[error("Accessibility permission is required for automatic paste. Text has been copied to clipboard.")]
    AccessibilityPermissionDenied,

    #[error("Transcription model is not loaded. Please wait for initialization.")]
    ModelNotLoaded,

    #[error("Transcription failed: {0}")]
    TranscriptionFailed(String),

    /// The work was stopped on request (Stop, Ctrl-C, shutdown). Not a failure:
    /// callers end quietly or show a neutral notice.
    #[error("Transcription cancelled.")]
    Cancelled,

    /// A transcription outran its time budget and was aborted. The payload is
    /// the complete user-facing text.
    #[error("{0}")]
    TranscriptionTimedOut(String),

    /// A background task ended abnormally (a join error: the worker panicked
    /// or was dropped). The payload is the complete user-facing text.
    #[error("{0}")]
    TaskFailed(String),

    /// A failure reported by the Pianissimo engine host, with the host's code
    /// and retry hint kept. Build it with [`DictationError::engine`].
    #[error("{0}")]
    Engine(EngineFailure),

    #[error(
        "Transcription engine is busy — a previous transcription may still be running. \
         Please try again in a moment."
    )]
    ModelBusy,

    #[error("No audio was captured. Please try again.")]
    NoAudioCaptured,

    #[error("Audio capture error: {0}")]
    AudioCaptureError(String),

    #[error("Model download failed: {0}")]
    ModelDownloadFailed(String),

    /// A settings problem the CLI reports in its own words (a bad flag, an
    /// unknown key). `settings_error`. A refusal or failure of the settings
    /// operations themselves is [`DictationError::Settings`], with a code of its
    /// own.
    #[error("Settings error: {0}")]
    SettingsError(String),

    /// A settings operation refused a change or failed (a model that does not
    /// fit the language, an invalid shortcut, a settings file that could not be
    /// written). The code is the [`SettingsError`]'s own.
    #[error("Settings error: {0}")]
    Settings(SettingsError),

    #[error("Application launch error: {0}")]
    ApplicationLaunchError(String),

    #[error("Paste error: {0}")]
    PasteError(String),

    #[error("File decode error: {0}")]
    FileDecodeError(String),

    /// The input file does not exist (any more). The payload is the OS error
    /// text, which names no path.
    #[error("File decode error: Failed to open file: {0}")]
    FileNotFound(String),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    #[error("Diarization error: {0}")]
    DiarizationError(String),
}

/// What the engine host reported, kept apart from [`DictationError::Engine`]'s
/// variant so its message can only be built through [`EngineFailure::new`],
/// which removes absolute paths. Fields of an enum variant are public; the
/// private fields of this struct are what make "the text was scrubbed" true for
/// every value, the log line and the UI alike. The raw text is not kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EngineFailure {
    code: String,
    retryable: bool,
    message: String,
}

impl EngineFailure {
    /// `code` is a stable snake_case identifier (`engine_model_missing`, …),
    /// `retryable` says whether trying the same action again can succeed and
    /// `message` is the host's text, which may contain absolute paths.
    pub fn new(code: impl Into<String>, retryable: bool, message: &str) -> Self {
        Self {
            code: code.into(),
            retryable,
            message: scrub_paths(message),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    /// The user-facing text, with absolute paths removed.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for EngineFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl DictationError {
    /// An engine-host failure with its code and retry hint (see
    /// [`EngineFailure::new`]; `message` is scrubbed of absolute paths).
    pub fn engine(code: impl Into<String>, retryable: bool, message: &str) -> Self {
        Self::Engine(EngineFailure::new(code, retryable, message))
    }

    /// The stable machine-readable code: the variant name in snake_case
    /// (`FileDecodeError` is `file_decode_error`), or the engine's own code for
    /// [`DictationError::Engine`]. Codes are never renamed or reused for another
    /// meaning (ADR 0006).
    pub fn code(&self) -> &str {
        match self {
            Self::MicrophonePermissionDenied => "microphone_permission_denied",
            Self::AccessibilityPermissionDenied => "accessibility_permission_denied",
            Self::ModelNotLoaded => "model_not_loaded",
            Self::TranscriptionFailed(_) => "transcription_failed",
            Self::Cancelled => "cancelled",
            Self::TranscriptionTimedOut(_) => "transcription_timed_out",
            Self::TaskFailed(_) => "task_failed",
            Self::Engine(failure) => failure.code(),
            Self::ModelBusy => "model_busy",
            Self::NoAudioCaptured => "no_audio_captured",
            Self::AudioCaptureError(_) => "audio_capture_error",
            Self::ModelDownloadFailed(_) => "model_download_failed",
            Self::SettingsError(_) => "settings_error",
            Self::Settings(error) => error.code(),
            Self::ApplicationLaunchError(_) => "application_launch_error",
            Self::PasteError(_) => "paste_error",
            Self::FileDecodeError(_) => "file_decode_error",
            Self::FileNotFound(_) => "file_not_found",
            Self::UnsupportedFormat(_) => "unsupported_format",
            Self::DiarizationError(_) => "diarization_error",
        }
    }

    /// Whether the user's action may be repeated (record again, run the file
    /// again) and is expected to succeed: the engine or model was busy or not
    /// ready yet, or nothing was captured. It does not mean the same call can
    /// be repeated: the audio of a stopped recording is consumed. A timeout is
    /// not retryable, because the same input under the same budget tends to
    /// time out again (ADR 0006).
    pub fn retryable(&self) -> bool {
        match self {
            Self::ModelBusy | Self::ModelNotLoaded | Self::NoAudioCaptured => true,
            Self::Engine(failure) => failure.retryable(),
            _ => false,
        }
    }
}

impl From<SettingsError> for DictationError {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

// String conversion so callers (e.g. the app crate's Tauri command handlers,
// which need error types convertible to a serializable form) can return
// DictationError without depending on its variants.
impl From<DictationError> for String {
    fn from(err: DictationError) -> String {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_microphone_permission() {
        let err = DictationError::MicrophonePermissionDenied;
        let msg = err.to_string();
        assert!(msg.contains("Microphone permission"));
        assert!(msg.contains("System Settings"));
    }

    #[test]
    fn display_accessibility_permission() {
        let err = DictationError::AccessibilityPermissionDenied;
        assert!(err.to_string().contains("Accessibility permission"));
    }

    #[test]
    fn display_model_not_loaded() {
        let err = DictationError::ModelNotLoaded;
        assert!(err.to_string().contains("not loaded"));
    }

    #[test]
    fn display_with_message() {
        let err = DictationError::TranscriptionFailed("timeout".into());
        assert_eq!(err.to_string(), "Transcription failed: timeout");

        let err = DictationError::AudioCaptureError("no device".into());
        assert_eq!(err.to_string(), "Audio capture error: no device");

        let err = DictationError::ModelDownloadFailed("404".into());
        assert_eq!(err.to_string(), "Model download failed: 404");

        let err = DictationError::ApplicationLaunchError("not installed".into());
        assert_eq!(err.to_string(), "Application launch error: not installed");

        let err = DictationError::FileDecodeError("corrupt".into());
        assert_eq!(err.to_string(), "File decode error: corrupt");

        let err = DictationError::UnsupportedFormat(".xyz".into());
        assert_eq!(err.to_string(), "Unsupported format: .xyz");
    }

    #[test]
    fn from_error_to_string() {
        let err = DictationError::NoAudioCaptured;
        let s: String = err.into();
        assert!(s.contains("No audio was captured"));
    }

    #[test]
    fn serialize_unit_variant() {
        let err = DictationError::MicrophonePermissionDenied;
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "MicrophonePermissionDenied");
    }

    #[test]
    fn model_busy_display_and_serialize() {
        let err = DictationError::ModelBusy;
        let msg = err.to_string();
        assert!(msg.contains("busy"), "message should say busy: {msg}");
        assert!(
            msg.contains("previous transcription"),
            "message should explain a previous transcription may be running: {msg}"
        );
        // Serializes as a bare unit variant so the frontend can switch on `kind`.
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "ModelBusy");
    }

    #[test]
    fn serialize_variant_with_message() {
        let err = DictationError::TranscriptionFailed("model error".into());
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "TranscriptionFailed");
        assert_eq!(json["message"], "model error");
    }

    #[test]
    fn clone_error() {
        let err = DictationError::SettingsError("bad config".into());
        let cloned = err.clone();
        assert_eq!(err.to_string(), cloned.to_string());
    }

    #[test]
    fn debug_format() {
        let err = DictationError::PasteError("conflict".into());
        let debug = format!("{:?}", err);
        assert!(debug.contains("PasteError"));
        assert!(debug.contains("conflict"));
    }

    #[test]
    fn cancelled_is_its_own_error_with_a_stable_code() {
        let err = DictationError::Cancelled;
        assert_eq!(err.code(), "cancelled");
        assert!(!err.retryable());
        assert_eq!(err.to_string(), "Transcription cancelled.");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["kind"], "Cancelled");
    }

    #[test]
    fn engine_error_keeps_its_code_and_retryable_flag_and_scrubs_the_message() {
        let err = DictationError::engine(
            "engine_model_load_failed",
            false,
            "Pianissimo engine: Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: The file couldn't be opened.",
        );
        assert_eq!(err.code(), "engine_model_load_failed");
        assert!(!err.retryable());
        let shown = err.to_string();
        assert!(!shown.contains("/Users/x"), "{shown}");
        assert!(shown.contains("foo.mlmodelc"), "{shown}");
        let debug = format!("{err:?}");
        assert!(
            !debug.contains("/Users/x"),
            "Debug must not leak the path: {debug}"
        );
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("/Users/x"), "{json}");

        let busy = DictationError::engine("engine_busy", true, "queue full");
        assert_eq!(busy.code(), "engine_busy");
        assert!(busy.retryable());
    }

    #[test]
    fn every_variant_has_a_snake_case_code() {
        let table: Vec<(DictationError, &str)> = vec![
            (
                DictationError::MicrophonePermissionDenied,
                "microphone_permission_denied",
            ),
            (
                DictationError::AccessibilityPermissionDenied,
                "accessibility_permission_denied",
            ),
            (DictationError::ModelNotLoaded, "model_not_loaded"),
            (
                DictationError::TranscriptionFailed("x".into()),
                "transcription_failed",
            ),
            (
                DictationError::TranscriptionTimedOut("x".into()),
                "transcription_timed_out",
            ),
            (DictationError::TaskFailed("x".into()), "task_failed"),
            (DictationError::ModelBusy, "model_busy"),
            (DictationError::NoAudioCaptured, "no_audio_captured"),
            (
                DictationError::AudioCaptureError("x".into()),
                "audio_capture_error",
            ),
            (
                DictationError::ModelDownloadFailed("x".into()),
                "model_download_failed",
            ),
            (DictationError::SettingsError("x".into()), "settings_error"),
            (
                DictationError::ApplicationLaunchError("x".into()),
                "application_launch_error",
            ),
            (DictationError::PasteError("x".into()), "paste_error"),
            (
                DictationError::FileDecodeError("x".into()),
                "file_decode_error",
            ),
            (DictationError::FileNotFound("x".into()), "file_not_found"),
            (
                DictationError::UnsupportedFormat("x".into()),
                "unsupported_format",
            ),
            (DictationError::Cancelled, "cancelled"),
        ];
        for (error, code) in table {
            assert_eq!(error.code(), code, "{error:?}");
        }
        assert_eq!(
            DictationError::DiarizationError("x".into()).code(),
            "diarization_error"
        );
    }

    #[test]
    fn retryable_errors_are_the_ones_a_second_attempt_can_fix() {
        assert!(DictationError::ModelBusy.retryable());
        assert!(DictationError::ModelNotLoaded.retryable());
        assert!(DictationError::NoAudioCaptured.retryable());
        // A timeout is not offered as a retry: the same input under the same
        // budget tends to time out again, and a file that always exceeds it
        // would be retried forever (ADR 0006).
        assert!(!DictationError::TranscriptionTimedOut("x".into()).retryable());
        assert!(!DictationError::TranscriptionFailed("x".into()).retryable());
        assert!(!DictationError::FileNotFound("x".into()).retryable());
        assert!(!DictationError::MicrophonePermissionDenied.retryable());
    }

    #[test]
    fn timeouts_and_task_failures_keep_their_text_and_their_kind() {
        let timeout = DictationError::TranscriptionTimedOut(
            "Transcription timed out after 60s (inference aborted)".into(),
        );
        assert_eq!(
            timeout.to_string(),
            "Transcription timed out after 60s (inference aborted)"
        );
        let task = DictationError::TaskFailed("Task join error: panicked".into());
        assert_eq!(task.to_string(), "Task join error: panicked");
        let missing = DictationError::FileNotFound("No such file or directory (os error 2)".into());
        assert_eq!(
            missing.to_string(),
            "File decode error: Failed to open file: No such file or directory (os error 2)"
        );
    }
}
