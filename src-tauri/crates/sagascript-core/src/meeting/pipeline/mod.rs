//! The file pipeline (#343, #374): what a run of one file does, from the audio to the transcript.
//!
//! This is the pipeline the CLI's `transcribe` command runs for every file and the app runs for a
//! meeting import: planning ([`plan`]), where the audio comes from ([`source`]), the Whisper
//! backend and the language check ([`inference`], [`language_detection`], [`language_regions`]),
//! what a file hands back ([`outcome`]) and, with the `diarization` feature, the speaker steps
//! ([`diarize`], [`diarized`]) and the entry points of a meeting job ([`entry`]). [`services`] is
//! what a run reaches outside itself through, [`narrate`] where it says what it does and
//! [`progress`] the `--progress-json` events.
//!
//! The plain pipeline of the CLI (Whisper, no speakers) and its batch, output modes and Pianissimo
//! path stay in the CLI; they use the types and steps here.

#[cfg(feature = "diarization")]
pub mod diarize;
#[cfg(feature = "diarization")]
pub mod diarized;
#[cfg(feature = "diarization")]
pub mod entry;
pub mod inference;
pub mod language_detection;
pub mod language_regions;
pub mod narrate;
pub mod outcome;
pub mod plan;
pub mod progress;
pub mod services;
pub mod source;
