//! Offline scoring of diarization output against a reference: the native
//! reference document, the scoring itself, qualification against a frozen split
//! and the per-stratum breakdown. Only the CLI's `diarization` commands use it;
//! it opens no audio, loads no model and reads no settings, and it is compiled
//! in every build (#372).

pub mod evaluation;
pub mod qualification;
pub mod reference;
pub mod strata;
