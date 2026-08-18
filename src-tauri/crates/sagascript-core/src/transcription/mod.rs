pub mod diagnostics;
pub mod glossary;
pub mod model;
mod postprocess;
pub mod whisper_backend;

#[cfg(target_os = "macos")]
mod metal_preflight;

pub use whisper_backend::{
    FILE_TRANSCRIBE_BEAM, TranscribeOptions, TranscriptSegment, WARM_MODEL_CACHE_BUDGET_MB,
    WARM_MODEL_CACHE_MAX_MODELS, WhisperBackend,
};
pub use glossary::{Glossary, GlossaryCorrection, GlossaryEntry};
pub use postprocess::normalize_nonspeech_markers;
