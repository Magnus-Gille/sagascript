//! The Whisper backend as a file run uses it (#343): a trait over the calls
//! `transcribe` makes, so a run can be driven in-process by a scripted backend
//! instead of model weights.
//!
//! The same pattern as [`DetectorLoader`](super::language_detection::DetectorLoader):
//! [`InferenceBackend`] is what a run needs, `WhisperBackend` is the one
//! implementation that ships, and a test supplies its own. The run asks for
//! nothing else of the backend; in particular the model files, the loading and
//! the decoding all go through this trait.

use std::time::Instant;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::DiarizationTranscription;
use sagascript_core::transcription::{
    model, ContextProfile, TranscribeOptions, TranscriptSegment, WhisperBackend,
};

use super::language_detection::LanguageProbe;
use super::resolve::model_id_string;
use crate::output::{errln, Out};

/// A Whisper backend: it can load a model, classify the language of audio
/// (through [`LanguageProbe`]) and transcribe it.
///
/// `Sync` because the diarized path overlaps the Whisper half with the
/// diarization analysis on another thread.
pub(super) trait InferenceBackend: LanguageProbe + Sync {
    /// Whether the weights of `model` are installed.
    fn is_downloaded(&self, model: WhisperModel) -> bool;

    /// Makes `model` the resident model, built for `profile`.
    fn load_model(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError>;

    /// Plain file transcription: timestamped segments, with recovery of speech
    /// Whisper's windowing skipped. `on_progress` gets Whisper's percent;
    /// `on_encode_start` is called once, when mel/encoder work begins.
    fn transcribe_segments(
        &self,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        on_progress: Box<dyn FnMut(i32) + Send>,
        on_encode_start: &dyn Fn(),
    ) -> Result<Vec<TranscriptSegment>, DictationError>;

    /// The Whisper half of a diarized run: word timings and the segments that
    /// coverage is judged on.
    #[cfg(feature = "diarization")]
    fn transcribe_for_diarization(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError>;
}

impl InferenceBackend for WhisperBackend {
    fn is_downloaded(&self, model: WhisperModel) -> bool {
        model::is_model_downloaded(model)
    }

    fn load_model(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        self.load_model_with_profile(model, profile)
    }

    fn transcribe_segments(
        &self,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        on_progress: Box<dyn FnMut(i32) + Send>,
        on_encode_start: &dyn Fn(),
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        self.transcribe_sync_with_gap_recovery(
            audio,
            language,
            options,
            on_progress,
            Some(on_encode_start),
        )
    }

    #[cfg(feature = "diarization")]
    fn transcribe_for_diarization(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        self.transcribe_sync_for_diarization_profiled_with_options(audio, language, prompt, options)
    }
}

/// Loads `model` into `backend` once per batch: `loaded` is the batch's
/// memory of it, so a validated cache hit that never needs the weights does
/// not pay for them, and a batch pays once. Returns the seconds spent.
pub(super) fn ensure_model_loaded(
    out: &Out,
    backend: &dyn InferenceBackend,
    model: WhisperModel,
    profile: ContextProfile,
    loaded: &mut bool,
) -> Result<f64, DictationError> {
    if *loaded {
        return Ok(0.0);
    }
    if !backend.is_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }
    errln!(out, "Loading model: {}...", model.display_name());
    let started = Instant::now();
    backend.load_model(model, profile)?;
    *loaded = true;
    Ok(started.elapsed().as_secs_f64())
}
