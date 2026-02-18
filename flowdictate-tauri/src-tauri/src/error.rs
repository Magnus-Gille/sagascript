use serde::Serialize;
use thiserror::Error;

/// Errors that can occur during dictation workflow
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

    #[error("No audio was captured. Please try again.")]
    NoAudioCaptured,

    #[error("OpenAI API key is not configured. Please add it in Settings.")]
    ApiKeyMissing,

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Audio capture error: {0}")]
    AudioCaptureError(String),

    #[error("Model download failed: {0}")]
    ModelDownloadFailed(String),

    #[error("Settings error: {0}")]
    SettingsError(String),

    #[error("Hotkey error: {0}")]
    HotkeyError(String),

    #[error("Paste error: {0}")]
    PasteError(String),
}

// Tauri commands need IntoResponse which requires Serialize
// We implement this so DictationError can be returned from commands
impl From<DictationError> for String {
    fn from(err: DictationError) -> String {
        err.to_string()
    }
}
