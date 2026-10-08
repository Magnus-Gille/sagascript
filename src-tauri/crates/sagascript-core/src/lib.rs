//! Sagascript core: everything the transcription pipeline needs and nothing
//! the desktop shell does. Consumed by both the `sagascript-cli` crate
//! (headless CLI) and the Tauri app crate (GUI), which layer their own
//! entry points and integrations on top.

pub mod audio;
pub mod diarization_evaluation;
pub mod diarization_reference;
pub mod diarization_qualification;
pub mod download;
pub mod diarization_report;
pub mod diarization_strata;
pub mod error;
pub mod meeting;
pub mod meeting_media;
pub mod meeting_reprocess;
pub mod meeting_reprocess_matching;
pub mod meeting_reprocess_plan;
pub mod meeting_reprocess_proposal;
pub mod meeting_review;
pub mod settings;
pub mod speaker_hint;
pub mod transcription;

#[cfg(feature = "diarization")]
pub mod diarization;
