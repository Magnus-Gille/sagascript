//! Sagascript core: everything the transcription pipeline needs and nothing
//! the desktop shell does. Consumed by both the `sagascript-cli` crate
//! (headless CLI) and the Tauri app crate (GUI), which layer their own
//! entry points and integrations on top.

pub mod audio;
pub mod diarization;
pub mod download;
pub mod error;
pub mod fsutil;
pub mod glossary;
pub mod hash;
pub mod machine;
pub mod meeting;
pub mod models;
pub mod path_scrub;
pub mod run_context;
pub mod settings;
pub mod sync;
#[doc(hidden)]
pub mod testing;
pub mod transcription;

// Old paths of modules that moved into `meeting/` (#372). Nothing in the workspace
// names them any more (tests/module_layering.rs locks that); they stay for one release
// for callers outside it.
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::meeting::media as meeting_media;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::meeting::reprocess as meeting_reprocess;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::meeting::reprocess::plan as meeting_reprocess_plan;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::meeting::reprocess::proposal as meeting_reprocess_proposal;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::meeting::review as meeting_review;

// Old paths of modules that moved into `diarization/` (#372), same rule.
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::eval::evaluation as diarization_evaluation;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::eval::qualification as diarization_qualification;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::eval::reference as diarization_reference;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::eval::strata as diarization_strata;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::report as diarization_report;
// Shim (#372): kept one release; remove after the next release.
#[doc(hidden)]
pub use crate::diarization::speaker_hint;

/// Test fixtures made of core types (#363); see the module documentation.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
