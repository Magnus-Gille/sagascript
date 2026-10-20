//! What a file run reaches outside itself through (#343): the collaborators a
//! test replaces.
//!
//! Each is a trait with one implementation that ships: [`InferenceBackend`]
//! (Whisper), and with diarization [`DiarizationAnalyzer`] (the ONNX models);
//! and the [`Canceller`] the process cancels through (Ctrl-C), which a test
//! builds its own of. The batch gets them from the process once and hands them
//! down together, so a step names what it uses rather than reading a global.

#[cfg(feature = "diarization")]
use super::diarize::DiarizationAnalyzer;
use super::inference::InferenceBackend;
use crate::cancel::Canceller;

/// The collaborators of a file run. Cheap to copy: it only borrows.
#[derive(Clone, Copy)]
pub(super) struct Services<'a> {
    /// Loads the model, checks the language and transcribes.
    pub(super) backend: &'a dyn InferenceBackend,
    /// The speaker analysis of a diarized run.
    #[cfg(feature = "diarization")]
    pub(super) analyzer: &'a dyn DiarizationAnalyzer,
    /// Where a cancel is asked and where the long steps (decoding, the
    /// language check, the batch between files) look for it.
    pub(super) canceller: &'a Canceller,
    /// The source revision of the running build: a diarization report records it, and core has
    /// no build identity of its own.
    pub(super) build_revision: &'a str,
}
