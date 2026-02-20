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

    #[error("File decode error: {0}")]
    FileDecodeError(String),

    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),
}

// Tauri commands need IntoResponse which requires Serialize
// We implement this so DictationError can be returned from commands
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
        let err = DictationError::HotkeyError("conflict".into());
        let debug = format!("{:?}", err);
        assert!(debug.contains("HotkeyError"));
        assert!(debug.contains("conflict"));
    }
}
