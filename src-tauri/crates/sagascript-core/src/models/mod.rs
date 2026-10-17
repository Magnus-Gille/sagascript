//! The model catalog: which models exist, which language each serves, where
//! each is downloaded from and what its download must hash to.
//!
//! Pure data and lookups. This module imports neither `settings` nor
//! `transcription`: settings depend on it (a profile names a model), as does
//! the transcription code. What is *downloaded* or *runnable* on this machine
//! is not known here; callers inject it (see `settings::MachineProbe`).
//!
//! The old `sagascript_core::settings::{FileModel, Language, WhisperModel}`
//! paths re-export these types for one release (#372).

mod file_model;
mod language;
mod sources;
mod whisper;

pub use file_model::FileModel;
pub use language::Language;
pub use whisper::{UnknownModelId, WhisperModel};
