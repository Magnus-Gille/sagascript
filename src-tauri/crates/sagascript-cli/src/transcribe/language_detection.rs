//! The language check of a transcription (#343): what the model hears, set
//! against the language the run was asked to use. The check asks for its
//! detectors through [`DetectorLoader`], naming an [`EncoderPolicy`], and
//! [`InstalledDetectors::load`] is the one place that touches the models
//! directory; it loads local files only today. The types make a new policy a
//! deliberate change (a variant, an exhaustive match, a test); they do not
//! stop code inside the loader from fetching something, which review and
//! `language_detection_never_downloads_an_encoder` have to catch.

use sagascript_core::error::DictationError;
use sagascript_core::settings::WhisperModel;
use sagascript_core::transcription::diagnostics::LanguageDetection;
use sagascript_core::transcription::WhisperBackend;

use super::inference::InferenceBackend;

use crate::output::errln;
use sagascript_core::meeting::pipeline::narrate::Narrator;

/// What loading a model for a language check may do about its Core ML
/// encoder (#343). Local mode makes no network request after setup, so the
/// language check asks for [`EncoderPolicy::NoDownload`], the only policy
/// there is: an encoder is used when `download-model` installed it, and
/// nothing in the check fetches one. A policy that fetches one would be a new
/// variant, and then every `match` over this enum, [`InstalledDetectors::load`]
/// and `language_detection_never_downloads_an_encoder` (in
/// `transcribe_tests.rs`) have to change with it, on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EncoderPolicy {
    NoDownload,
}

/// A loaded Whisper context that can name the language of 16 kHz audio.
pub(super) trait LanguageProbe {
    fn detect_language(&self, audio: &[f32]) -> Result<Option<LanguageDetection>, DictationError>;
}

impl LanguageProbe for WhisperBackend {
    fn detect_language(&self, audio: &[f32]) -> Result<Option<LanguageDetection>, DictationError> {
        WhisperBackend::detect_language(self, audio)
    }
}

/// Where the language check gets the extra detectors it needs for a
/// language-optimized model. The only way to obtain a detector is
/// [`DetectorLoader::load`], which takes an [`EncoderPolicy`]. Which models are
/// installed is not asked here: the run has one answer to that, the
/// transcription backend's ([`InferenceBackend::is_downloaded`]).
pub(super) trait DetectorLoader {
    fn load(
        &self,
        model: WhisperModel,
        encoder: EncoderPolicy,
    ) -> Result<Box<dyn LanguageProbe>, DictationError>;
}

/// The detectors of an installed copy: models that `download-model` put in the
/// models directory, loaded as they are.
struct InstalledDetectors;

impl DetectorLoader for InstalledDetectors {
    fn load(
        &self,
        model: WhisperModel,
        encoder: EncoderPolicy,
    ) -> Result<Box<dyn LanguageProbe>, DictationError> {
        // Irrefutable while `NoDownload` is the only policy: adding one is a
        // compile error here until its loading is written.
        let EncoderPolicy::NoDownload = encoder;
        let backend = WhisperBackend::new();
        backend.load_model(model)?;
        Ok(Box::new(backend))
    }
}

pub(super) fn detect_file_language(
    out: &dyn Narrator,
    transcription_backend: &dyn InferenceBackend,
    audio: &[f32],
    transcription_model: WhisperModel,
) -> Result<Option<LanguageDetection>, DictationError> {
    detect_file_language_with(
        out,
        &InstalledDetectors,
        transcription_backend,
        &|candidate| transcription_backend.is_downloaded(candidate),
        audio,
        transcription_model,
    )
}

/// The language check, over its detectors (`detectors`), the transcription
/// backend's own probe and its answer to which models are installed
/// (`is_downloaded`, which picks the neutral and the accurate detector).
pub(super) fn detect_file_language_with(
    out: &dyn Narrator,
    detectors: &dyn DetectorLoader,
    transcription_probe: &dyn LanguageProbe,
    is_downloaded: &dyn Fn(WhisperModel) -> bool,
    audio: &[f32],
    transcription_model: WhisperModel,
) -> Result<Option<LanguageDetection>, DictationError> {
    if transcription_model.is_english_only() {
        return Ok(None);
    }
    if !transcription_model.is_language_optimized() {
        return transcription_probe.detect_language(audio);
    }

    let Some(detection_model) = neutral_language_detection_model(is_downloaded) else {
        errln!(
            out,
            "Warning: language mismatch check skipped because no neutral multilingual model is downloaded."
        );
        return Ok(None);
    };
    errln!(
        out,
        "Checking language with neutral model: {}...",
        detection_model.display_name()
    );
    let detection_backend = detectors.load(detection_model, EncoderPolicy::NoDownload)?;
    let initial_detection = detection_backend.detect_language(audio)?;
    let Some(initial) = initial_detection else {
        return Ok(None);
    };
    if initial.probability >= 0.90 {
        return Ok(Some(initial));
    }

    // Release the cheap detector before loading the larger fallback. The
    // transcription backend is still resident, so keeping both detection
    // contexts alive here would needlessly increase peak memory.
    drop(detection_backend);

    let Some(accurate_model) = accurate_language_detection_model(is_downloaded, detection_model)
    else {
        return Ok(Some(initial));
    };
    errln!(
        out,
        "Language check was uncertain ({} p={:.3}); verifying with {}...",
        initial.language,
        initial.probability,
        accurate_model.display_name()
    );
    let accurate_backend = detectors.load(accurate_model, EncoderPolicy::NoDownload)?;
    accurate_backend.detect_language(audio)
}

pub(super) fn neutral_language_detection_model(
    is_downloaded: impl Fn(WhisperModel) -> bool,
) -> Option<WhisperModel> {
    // Start with a small neutral model so the common, confident case stays
    // cheap. An uncertain result is escalated below.
    [
        WhisperModel::Base,
        WhisperModel::Tiny,
        WhisperModel::Small,
        WhisperModel::Medium,
        WhisperModel::LargeV3TurboQ8,
        WhisperModel::LargeV3Turbo,
    ]
    .into_iter()
    .find(|&candidate| is_downloaded(candidate))
}

pub(super) fn accurate_language_detection_model(
    is_downloaded: impl Fn(WhisperModel) -> bool,
    exclude: WhisperModel,
) -> Option<WhisperModel> {
    // Older Whisper language classifiers can confuse accented English with
    // Swedish. Large v3 Turbo is used only when the cheap detector is below
    // the strong-confidence threshold.
    [WhisperModel::LargeV3TurboQ8, WhisperModel::LargeV3Turbo]
        .into_iter()
        .find(|&candidate| candidate != exclude && is_downloaded(candidate))
}
