//! The file pipeline (#343, #374): what a run of one file does, from the audio to the transcript.
//!
//! This is the pipeline the CLI's `transcribe` command runs for every file and the app runs for a
//! meeting import: planning, where the audio comes from, the Whisper backend, the language check,
//! and, with the `diarization` feature, the speaker steps and the meeting document.

pub mod narrate;
