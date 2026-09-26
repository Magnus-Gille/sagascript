pub(crate) mod chunking;
pub mod diagnostics;
pub mod glossary;
pub mod glossary_suggestions;
pub mod live_dictation;
pub mod model;
pub mod pianissimo_backend;
pub mod pianissimo_model;
mod postprocess;
pub mod whisper_backend;

#[cfg(target_os = "macos")]
mod metal_preflight;

pub use glossary::{Glossary, GlossaryCorrection, GlossaryEntry};
pub use glossary_suggestions::{
    suggest_glossary_candidates, GlossarySuggestion, GlossarySuggestionKind,
};
pub use live_dictation::LiveDictationBackend;
pub use postprocess::normalize_nonspeech_markers;
pub use whisper_backend::{
    recommended_parallel_chunks, ContextProfile, TranscribeOptions, TranscriptSegment,
    WhisperBackend, FILE_TRANSCRIBE_BEAM, WARM_MODEL_CACHE_BUDGET_MB, WARM_MODEL_CACHE_MAX_MODELS,
};
#[cfg(feature = "diarization")]
pub use whisper_backend::{DiarizationTranscription, DiarizationTranscriptionTimings};
