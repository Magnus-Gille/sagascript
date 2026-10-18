//! Sagascript core: everything the transcription pipeline needs and nothing
//! the desktop shell does. Consumed by both the `sagascript-cli` crate
//! (headless CLI) and the Tauri app crate (GUI), which layer their own
//! entry points and integrations on top.

pub mod audio;
pub mod diarization_evaluation;
pub mod diarization_qualification;
pub mod diarization_reference;
pub mod diarization_report;
pub mod diarization_strata;
pub mod download;
pub mod error;
pub mod fsutil;
pub mod glossary;
pub mod machine;
pub mod meeting;
pub mod meeting_reprocess_plan;
pub mod meeting_reprocess_proposal;
pub mod models;
pub mod path_scrub;
pub mod run_context;
pub mod settings;
pub mod speaker_hint;
pub mod sync;
pub mod transcription;

// Old paths of modules that moved into `meeting/` (#372). The CLI and the app still
// name them; each line goes when the last caller imports the new path.
pub use crate::meeting::media as meeting_media;
pub use crate::meeting::reprocess as meeting_reprocess;
pub use crate::meeting::review as meeting_review;

#[cfg(feature = "diarization")]
pub mod diarization;

/// Test fixtures made of core types (#363); see the module documentation.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
