pub mod chunk_merge;
pub(crate) mod chunking;
pub mod dev_overrides;
pub mod diagnostics;
pub mod engine_host;
pub mod gap_redecode;
pub mod glossary_suggestions;
pub mod live_dictation;
pub mod model;
mod native_log;
pub mod pianissimo_backend;
pub mod pianissimo_model;
mod postprocess;
pub mod whisper;
// `whisper_backend` was one file (#371); the old path stays for the CLI, the app
// and the tests, which name `transcription::whisper_backend::...`.
pub use whisper as whisper_backend;

#[cfg(target_os = "macos")]
mod metal_preflight;

// The personal dictionary moved to `crate::glossary` (#372) so settings need not
// reach into transcription. Shims for the old paths
// (`transcription::glossary`, `transcription::Glossary`, ...): remove one release
// after the callers import from `sagascript_core::glossary`.
pub use crate::glossary;
pub use crate::glossary::{Glossary, GlossaryCorrection, GlossaryEntry};
pub use glossary_suggestions::{
    suggest_glossary_candidates, GlossarySuggestion, GlossarySuggestionKind,
};
pub use live_dictation::LiveDictationBackend;
pub use postprocess::normalize_nonspeech_markers;
pub use whisper_backend::{
    recommended_parallel_chunks, ContextProfile, DiarizationTranscription,
    DiarizationTranscriptionTimings, TranscribeHooks, TranscribeOptions, TranscribeRequest,
    TranscriptSegment, WhisperBackend, FILE_TRANSCRIBE_BEAM, WARM_MODEL_CACHE_BUDGET_MB,
    WARM_MODEL_CACHE_MAX_MODELS,
};
