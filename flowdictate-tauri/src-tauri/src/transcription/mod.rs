pub mod backend;
pub mod openai_backend;
pub mod model;
pub mod whisper_backend;

pub use backend::TranscriptionBackend;
pub use openai_backend::OpenAIBackend;
pub use whisper_backend::WhisperBackend;
