/// Tauri event names emitted from backend to frontend
pub mod event {
    /// App state changed (idle/recording/transcribing/error)
    pub const STATE_CHANGED: &str = "state-changed";
    /// Transcription result ready
    pub const TRANSCRIPTION_RESULT: &str = "transcription-result";
    /// Error occurred
    pub const ERROR: &str = "error";
    /// Model download progress
    pub const MODEL_DOWNLOAD_PROGRESS: &str = "model-download-progress";
    /// Model ready
    pub const MODEL_READY: &str = "model-ready";
}
