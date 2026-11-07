//! The Whisper backend as a file run uses it (#343): a trait over the calls
//! `transcribe` makes, so a run can be driven in-process by a scripted backend
//! instead of model weights.
//!
//! The same pattern as [`DetectorLoader`](super::language_detection::DetectorLoader):
//! [`InferenceBackend`] is what a run needs, `WhisperBackend` is the one
//! implementation that ships, and a test supplies its own. The run asks for
//! nothing else of the backend; in particular the model files, the loading and
//! the decoding all go through this trait.

use std::path::PathBuf;
use std::time::Instant;

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::transcription::diagnostics::{LanguageDetection, LanguageRegionDiagnostics};
#[cfg(feature = "diarization")]
use crate::transcription::DiarizationTranscription;
use crate::transcription::{
    model, ContextProfile, TranscribeOptions, TranscribeRequest, TranscriptSegment, WhisperBackend,
};

use super::language_detection::{detect_file_language, LanguageProbe};
use super::language_regions::detect_file_language_regions;
use super::narrate::errln;
use super::narrate::Narrator;
use super::plan::FileRequest;
use super::progress::Progress;
use super::services::Services;

/// A Whisper backend: it can load a model, classify the language of audio
/// (through [`LanguageProbe`]) and transcribe it.
///
/// `Sync` because the diarized path overlaps the Whisper half with the
/// diarization analysis on another thread.
pub trait InferenceBackend: LanguageProbe + Sync {
    /// Whether the weights of `model` are installed.
    fn is_downloaded(&self, model: WhisperModel) -> bool;

    /// Makes `model` the resident model, built for `profile`.
    fn load_model(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError>;

    /// Makes sure the Silero VAD model is on disk, downloading it on first
    /// use, and returns where it is.
    fn ensure_vad_model(&self) -> Result<PathBuf, DictationError>;

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

    fn ensure_vad_model(&self) -> Result<PathBuf, DictationError> {
        let path = model::vad_model_path();
        tokio::runtime::Runtime::new()
            .map_err(|e| DictationError::ModelDownloadFailed(format!("tokio runtime: {e}")))?
            .block_on(model::download_vad_model(|_, _| {}))?;
        Ok(path)
    }

    fn transcribe_segments(
        &self,
        audio: &[f32],
        language: Language,
        options: &TranscribeOptions,
        on_progress: Box<dyn FnMut(i32) + Send>,
        on_encode_start: &dyn Fn(),
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        self.transcribe_with_gap_recovery(
            TranscribeRequest::new(audio, language, options)
                .on_progress(on_progress)
                .on_encode_start(on_encode_start),
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

/// The error for a model whose weights are not installed.
pub fn not_downloaded(model: WhisperModel) -> DictationError {
    DictationError::TranscriptionFailed(format!(
        "Model '{}' is not downloaded. Run: sagascript download-model {}",
        model.display_name(),
        model.id()
    ))
}

/// Loads `model` into `backend` once per batch: `loaded` is the batch's
/// memory of it, so a validated cache hit that never needs the weights does
/// not pay for them, and a batch pays once. Returns the seconds spent.
pub fn ensure_model_loaded(
    out: &dyn Narrator,
    backend: &dyn InferenceBackend,
    model: WhisperModel,
    profile: ContextProfile,
    loaded: &mut bool,
) -> Result<f64, DictationError> {
    if *loaded {
        return Ok(0.0);
    }
    if !backend.is_downloaded(model) {
        return Err(not_downloaded(model));
    }
    errln!(out, "Loading model: {}...", model.display_name());
    let started = Instant::now();
    backend.load_model(model, profile)?;
    *loaded = true;
    Ok(started.elapsed().as_secs_f64())
}

/// What a run learns about the audio's language, and what it cost: the model
/// load, the language check of the whole file and the sampled region check.
/// A diarization cache hit brings its own, from the run that wrote it.
pub struct LanguageEvidence {
    // Only the diarized run's performance report reads the two timings.
    #[cfg_attr(not(feature = "diarization"), allow(dead_code))]
    pub model_load_seconds: f64,
    pub detected_language: Option<LanguageDetection>,
    pub language_regions: Option<LanguageRegionDiagnostics>,
    #[cfg_attr(not(feature = "diarization"), allow(dead_code))]
    pub language_detection_seconds: f64,
}

/// Loads the model (once per batch) and checks the language of `audio`.
/// A check that cannot run is a warning on stderr, not a failure of the file;
/// a cancel after the load, or after the check, ends the run.
pub fn prepare_language(
    request: &FileRequest<'_>,
    services: Services<'_>,
    audio: &[f32],
    profile: ContextProfile,
    model_loaded: &mut bool,
    progress: &Progress,
    out: &dyn Narrator,
) -> Result<LanguageEvidence, DictationError> {
    let FileRequest {
        language, model, ..
    } = *request;
    let Services {
        backend, canceller, ..
    } = services;
    progress.phase("loading", None);
    let model_load_seconds = ensure_model_loaded(out, backend, model, profile, model_loaded)?;
    canceller.check()?;
    progress.phase("language_detection", None);
    let language_detection_started = Instant::now();
    let detected_language = match detect_file_language(out, backend, audio, model) {
        Ok(detection) => detection,
        Err(error) => {
            errln!(
                out,
                "Warning: local language detection was unavailable: {error}"
            );
            None
        }
    };
    let language_regions = match detect_file_language_regions(out, backend, audio, language) {
        Ok(diagnostics) => diagnostics,
        Err(error) => {
            errln!(
                out,
                "Warning: language region detection was unavailable: {error}"
            );
            None
        }
    };
    canceller.check()?;
    Ok(LanguageEvidence {
        model_load_seconds,
        detected_language,
        language_regions,
        language_detection_seconds: language_detection_started.elapsed().as_secs_f64(),
    })
}

/// Says it is checking the Silero VAD model and has `backend` make sure it is
/// on disk; returns where it is.
pub fn verify_vad_model(
    backend: &dyn InferenceBackend,
    out: &dyn Narrator,
) -> Result<PathBuf, DictationError> {
    errln!(out, "Verifying Silero VAD model...");
    backend.ensure_vad_model()
}
