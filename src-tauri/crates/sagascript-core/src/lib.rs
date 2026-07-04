//! Sagascript core: everything the transcription pipeline needs and nothing
//! the desktop shell does. Consumed by both the `sagascript-cli` crate
//! (headless CLI) and the Tauri app crate (GUI), which layer their own
//! entry points and integrations on top.

pub mod audio;
pub mod download;
pub mod error;
pub mod settings;
pub mod transcription;

#[cfg(feature = "diarization")]
pub mod diarization;
