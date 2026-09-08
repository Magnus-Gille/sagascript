//! Explicit selective work orchestration shared by CLI and desktop callers.
//!
//! Planning only reads explicitly selected files. Execution returns a proposal,
//! never replaces an active review, and never upgrades a selective cache miss
//! to full inference. The backend boundary makes skipped expensive work testable.

use std::path::Path;
use std::time::Instant;

use sagascript_core::diarization::{
    self, merge::merge_with_transcript, DiarizationAnalysis, DiarizeConfig, TimestampedSegment,
};
use sagascript_core::error::DictationError;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::meeting_reprocess_plan::{
    ReprocessingContext, ReprocessingMode, ReprocessingPlan, RequiredWork,
};
use sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal;
use sagascript_core::meeting_review::MeetingReview;
use sagascript_core::settings::{Language, Settings, WhisperModel};
use sagascript_core::transcription::{Glossary, WhisperBackend};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::diarization_cache::{
    self, AnalysisIdentity, CacheIdentity, CacheLookup, DiarizationCache,
};
use crate::transcribe::{self, MeetingControl, MeetingPhase};

pub struct ReprocessingInput<'a> {
    pub audio: &'a Path,
    pub cache: Option<&'a Path>,
    pub previous: &'a MeetingReview,
    pub language: Language,
    pub model: WhisperModel,
    pub glossary: &'a Glossary,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReprocessingTimings {
    pub validation_seconds: f64,
    pub decode_seconds: f64,
    pub analysis_seconds: f64,
    pub clustering_seconds: f64,
    pub full_pipeline_seconds: f64,
    pub proposal_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReprocessingResult {
    pub proposal: MeetingReprocessingProposal,
    pub required_work: RequiredWork,
    pub timings: ReprocessingTimings,
}

struct Prepared {
    context: ReprocessingContext,
    cache: Option<Box<DiarizationCache>>,
}

/// Inspect source and cache provenance without decoding or loading a model.
pub fn plan_reprocessing(
    input: &ReprocessingInput<'_>,
    mode: ReprocessingMode,
    threshold: f32,
    control: Option<&MeetingControl<'_>>,
) -> Result<ReprocessingPlan, DictationError> {
    plan_reprocessing_with_model_check(
        input,
        mode,
        threshold,
        control,
        sagascript_core::diarization::model::all_models_downloaded,
    )
}

/// Testable planning seam. Production callers must use [`plan_reprocessing`],
/// which supplies the real local-model availability check.
fn plan_reprocessing_with_model_check(
    input: &ReprocessingInput<'_>,
    mode: ReprocessingMode,
    threshold: f32,
    control: Option<&MeetingControl<'_>>,
    models_available: impl FnOnce() -> bool,
) -> Result<ReprocessingPlan, DictationError> {
    ensure_rediarize_models(mode, models_available)?;
    let prepared = prepare(input, mode, control)?;
    ReprocessingPlan::new(mode, prepared.context, threshold).map_err(plan_error)
}

/// Execute exactly the validated plan. A new cache output is opt-in and only
/// supported for full recomputation; it never replaces an existing artifact.
#[allow(clippy::too_many_arguments)]
pub fn execute_reprocessing(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    stored: &Settings,
    whisper: &WhisperBackend,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
) -> Result<ReprocessingResult, DictationError> {
    execute_with_model_check(
        plan,
        expected_plan_revision,
        input,
        cache_output,
        control,
        sagascript_core::diarization::model::all_models_downloaded,
        &mut NativeBackend { stored, whisper },
    )
}

trait WorkBackend {
    fn decode(
        &mut self,
        audio: &Path,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<Vec<f32>, DictationError>;
    fn analyze(
        &mut self,
        audio: &[f32],
        config: &DiarizeConfig,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<DiarizationAnalysis, DictationError>;
    fn full(
        &mut self,
        input: &ReprocessingInput<'_>,
        threshold: f32,
        cache_output: Option<&Path>,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError>;
}

struct NativeBackend<'a> {
    stored: &'a Settings,
    whisper: &'a WhisperBackend,
}

impl WorkBackend for NativeBackend<'_> {
    fn decode(
        &mut self,
        audio: &Path,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<Vec<f32>, DictationError> {
        sagascript_core::audio::decoder::decode_audio_file_with_control(audio, &|| check(control))
    }

    fn analyze(
        &mut self,
        audio: &[f32],
        config: &DiarizeConfig,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<DiarizationAnalysis, DictationError> {
        diarization::analyze_with_control(audio, config, &|| check(control))
            .map(|(analysis, _)| analysis)
    }

    fn full(
        &mut self,
        input: &ReprocessingInput<'_>,
        threshold: f32,
        cache_output: Option<&Path>,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError> {
        transcribe::transcribe_meeting_file_full(
            input.audio,
            self.stored,
            input.language,
            input.model,
            input.glossary,
            self.whisper,
            control,
            threshold,
            cache_output,
        )
    }
}

#[cfg(test)]
fn execute_with(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
    backend: &mut impl WorkBackend,
) -> Result<ReprocessingResult, DictationError> {
    execute_with_model_check(
        plan,
        expected_plan_revision,
        input,
        cache_output,
        control,
        || true,
        backend,
    )
}

/// Testable execution seam. Production callers must use
/// [`execute_reprocessing`], which supplies the real local-model availability
/// check independently of planning.
fn execute_with_model_check(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
    models_available: impl FnOnce() -> bool,
    backend: &mut impl WorkBackend,
) -> Result<ReprocessingResult, DictationError> {
    let started = Instant::now();
    plan.validate().map_err(plan_error)?;
    ensure_rediarize_models(plan.mode, models_available)?;
    if cache_output.is_some() && plan.mode != ReprocessingMode::Full {
        return Err(failure(
            "a new cache output requires explicit full recomputation",
        ));
    }
    let mut prepared = prepare(input, plan.mode, control)?;
    plan.revalidate(&prepared.context, expected_plan_revision)
        .map_err(plan_error)?;
    let mut timings = ReprocessingTimings {
        validation_seconds: started.elapsed().as_secs_f64(),
        ..ReprocessingTimings::default()
    };
    let config = DiarizeConfig {
        threshold: plan.threshold,
        ..DiarizeConfig::default()
    };
    let proposed = match plan.mode {
        ReprocessingMode::Full => {
            checkpoint(control, MeetingPhase::Preparing)?;
            let phase = Instant::now();
            let transcript = backend.full(input, plan.threshold, cache_output, control)?;
            timings.full_pipeline_seconds = phase.elapsed().as_secs_f64();
            transcript
        }
        ReprocessingMode::Recluster | ReprocessingMode::Rediarize => {
            let mut cached = prepared
                .cache
                .take()
                .ok_or_else(|| failure("selective work requires a validated cache"))?;
            if plan.mode == ReprocessingMode::Rediarize {
                checkpoint(control, MeetingPhase::Decoding)?;
                let phase = Instant::now();
                let audio = backend.decode(input.audio, control)?;
                timings.decode_seconds = phase.elapsed().as_secs_f64();
                checkpoint(control, MeetingPhase::Analyzing)?;
                let phase = Instant::now();
                cached.analysis = backend.analyze(&audio, &config, control)?;
                timings.analysis_seconds = phase.elapsed().as_secs_f64();
                check(control)?;
            }
            checkpoint(control, MeetingPhase::Clustering)?;
            let phase = Instant::now();
            let speakers = diarization::cluster(&cached.analysis, &config)?;
            let words = cached
                .transcript
                .into_iter()
                .map(|(start, end, text)| TimestampedSegment { start, end, text })
                .collect::<Vec<_>>();
            let merged = merge_with_transcript(&speakers, &words);
            let plain = transcribe::prepare_diarized_plain_segments(
                &merged,
                input.language,
                input.glossary,
            );
            let transcript = transcribe::meeting_transcript_from_plain_segments(
                prepared.context.source_sha256.clone(),
                input.language,
                input.model,
                cached.coverage_profile.duration_seconds(),
                &plain,
            )?;
            timings.clustering_seconds = phase.elapsed().as_secs_f64();
            transcript
        }
    };
    checkpoint(control, MeetingPhase::Finalizing)?;
    transcribe::verify_meeting_source_unchanged(
        input.audio,
        &prepared.context.source_sha256,
        control,
    )?;
    if let (Some(path), Some(expected)) = (input.cache, prepared.context.cache_sha256.as_ref()) {
        if transcribe::stable_file_sha256(path, control)? != *expected {
            return Err(failure(
                "cache changed during reprocessing; request a new plan",
            ));
        }
    }
    if proposed.source_sha256 != prepared.context.source_sha256 {
        return Err(failure("reprocessing returned a different source identity"));
    }
    let phase = Instant::now();
    let proposal = MeetingReprocessingProposal::new(input.previous.clone(), proposed)
        .map_err(|error| failure(&format!("proposal could not be created: {error}")))?;
    timings.proposal_seconds = phase.elapsed().as_secs_f64();
    check(control)?;
    timings.total_seconds = started.elapsed().as_secs_f64();
    Ok(ReprocessingResult {
        proposal,
        required_work: plan.required_work.clone(),
        timings,
    })
}

fn prepare(
    input: &ReprocessingInput<'_>,
    mode: ReprocessingMode,
    control: Option<&MeetingControl<'_>>,
) -> Result<Prepared, DictationError> {
    checkpoint(control, MeetingPhase::Preparing)?;
    input
        .previous
        .validate()
        .map_err(|error| failure(&format!("invalid previous review: {error}")))?;
    if (mode == ReprocessingMode::Full) != input.cache.is_none() {
        return Err(failure(
            "full mode does not read a cache; selective modes require an explicit cache",
        ));
    }
    let source_sha256 = transcribe::stable_file_sha256(input.audio, control)?;
    let prompt = input.glossary.decoder_prompt();
    let language = input.language.whisper_code().unwrap_or("auto");
    let model = transcribe::model_id_string(input.model);
    // Bind aliases as well as decoder terms: post-inference correction changes
    // must invalidate an already displayed plan too.
    let transcription_context_sha256 =
        hash_json(&(1u32, language, model, input.glossary.render()))?;
    let analysis_context_sha256 = hash_json(&(1u32, AnalysisIdentity::current()))?;
    let (cache_sha256, cache) = if let Some(path) = input.cache {
        let before = transcribe::stable_file_sha256(path, control)?;
        let identity =
            CacheIdentity::for_source_sha256(&source_sha256, language, model, prompt.as_deref())?;
        let loaded = match mode {
            ReprocessingMode::Recluster => diarization_cache::load(path, &identity)?,
            ReprocessingMode::Rediarize => {
                diarization_cache::load_for_rediarization(path, &identity)?
            }
            ReprocessingMode::Full => return Err(failure("full mode cannot reuse a cache")),
        };
        let cached = match loaded {
            CacheLookup::Hit(cached) => cached,
            CacheLookup::Miss(reason) => {
                return Err(failure(&format!(
                    "selective cache miss ({reason}); explicitly plan full recomputation if wanted"
                )))
            }
        };
        if transcribe::stable_file_sha256(path, control)? != before {
            return Err(failure("cache changed while planning; request a new plan"));
        }
        (Some(before), Some(cached))
    } else {
        (None, None)
    };
    transcribe::verify_meeting_source_unchanged(input.audio, &source_sha256, control)?;
    Ok(Prepared {
        context: ReprocessingContext {
            source_sha256,
            previous_revision: input.previous.revision.clone(),
            transcription_context_sha256,
            analysis_context_sha256,
            cache_sha256,
        },
        cache,
    })
}

fn hash_json(value: &impl Serialize) -> Result<String, DictationError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| failure("could not serialize reprocessing context"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn check(control: Option<&MeetingControl<'_>>) -> Result<(), DictationError> {
    control.map_or(Ok(()), MeetingControl::check)
}

fn checkpoint(
    control: Option<&MeetingControl<'_>>,
    phase: MeetingPhase,
) -> Result<(), DictationError> {
    check(control)?;
    if let Some(control) = control {
        (control.progress)(phase);
    }
    check(control)
}

fn ensure_rediarize_models(
    mode: ReprocessingMode,
    models_available: impl FnOnce() -> bool,
) -> Result<(), DictationError> {
    if mode == ReprocessingMode::Rediarize && !models_available() {
        return Err(failure(
            "rediarization requires both diarization models; run: sagascript download-model diarization",
        ));
    }
    Ok(())
}

fn plan_error(error: impl std::fmt::Display) -> DictationError {
    failure(&format!("invalid or stale reprocessing plan: {error}"))
}

fn failure(message: &str) -> DictationError {
    DictationError::TranscriptionFailed(message.to_owned())
}

#[cfg(test)]
#[path = "meeting_reprocessing_tests.rs"]
mod tests;
