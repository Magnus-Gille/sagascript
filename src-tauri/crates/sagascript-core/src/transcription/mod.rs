pub mod model;
mod postprocess;
pub mod whisper_backend;

#[cfg(target_os = "macos")]
mod metal_preflight;

pub use whisper_backend::{
    FILE_TRANSCRIBE_BEAM, TranscribeOptions, TranscriptSegment, WhisperBackend,
};
pub use postprocess::normalize_nonspeech_markers;
