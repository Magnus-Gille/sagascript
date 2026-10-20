//! The first steps of a diarized run (#343): what the run knows about the
//! audio's language, and the settings the analysis is configured with.

use crate::diarization::speaker_hint::SpeakerCountHint;
use crate::diarization::DiarizeConfig;
use crate::error::DictationError;
use crate::transcription::{model, ContextProfile};

use super::super::diarize::DiarizationPerformance;
use super::super::inference::{prepare_language, verify_vad_model, LanguageEvidence};
use super::super::narrate::errln;
use super::super::narrate::Narrator;
use super::super::plan::{DiarizationPlan, FileOptions, FileRequest};
use super::super::progress::Progress;
use super::super::services::Services;
use super::super::source::Source;
use crate::meeting::control::{meeting_checkpoint, MeetingPhase};

/// What [`settle_language`] needs besides the batch's memory of its model.
pub(super) struct LanguageRequest<'a> {
    pub(super) request: &'a FileRequest<'a>,
    pub(super) plan: &'a DiarizationPlan,
    pub(super) source: &'a Source,
    pub(super) services: Services<'a>,
}

/// Settles what the run knows about the audio's language: a cache hit brings
/// the evidence of the run that wrote it; a decoded file gets the model loaded
/// (once per batch, after the VAD model is verified when VAD is on) and its
/// audio checked. A meeting job is told it is loading the model before and
/// after.
pub(super) fn settle_language(
    step: &LanguageRequest<'_>,
    model_loaded: &mut bool,
    progress: &Progress,
    out: &dyn Narrator,
) -> Result<LanguageEvidence, DictationError> {
    let LanguageRequest {
        request,
        plan,
        source,
        services,
    } = *step;
    let FileRequest { control, .. } = *request;
    let meeting_json = plan.flags.meeting_json;
    if meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    let evidence = match source {
        Source::Cached(run) => LanguageEvidence {
            model_load_seconds: 0.0,
            detected_language: run.detected_language.clone(),
            language_regions: run.language_regions.clone(),
            language_detection_seconds: 0.0,
        },
        Source::Decoded(audio) => {
            if plan.options.vad_model_path.is_some() {
                verify_vad_model(services.backend, out)?;
            }
            prepare_language(
                request,
                services,
                &audio.samples,
                ContextProfile::for_diarization(true),
                model_loaded,
                progress,
                out,
            )?
        }
    };
    if meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    Ok(evidence)
}

/// `--correct-hints` repairs words by their confidence, which diarized output
/// does not have.
pub(super) fn refuse_hint_correction(options: &FileOptions) -> Result<(), DictationError> {
    if options.correct_hints {
        return Err(DictationError::SettingsError(
            "--correct-hints is not available with --diarize because diarized output has no segment confidence"
                .to_string(),
        ));
    }
    Ok(())
}

/// What [`configure`] needs.
pub(super) struct ConfigureRequest<'a> {
    pub(super) request: &'a FileRequest<'a>,
    pub(super) plan: &'a DiarizationPlan,
    pub(super) language_evidence: &'a LanguageEvidence,
    /// Zero for a cache hit, which decoded nothing.
    pub(super) decode_resample_seconds: f64,
    pub(super) cache_lookup_seconds: f64,
}

/// What the analysis, the clustering and the merge are configured with.
pub(super) struct Configured {
    pub(super) config: DiarizeConfig,
    pub(super) speaker_hint: Option<SpeakerCountHint>,
    /// The report so far: what happened before the analysis.
    pub(super) performance: DiarizationPerformance,
}

/// Fixes the speaker-count hint and the clustering settings, starts the
/// performance report, and says on stderr which acceleration and which decoder
/// the run uses.
pub(super) fn configure(
    step: &ConfigureRequest<'_>,
    out: &dyn Narrator,
) -> Result<Configured, DictationError> {
    let ConfigureRequest {
        request,
        plan,
        language_evidence,
        decode_resample_seconds,
        cache_lookup_seconds,
    } = *step;
    let FileRequest { model, .. } = *request;
    let flags = &plan.flags;
    let acceleration = model::acceleration_profile(model);
    let performance = DiarizationPerformance {
        acceleration_backend: acceleration.backend,
        coreml_status: acceleration.coreml_status,
        model_load_seconds: language_evidence.model_load_seconds,
        decode_resample_seconds,
        language_detection_seconds: language_evidence.language_detection_seconds,
        effective_beam_size: plan.options.beam_size,
        effective_temperature_fallback: plan.options.temperature_fallback,
        effective_vad: plan.options.vad_model_path.is_some(),
        cache_lookup_seconds,
        ..DiarizationPerformance::default()
    };
    errln!(
        out,
        "Whisper acceleration: {} (Core ML: {})",
        acceleration.backend,
        acceleration.coreml_status
    );

    let speaker_hint = flags.speaker_count.validated()?;
    let config = DiarizeConfig {
        threshold: flags.threshold,
        speaker_hint,
        exclusive_speech_embeddings: flags.exclusive_speech_embeddings,
        ..DiarizeConfig::default()
    };
    errln!(
        out,
        "Diarization decoder: beam={}, temp_fallback={}, vad={}",
        plan.options.beam_size,
        plan.options.temperature_fallback,
        plan.options.vad_model_path.is_some()
    );
    Ok(Configured {
        config,
        speaker_hint,
        performance,
    })
}
