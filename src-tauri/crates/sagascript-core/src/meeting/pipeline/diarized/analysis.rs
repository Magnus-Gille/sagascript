//! The analysis step of a diarized run (#343): the speaker analysis and the
//! Whisper word timings, from the cache or computed, and the cache write.

use std::time::Instant;

use crate::diarization::{DiarizationAnalysis, DiarizationTimings, DiarizeConfig};
use crate::error::DictationError;
use crate::models::Language;
use crate::transcription::diagnostics::CoverageProfile;
use crate::transcription::{
    DiarizationTranscription, DiarizationTranscriptionTimings, TranscribeOptions, TranscriptSegment,
};

use super::super::diarize::{
    canonicalize_coverage_segments, run_diarization_analysis, CachePolicy, DiarizationPerformance,
};
use super::super::inference::LanguageEvidence;
use super::super::narrate::errln;
use super::super::narrate::Narrator;
use super::super::services::Services;
use super::super::source::{CacheTarget, Source};
use crate::meeting::control::{meeting_checkpoint, MeetingControl, MeetingPhase};

/// What [`obtain_analysis`] needs besides the source and the output.
pub(super) struct AnalysisRequest<'a> {
    pub(super) services: Services<'a>,
    pub(super) language: Language,
    pub(super) prompt: Option<&'a str>,
    pub(super) config: &'a DiarizeConfig,
    pub(super) options: &'a TranscribeOptions,
    pub(super) coverage_profile: &'a CoverageProfile,
    pub(super) language_evidence: &'a LanguageEvidence,
    pub(super) cache: Option<&'a CacheTarget>,
    pub(super) control: Option<&'a MeetingControl<'a>>,
}

/// The speaker analysis and the Whisper word timings of a file, and what
/// getting them cost.
pub(super) struct Analyzed {
    pub(super) analysis: DiarizationAnalysis,
    /// Word timings: `(start, end, text)`.
    pub(super) words: Vec<(f64, f64, String)>,
    pub(super) coverage_segments: Vec<TranscriptSegment>,
    /// They came from the cache: nothing was computed.
    pub(super) cache_hit: bool,
    pub(super) diarization_timings: DiarizationTimings,
    pub(super) transcription_timings: DiarizationTranscriptionTimings,
    /// Seconds the two halves took together (overlapped where the platform allows).
    pub(super) parallel_analysis_span_seconds: f64,
    pub(super) cache_write_seconds: f64,
}

/// Adds what the analysis step measured to the performance report.
pub(super) fn record_analysis(performance: &mut DiarizationPerformance, analyzed: &Analyzed) {
    let diarization = &analyzed.diarization_timings;
    let transcription = &analyzed.transcription_timings;
    performance.cache_hit = analyzed.cache_hit;
    performance.parallel_analysis_span_seconds = analyzed.parallel_analysis_span_seconds;
    performance.cache_write_seconds = analyzed.cache_write_seconds;
    performance.diarization_model_load_seconds = diarization.model_load_seconds;
    performance.diarization_segmentation_seconds = diarization.segmentation_seconds;
    performance.diarization_segment_extraction_seconds = diarization.segment_extraction_seconds;
    performance.diarization_embeddings_seconds = diarization.embeddings_seconds;
    performance.whisper_inference_seconds = transcription.whisper_inference_seconds;
    performance.word_timestamp_attribution_seconds =
        transcription.word_timestamp_attribution_seconds;
}

/// Takes the analysis from a cache hit, or computes it (the diarization
/// analysis and the Whisper timestamps, overlapped where the platform allows)
/// and writes it to the cache when there is one.
pub(super) fn obtain_analysis(
    source: Source,
    request: &AnalysisRequest<'_>,
    out: &dyn Narrator,
) -> Result<Analyzed, DictationError> {
    let AnalysisRequest {
        services,
        language,
        prompt,
        config,
        options,
        coverage_profile,
        language_evidence,
        cache,
        control,
    } = *request;
    let audio = match source {
        Source::Cached(run) => {
            let run = *run;
            return Ok(Analyzed {
                analysis: run.analysis,
                words: run.transcript,
                coverage_segments: run.coverage_segments,
                cache_hit: true,
                diarization_timings: DiarizationTimings::default(),
                transcription_timings: DiarizationTranscriptionTimings::default(),
                parallel_analysis_span_seconds: 0.0,
                cache_write_seconds: 0.0,
            });
        }
        Source::Decoded(audio) => audio,
    };
    errln!(
        out,
        "Running speaker diarization and Whisper timestamps concurrently..."
    );
    let parallel_started = Instant::now();
    meeting_checkpoint(control, MeetingPhase::Analyzing)?;
    let (analysis, diarization_timings, mut transcription) =
        run_diarization_analysis(&audio.samples, services, language, prompt, config, options)?;
    transcription.coverage_segments = canonicalize_coverage_segments(
        transcription.coverage_segments,
        coverage_profile.duration_seconds(),
    );
    meeting_checkpoint(control, MeetingPhase::Analyzing)?;
    let parallel_analysis_span_seconds = parallel_started.elapsed().as_secs_f64();

    let cache_write_seconds = match cache {
        Some(target) => write_cache(
            &CacheWrite {
                target,
                analysis: &analysis,
                transcription: &transcription,
                coverage_profile,
                language_evidence,
                exclusive_speech_embeddings: config.exclusive_speech_embeddings,
            },
            out,
        )?,
        None => 0.0,
    };

    Ok(Analyzed {
        analysis,
        words: transcription.segments,
        coverage_segments: transcription.coverage_segments,
        cache_hit: false,
        diarization_timings,
        transcription_timings: transcription.timings,
        parallel_analysis_span_seconds,
        cache_write_seconds,
    })
}

/// What [`write_cache`] stores, and where (the target also says under which policy).
struct CacheWrite<'a> {
    target: &'a CacheTarget,
    analysis: &'a DiarizationAnalysis,
    transcription: &'a DiarizationTranscription,
    coverage_profile: &'a CoverageProfile,
    language_evidence: &'a LanguageEvidence,
    exclusive_speech_embeddings: bool,
}

/// Writes a computed analysis to the run's cache; returns the seconds it took.
///
/// `Normal` replaces whatever is there (atomically) and says so. `RefreshNew`
/// never replaces: the path is a private staging path, and the reprocessing
/// caller publishes the cache (and says so) only after validating the whole
/// run (#342).
fn write_cache(step: &CacheWrite<'_>, out: &dyn Narrator) -> Result<f64, DictationError> {
    let CacheWrite {
        target,
        analysis,
        transcription,
        coverage_profile,
        language_evidence,
        exclusive_speech_embeddings,
    } = *step;
    let cache_write_started = Instant::now();
    let cache = crate::diarization::cache::DiarizationCache::new_with_coverage_segments_and_policy(
        target.identity.clone(),
        analysis.clone(),
        transcription.segments.clone(),
        transcription.coverage_segments.clone(),
        coverage_profile.clone(),
        language_evidence.detected_language.clone(),
        language_evidence.language_regions.clone(),
        exclusive_speech_embeddings,
    );
    match target.policy {
        CachePolicy::Normal => {
            crate::diarization::cache::save(&target.path, &cache)?;
            let seconds = cache_write_started.elapsed().as_secs_f64();
            errln!(
                out,
                "Saved reusable diarization cache: {}",
                target.path.display()
            );
            Ok(seconds)
        }
        CachePolicy::RefreshNew => {
            crate::diarization::cache::save_new(&target.path, &cache)?;
            Ok(cache_write_started.elapsed().as_secs_f64())
        }
    }
}
