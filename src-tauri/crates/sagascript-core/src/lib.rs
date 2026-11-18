//! Sagascript core: everything the transcription pipeline needs and nothing
//! the desktop shell does. Consumed by both the `sagascript-cli` crate
//! (headless CLI) and the Tauri app crate (GUI), which layer their own
//! entry points and integrations on top.

pub mod acceptance;
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
pub mod recording;
pub mod run_context;
pub mod settings;
pub mod sync;
#[doc(hidden)]
pub mod testing;
pub mod timestamp;
pub mod transcription;

/// Test fixtures made of core types (#363); see the module documentation.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
