use crate::error::DictationError;
use crate::settings::Language;

/// Trait for transcription backends (local whisper-rs and remote OpenAI)
#[async_trait::async_trait]
pub trait TranscriptionBackend: Send + Sync {
    /// Check if the backend is ready to transcribe
    async fn is_ready(&self) -> bool;

    /// Warm up the backend (load model, verify API key, etc.)
    async fn warm_up(&self) -> Result<(), DictationError>;

    /// Transcribe audio samples (16kHz mono f32) to text
    async fn transcribe(&self, audio: &[f32], language: Language) -> Result<String, DictationError>;
}
