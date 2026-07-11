pub mod model;
pub mod whisper_backend;

#[cfg(target_os = "macos")]
mod metal_preflight;

pub use whisper_backend::{
    FILE_TRANSCRIBE_BEAM, TranscribeOptions, TranscriptSegment, WhisperBackend,
};
