//! A diarized run of one file (#343): `--diarize` and `--meeting-json`.
//!
//! [`run_diarized`] only delegates. Each step is a function of its own that
//! takes a request type and returns a result type, in this order:
//!
//! 1. [`acquire_source`] (`source`) settles where the audio comes from: a cache
//!    hit, or the decoded file;
//! 2. [`settle_language`] (`prepare`) takes the language evidence from the cache
//!    or loads the model and checks the audio;
//! 3. [`configure`] (`prepare`) fixes the speaker hint and the clustering
//!    settings and starts the performance report;
//! 4. [`obtain_analysis`] (`analysis`) takes the speaker analysis and the
//!    Whisper word timings from the cache or computes both, and writes the cache;
//! 5. [`cluster_speakers`] (`speakers`) groups the analysis into speaker
//!    segments;
//! 6. [`merge_speakers`] (`speakers`) merges the words into the speakers and
//!    builds the diarization report;
//! 7. [`assemble_result`] (`result`) shapes the words into the `--json` object
//!    or the shared meeting document.

use std::time::Instant;

use sagascript_core::error::DictationError;

use super::output::FileTranscription;
use super::plan::{DiarizationPlan, FileRequest};
use super::progress::Progress;
use super::services::Services;
use super::source::{acquire_source, Acquired};
use sagascript_core::meeting::pipeline::narrate::Narrator;

mod analysis;
mod prepare;
mod result;
mod speakers;

use self::analysis::{obtain_analysis, record_analysis, AnalysisRequest, Analyzed};
use self::prepare::{
    configure, refuse_hint_correction, settle_language, ConfigureRequest, Configured,
    LanguageRequest,
};
use self::result::{assemble_result, AssembleRequest};
use self::speakers::{cluster_speakers, merge_speakers, ClusterRequest, MergeRequest};

/// Runs the diarized path for one file.
pub(super) fn run_diarized(
    request: &FileRequest<'_>,
    started: Instant,
    plan: DiarizationPlan,
    services: Services<'_>,
    model_loaded: &mut bool,
    out: &dyn Narrator,
) -> Result<FileTranscription, DictationError> {
    let FileRequest {
        options,
        language,
        control,
        ..
    } = *request;
    let progress = Progress::new(
        out,
        options.progress_json,
        started,
        Some(plan.decoder_progress.clone()),
    );
    let Acquired {
        source,
        cache,
        cache_lookup_seconds,
    } = acquire_source(request, &plan, services, &progress, out)?;
    let duration = source.duration_seconds();
    let coverage_profile = source.coverage_profile();

    let language_evidence = settle_language(
        &LanguageRequest {
            request,
            plan: &plan,
            source: &source,
            services,
        },
        model_loaded,
        &progress,
        out,
    )?;
    refuse_hint_correction(options)?;

    let Configured {
        config,
        speaker_hint,
        mut performance,
    } = configure(
        &ConfigureRequest {
            request,
            plan: &plan,
            language_evidence: &language_evidence,
            decode_resample_seconds: source.decode_resample_seconds(),
            cache_lookup_seconds,
        },
        out,
    )?;

    progress.meeting_phase("analyzing");
    let analyzed = obtain_analysis(
        source,
        &AnalysisRequest {
            services,
            language,
            prompt: plan.decoder_prompt.as_deref(),
            config: &config,
            options: &plan.options,
            coverage_profile: &coverage_profile,
            language_evidence: &language_evidence,
            cache: cache.as_ref(),
            control,
        },
        out,
    )?;
    record_analysis(&mut performance, &analyzed);
    let Analyzed {
        analysis,
        words,
        coverage_segments,
        ..
    } = analyzed;

    let clustered = cluster_speakers(
        &ClusterRequest {
            analysis: &analysis,
            config: &config,
            control,
            progress: &progress,
        },
        out,
    )?;
    performance.diarization_clustering_seconds = clustered.seconds;

    let merged = merge_speakers(
        MergeRequest {
            request,
            plan: &plan,
            analysis: &analysis,
            speakers: &clustered.speaker_segments,
            words,
            coverage_segments: &coverage_segments,
            hint_outcome: clustered.hint_outcome,
            config: &config,
            duration,
            progress: &progress,
        },
        out,
    )?;
    assemble_result(
        AssembleRequest {
            request,
            flags: &plan.flags,
            source_sha256: plan.source_sha256,
            duration,
            coverage_profile: &coverage_profile,
            language_evidence: &language_evidence,
            coverage_segments: &coverage_segments,
            speaker_hint,
            hint_outcome: clustered.hint_outcome,
            merged,
            progress: &progress,
            started,
        },
        performance,
        out,
    )
}
