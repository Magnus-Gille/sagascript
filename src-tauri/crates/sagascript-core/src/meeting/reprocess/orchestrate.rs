//! Explicit selective work orchestration shared by CLI and desktop callers.
//!
//! Planning only reads explicitly selected files. Execution returns a proposal,
//! never replaces an active review, and never upgrades a selective cache miss
//! to full inference. The backend boundary makes skipped expensive work testable.
//!
//! # Publication of the outputs (#342, ADR 0008)
//!
//! A run produces up to two new files: the proposal and, for a full
//! recomputation that asked for one, a reusable cache. Neither is visible at
//! its destination until the run is validated:
//!
//! 1. The cache is written privately beside its destination while inference
//!    runs ([`StagedFile::reserve`]); the proposal stays in memory.
//! 2. After every check (source, cache and plan revisions, cancellation) the
//!    result is a [`StagedReprocessing`]. Dropping it, or any error, removes
//!    the staged cache: nothing was published and the same command can run
//!    again.
//! 3. [`StagedReprocessing::stage_proposal`] serializes and stages the
//!    proposal, so serialization and disk-full errors also come before the
//!    first publication. The caller may run its own last checks.
//! 4. [`PendingPublication::commit`] publishes the cache, then the proposal.
//!    The proposal is the completion marker. Only a failure of the last step
//!    leaves a partial result: [`CommitError::CacheOnly`], a complete cache
//!    and no proposal, never the reverse. A selective run from that cache
//!    recovers the proposal without recomputation.
//!
//! Every publication is no-replace ([`crate::fsutil`]); no existing
//! file is overwritten or deleted. Callers whose proposal stays in memory (the
//! desktop app) use [`StagedReprocessing::finish`], which publishes the cache.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

use crate::diarization::cache::{
    self as diarization_cache, AnalysisIdentity, CacheIdentity, CacheLookup, DiarizationCache,
};
use crate::diarization::report::{AsrSegmentEvidence, DecoderEvidence};
use crate::diarization::speaker_hint::SpeakerCountHint;
use crate::diarization::{
    self, merge::merge_with_diagnostics, DiarizationAnalysis, DiarizeConfig, TimestampedSegment,
};
use crate::error::DictationError;
use crate::fsutil::{Leftover, PublishFs, RealFs, StagedFile};
use crate::glossary::Glossary;
use crate::hash::sha256_hex;
use crate::meeting::assemble::{
    meeting_transcript_from_plain_segments, prepare_diarized_plain_segments,
};
use crate::meeting::control::{MeetingControl, MeetingPhase};
use crate::meeting::reprocess::io as proposal_io;
use crate::meeting::reprocess::plan::{
    ReprocessingContext, ReprocessingMode, ReprocessingPlan, RequiredWork,
};
use crate::meeting::reprocess::proposal::MeetingReprocessingProposal;
use crate::meeting::review::MeetingReview;
use crate::meeting::source::{stable_file_sha256, verify_meeting_source_unchanged};
use crate::meeting::MeetingTranscript;
use crate::models::{Language, WhisperModel};
use crate::transcription::TranscribeOptions;

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
    decoder: DecoderEvidence,
    exclusive_speech_embeddings: bool,
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
        crate::diarization::model::all_models_downloaded,
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

/// A validated run whose new cache, if one was requested, is still private.
///
/// Nothing exists at the requested cache destination yet; the cache sits in a
/// staging file beside it and is removed if this value is dropped unpublished.
#[derive(Debug)]
pub struct StagedReprocessing {
    pub result: ReprocessingResult,
    cache: Option<StagedFile>,
}

/// The proposal (and cache) staged and ready to publish; see
/// [`PendingPublication::commit`]. Dropping it removes both staging files.
#[derive(Debug)]
pub struct PendingPublication {
    cache: Option<StagedFile>,
    proposal: StagedFile,
    required_work: RequiredWork,
    timings: ReprocessingTimings,
}

/// What a successful [`PendingPublication::commit`] published.
#[derive(Debug)]
pub struct PublishReceipt {
    pub proposal: PathBuf,
    pub cache: Option<PathBuf>,
    /// Private staging files that could not be removed after the publication;
    /// every published file is complete regardless.
    pub leftovers: Vec<Leftover>,
    pub required_work: RequiredWork,
    pub timings: ReprocessingTimings,
}

/// A commit that did not publish everything.
#[derive(Debug)]
pub enum CommitError {
    /// Nothing was published and every staging file was removed (or reported
    /// in the cause). The same command can run again.
    NothingPublished(DictationError),
    /// The cache was published and is complete; the proposal was not.
    CacheOnly {
        cache: PathBuf,
        proposal: PathBuf,
        cause: DictationError,
    },
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NothingPublished(cause) | Self::CacheOnly { cause, .. } => cause.fmt(formatter),
        }
    }
}

impl StagedReprocessing {
    /// Publish the staged cache (if any) and return the result. For callers
    /// whose proposal stays in memory, such as the desktop app: either the
    /// cache is published and the result returned, or nothing is published.
    ///
    /// A staging file that could not be removed after the cache was published
    /// is a warning in the log, not a failure: the cache is complete, and
    /// failing the job would lose the validated proposal and make a retry hit
    /// "cache output already exists".
    pub fn finish(self) -> Result<ReprocessingResult, DictationError> {
        self.finish_with(&RealFs)
    }

    /// [`finish`](Self::finish) on the given filesystem operations.
    pub(crate) fn finish_with(
        self,
        ops: &dyn PublishFs,
    ) -> Result<ReprocessingResult, DictationError> {
        let Self { result, cache } = self;
        if let Some(cache) = cache {
            let destination = cache.destination().to_path_buf();
            let published = cache
                .publish_with(ops)
                .map_err(|error| diarization_cache::publish_error(&destination, &error))?;
            if let Some(leftover) = published.leftover {
                // The path is the user's, so the log carries the error kind alone.
                tracing::warn!(
                    kind = ?leftover.error.kind(),
                    "a private cache staging file could not be removed"
                );
            }
        }
        Ok(result)
    }

    /// Serialize the proposal and stage it beside `output`, without publishing
    /// anything. Errors here (serialization, a missing folder, a full disk)
    /// come before any publication and remove the staged cache.
    pub fn stage_proposal(self, output: &Path) -> Result<PendingPublication, DictationError> {
        let Self { result, cache } = self;
        if let Some(cache) = &cache {
            ensure_distinct_outputs(output, cache.destination())?;
        }
        let proposal = proposal_io::stage_json_new(output, &result.proposal)?;
        Ok(PendingPublication {
            cache,
            proposal,
            required_work: result.required_work,
            timings: result.timings,
        })
    }
}

impl PendingPublication {
    /// Publish the cache, then the proposal; see [`commit_with`](Self::commit_with).
    pub fn commit(self) -> Result<PublishReceipt, CommitError> {
        self.commit_with(&RealFs)
    }

    /// Publish the cache, then the proposal, each without replacing anything.
    ///
    /// The proposal destination is checked once more before the cache is
    /// published, so a file that appeared meanwhile publishes nothing. If the
    /// cache cannot be published, nothing is published. Only when the
    /// proposal cannot be published after the cache was is the result
    /// [`CommitError::CacheOnly`].
    pub fn commit_with(self, ops: &dyn PublishFs) -> Result<PublishReceipt, CommitError> {
        let Self {
            cache,
            proposal,
            required_work,
            timings,
        } = self;
        let proposal_path = proposal.destination().to_path_buf();
        proposal_io::ensure_output_absent(&proposal_path).map_err(CommitError::NothingPublished)?;
        let mut leftovers = Vec::new();
        let mut cache_path = None;
        if let Some(cache) = cache {
            let destination = cache.destination().to_path_buf();
            match cache.publish_with(ops) {
                Ok(published) => leftovers.extend(published.leftover),
                Err(error) => {
                    let cause = diarization_cache::publish_error(&destination, &error);
                    return Err(CommitError::NothingPublished(cause));
                }
            }
            cache_path = Some(destination);
        }
        match proposal.publish_with(ops) {
            Ok(published) => leftovers.extend(published.leftover),
            Err(error) => {
                let cause = proposal_io::publish_error(&error);
                return Err(match cache_path {
                    Some(cache) => CommitError::CacheOnly {
                        cache,
                        proposal: proposal_path,
                        cause,
                    },
                    None => CommitError::NothingPublished(cause),
                });
            }
        }
        Ok(PublishReceipt {
            proposal: proposal_path,
            cache: cache_path,
            leftovers,
            required_work,
            timings,
        })
    }
}

pub trait WorkBackend {
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
        speaker_hint: Option<SpeakerCountHint>,
        cache_output: Option<&Path>,
        control: Option<&MeetingControl<'_>>,
    ) -> Result<MeetingTranscript, DictationError>;
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
        "test-revision",
        || true,
        backend,
    )?
    .finish()
}

/// Testable execution seam. Production callers use the CLI's `execute_reprocessing` or
/// `execute_reprocessing_staged`, which supply the real local-model availability check
/// independently of planning and the native backend. `build_revision` is the source revision of
/// the running build, which the diarization report of a selective run records (core has no build
/// identity of its own).
#[allow(clippy::too_many_arguments)]
pub fn execute_with_model_check(
    plan: &ReprocessingPlan,
    expected_plan_revision: &str,
    input: &ReprocessingInput<'_>,
    cache_output: Option<&Path>,
    control: Option<&MeetingControl<'_>>,
    build_revision: &str,
    models_available: impl FnOnce() -> bool,
    backend: &mut impl WorkBackend,
) -> Result<StagedReprocessing, DictationError> {
    let started = Instant::now();
    plan.validate().map_err(plan_error)?;
    ensure_rediarize_models(plan.mode, models_available)?;
    if cache_output.is_some() && plan.mode != ReprocessingMode::Full {
        return Err(failure(
            "a new cache output requires explicit full recomputation",
        ));
    }
    // The requested cache is written to a private staging path while inference
    // runs and is published only after the checks below; an early return drops
    // the guard and removes it.
    let staged_cache = cache_output.map(reserve_cache).transpose()?;
    let mut prepared = prepare(input, plan.mode, control)?;
    plan.revalidate(&prepared.context, expected_plan_revision)
        .map_err(plan_error)?;
    let mut timings = ReprocessingTimings {
        validation_seconds: started.elapsed().as_secs_f64(),
        ..ReprocessingTimings::default()
    };
    let config = DiarizeConfig {
        threshold: plan.threshold,
        speaker_hint: plan.speaker_hint,
        exclusive_speech_embeddings: prepared.exclusive_speech_embeddings,
        ..DiarizeConfig::default()
    };
    let proposed = match plan.mode {
        ReprocessingMode::Full => {
            checkpoint(control, MeetingPhase::Preparing)?;
            let phase = Instant::now();
            let transcript = backend.full(
                input,
                plan.threshold,
                plan.speaker_hint,
                staged_cache.as_ref().map(StagedFile::staged_path),
                control,
            )?;
            if staged_cache
                .as_ref()
                .is_some_and(|cache| !cache.is_staged())
            {
                return Err(failure(
                    "full recomputation did not produce the requested cache; nothing was published",
                ));
            }
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
            let (speakers, hint_outcome) =
                diarization::cluster_with_outcome(&cached.analysis, &config)?;
            let words = cached
                .transcript
                .into_iter()
                .map(|(start, end, text)| TimestampedSegment { start, end, text })
                .collect::<Vec<_>>();
            let (merged, attributions) = merge_with_diagnostics(&speakers, &words);
            let plain = prepare_diarized_plain_segments(&merged, input.language, input.glossary);
            let duration = cached.coverage_profile.duration_seconds();
            let include_diagnostics = input
                .previous
                .original
                .diarization
                .as_ref()
                .is_some_and(|report| report.diagnostics_included);
            let mut report = cached.analysis.report(
                &speakers,
                &config,
                attributions,
                include_diagnostics,
                prepared.context.source_sha256.clone(),
                duration,
                build_revision.into(),
                env!("CARGO_PKG_VERSION").into(),
            )?;
            report.decoder = Some(prepared.decoder.clone());
            report.speaker_hint_outcome = hint_outcome;
            if include_diagnostics {
                report.asr_segments = cached
                    .coverage_segments
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .map(|segment| AsrSegmentEvidence {
                        start: segment.start,
                        end: segment.end,
                        avg_logprob: segment.avg_logprob,
                        no_speech_prob: Some(segment.no_speech_prob),
                    })
                    .collect();
            }
            let transcript = meeting_transcript_from_plain_segments(
                prepared.context.source_sha256.clone(),
                input.language,
                input.model,
                duration,
                &plain,
            )?
            .with_speaker_hint(plan.speaker_hint)
            .with_speaker_hint_outcome(hint_outcome)
            .with_diarization(report)
            .map_err(|error| failure(&format!("invalid selective acoustic report: {error}")))?;
            timings.clustering_seconds = phase.elapsed().as_secs_f64();
            transcript
        }
    };
    let proposed = proposed.with_speaker_hint(plan.speaker_hint);
    verify_inputs_unchanged(input, &prepared.context, &proposed, control)?;
    let phase = Instant::now();
    let proposal = MeetingReprocessingProposal::new(input.previous.clone(), proposed)
        .map_err(|error| failure(&format!("proposal could not be created: {error}")))?;
    timings.proposal_seconds = phase.elapsed().as_secs_f64();
    check(control)?;
    timings.total_seconds = started.elapsed().as_secs_f64();
    Ok(StagedReprocessing {
        result: ReprocessingResult {
            proposal,
            required_work: plan.required_work.clone(),
            timings,
        },
        cache: staged_cache,
    })
}

/// The last checks before a result may be published: the source and the cache
/// are what the plan was made for, and the run answered for that source.
fn verify_inputs_unchanged(
    input: &ReprocessingInput<'_>,
    context: &ReprocessingContext,
    proposed: &MeetingTranscript,
    control: Option<&MeetingControl<'_>>,
) -> Result<(), DictationError> {
    checkpoint(control, MeetingPhase::Finalizing)?;
    verify_meeting_source_unchanged(input.audio, &context.source_sha256, control)?;
    if let (Some(path), Some(expected)) = (input.cache, context.cache_sha256.as_ref()) {
        if stable_file_sha256(path, control)? != *expected {
            return Err(failure(
                "cache changed during reprocessing; request a new plan",
            ));
        }
    }
    if proposed.source_sha256 != context.source_sha256 {
        return Err(failure("reprocessing returned a different source identity"));
    }
    Ok(())
}

/// Reserve the private staging path for a requested new cache, after checking
/// that nothing exists at its destination yet (so a mistake costs no inference).
fn reserve_cache(destination: &Path) -> Result<StagedFile, DictationError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(DictationError::FileDecodeError(format!(
                "Diarization cache output already exists: {}",
                destination.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DictationError::FileDecodeError(format!(
                "Failed to inspect diarization cache output '{}': {error}",
                destination.display()
            )))
        }
    }
    StagedFile::reserve(destination)
        .map_err(|error| diarization_cache::publish_error(destination, &error))
}

/// The proposal and the cache are two files and must not be one path: the
/// second publication would fail after the first succeeded.
pub fn ensure_distinct_outputs(left: &Path, right: &Path) -> Result<(), DictationError> {
    let left = output_identity(left);
    let right = output_identity(right);
    if left == right {
        return Err(DictationError::SettingsError(
            "meeting proposal and cache outputs must be different paths".to_string(),
        ));
    }
    Ok(())
}

fn output_identity(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let Some(filename) = path.file_name() else {
        return path.to_path_buf();
    };
    fs::canonicalize(parent)
        .map(|parent| parent.join(filename))
        .unwrap_or_else(|_| path.to_path_buf())
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
    let source_sha256 = stable_file_sha256(input.audio, control)?;
    let prompt = input.glossary.decoder_prompt();
    let language = input.language.whisper_code().unwrap_or("auto");
    let model = input.model.id();
    let previous_report = input.previous.original.diarization.as_ref();
    let options = if mode == ReprocessingMode::Full {
        legacy_decoder_options()
    } else {
        let mut options = legacy_decoder_options();
        if let Some(decoder) = previous_report.and_then(|report| report.decoder.as_ref()) {
            options.beam_size = decoder.beam_size;
            options.temperature_fallback = decoder.temperature_fallback;
            options.vad_model_path = decoder.vad_enabled.then(|| {
                crate::transcription::model::vad_model_path()
                    .to_string_lossy()
                    .into_owned()
            });
        }
        options
    };
    let decoder: DecoderEvidence =
        crate::transcription::whisper_backend::effective_diarization_decoder_config(&options)?
            .into();
    if mode != ReprocessingMode::Full
        && previous_report
            .and_then(|report| report.decoder.as_ref())
            .is_some_and(|previous| previous != &decoder)
    {
        return Err(failure(
            "saved decoder provenance is incompatible; explicitly plan full recomputation",
        ));
    }
    let exclusive_speech_embeddings = mode != ReprocessingMode::Full
        && previous_report.is_some_and(|report| report.parameters.exclusive_speech_embeddings);
    // Bind aliases as well as decoder terms: post-inference correction changes
    // must invalidate an already displayed plan too.
    let transcription_context_sha256 =
        hash_json(&(2u32, language, model, input.glossary.render(), &decoder))?;
    let analysis_context_sha256 = hash_json(&(
        2u32,
        AnalysisIdentity::current_with_policy(exclusive_speech_embeddings),
    ))?;
    let (cache_sha256, cache) = if let Some(path) = input.cache {
        let before = stable_file_sha256(path, control)?;
        let identity = CacheIdentity::for_source_sha256_with_options(
            &source_sha256,
            language,
            model,
            prompt.as_deref(),
            options.beam_size,
            options.temperature_fallback,
            options.vad_model_path.is_some(),
        )?;
        let loaded = match mode {
            ReprocessingMode::Recluster => diarization_cache::load_with_analysis_policy(
                path,
                &identity,
                exclusive_speech_embeddings,
            )?,
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
        if stable_file_sha256(path, control)? != before {
            return Err(failure("cache changed while planning; request a new plan"));
        }
        (Some(before), Some(cached))
    } else {
        (None, None)
    };
    verify_meeting_source_unchanged(input.audio, &source_sha256, control)?;
    Ok(Prepared {
        context: ReprocessingContext {
            source_sha256,
            previous_revision: input.previous.revision.clone(),
            transcription_context_sha256,
            analysis_context_sha256,
            cache_sha256,
        },
        cache,
        decoder,
        exclusive_speech_embeddings,
    })
}

fn legacy_decoder_options() -> TranscribeOptions {
    TranscribeOptions {
        prompt: None,
        beam_size: 0,
        temperature_fallback: true,
        vad_model_path: None,
        segment_timestamps: true,
        parallel_chunks: 1,
    }
}

fn hash_json(value: &impl Serialize) -> Result<String, DictationError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| failure("could not serialize reprocessing context"))?;
    Ok(sha256_hex(bytes))
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
#[path = "orchestrate_tests.rs"]
mod tests;
