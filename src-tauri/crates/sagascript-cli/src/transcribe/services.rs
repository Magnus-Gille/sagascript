//! What a file run reaches outside itself through (#343): the collaborators a
//! test replaces.
//!
//! Each is a trait with one implementation that ships: [`InferenceBackend`]
//! (Whisper), and with diarization [`DiarizationAnalyzer`] (the ONNX models).
//! The batch gets them from the process once and hands them down together,
//! so a step names what it uses rather than threading them one by one.

#[cfg(feature = "diarization")]
use super::diarize::DiarizationAnalyzer;
use super::inference::InferenceBackend;

/// The collaborators of a file run. Cheap to copy: it only borrows.
#[derive(Clone, Copy)]
pub(super) struct Services<'a> {
    /// Loads the model, checks the language and transcribes.
    pub(super) backend: &'a dyn InferenceBackend,
    /// The speaker analysis of a diarized run.
    #[cfg(feature = "diarization")]
    pub(super) analyzer: &'a dyn DiarizationAnalyzer,
}
