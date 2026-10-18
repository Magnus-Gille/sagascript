//! A diarized run of one file (#343): `--diarize` and `--meeting-json`.
//!
//! [`run_diarized`] only delegates. Each step is a function of its own that
//! takes a request type and returns a result type, in this order:
//!
//! 1. [`acquire_source`] settles where the audio comes from: a cache hit, or
//!    the decoded file;
//! 2. [`settle_language`] takes the language evidence from the cache or loads
//!    the model and checks the audio;
//! 3. [`configure`] fixes the speaker hint and the clustering settings and
//!    starts the performance report;
//! 4. [`obtain_analysis`] takes the speaker analysis and the Whisper word
//!    timings from the cache or computes both, and [`write_cache`] stores them;
//! 5. [`cluster_speakers`] groups the analysis into speaker segments;
//! 6. [`merge_speakers`] merges the words into the speakers and builds the
//!    diarization report;
//! 7. [`assemble_result`] shapes the words into the `--json` object or the
//!    shared meeting document.

use std::collections::HashSet;
use std::time::Instant;

use sagascript_core::diarization::merge::{consolidate, merge_with_diagnostics};
use sagascript_core::diarization::{
    cluster_with_outcome, DiarizationAnalysis, DiarizationTimings, DiarizeConfig, DiarizedSegment,
    SpeakerSegment, TimestampedSegment,
};
use sagascript_core::diarization_report::DiarizationReport;
use sagascript_core::error::DictationError;
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::settings::Language;
use sagascript_core::speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::{
    model, normalize_nonspeech_markers, ContextProfile, DiarizationTranscription,
    DiarizationTranscriptionTimings, Glossary, GlossaryCorrection, TranscribeOptions,
    TranscriptSegment,
};

use super::args::{speaker_hint_from_args, TranscribeArgs};
use super::diarize::{
    canonicalize_coverage_segments, run_diarization_analysis, CachePolicy, DiarizationPerformance,
};
use super::inference::{prepare_language, verify_vad_model, InferenceBackend, LanguageEvidence};
use super::meeting::{
    meeting_checkpoint, meeting_transcript_from_plain_segments, verify_meeting_source_unchanged,
    MeetingControl, MeetingPhase,
};
use super::output::{
    assemble_diarized_plain_text, combined_warnings, emit_warnings,
    prepare_diarized_plain_segments, FileTranscription,
};
use super::plan::{DiarizationPlan, FileRequest};
use super::progress::Progress;
use super::resolve::model_id_string;
use super::services::Services;
use super::source::{acquire_source, Acquired, CacheTarget, Source};
use crate::output::{errln, Out};

/// Runs the diarized path for one file.
pub(super) fn run_diarized(
    request: &FileRequest<'_>,
    started: Instant,
    plan: DiarizationPlan,
    services: Services<'_>,
    model_loaded: &mut bool,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let FileRequest {
        args,
        language,
        control,
        cache_policy,
        ..
    } = *request;
    let progress = Progress::new(
        out,
        args.progress_json,
        started,
        Some(plan.decoder_progress.clone()),
    );
    let Acquired {
        source,
        cache,
        cache_lookup_seconds,
    } = acquire_source(request, &plan, services.analyzer, &progress, out)?;
    let duration = source.duration_seconds();
    let coverage_profile = source.coverage_profile();

    let language_evidence = settle_language(
        &LanguageRequest {
            request,
            plan: &plan,
            source: &source,
            backend: services.backend,
        },
        model_loaded,
        &progress,
        out,
    )?;
    refuse_hint_correction(args)?;

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
            cache_policy,
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

// ---- the language ----------------------------------------------------------------

/// What [`settle_language`] needs besides the batch's memory of its model.
struct LanguageRequest<'a> {
    request: &'a FileRequest<'a>,
    plan: &'a DiarizationPlan,
    source: &'a Source,
    backend: &'a dyn InferenceBackend,
}

/// Settles what the run knows about the audio's language: a cache hit brings
/// the evidence of the run that wrote it; a decoded file gets the model loaded
/// (once per batch, after the VAD model is verified when VAD is on) and its
/// audio checked. A meeting job is told it is loading the model before and
/// after.
fn settle_language(
    step: &LanguageRequest<'_>,
    model_loaded: &mut bool,
    progress: &Progress,
    out: &Out,
) -> Result<LanguageEvidence, DictationError> {
    let LanguageRequest {
        request,
        plan,
        source,
        backend,
    } = *step;
    let FileRequest { args, control, .. } = *request;
    if args.meeting_json {
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
                verify_vad_model(backend, out)?;
            }
            prepare_language(
                request,
                backend,
                &audio.samples,
                ContextProfile::for_diarization(true),
                model_loaded,
                progress,
                out,
            )?
        }
    };
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    Ok(evidence)
}

/// `--correct-hints` repairs words by their confidence, which diarized output
/// does not have.
fn refuse_hint_correction(args: &TranscribeArgs) -> Result<(), DictationError> {
    if args.correct_hints {
        return Err(DictationError::SettingsError(
            "--correct-hints is not available with --diarize because diarized output has no segment confidence"
                .to_string(),
        ));
    }
    Ok(())
}

// ---- the settings ----------------------------------------------------------------

/// What [`configure`] needs.
struct ConfigureRequest<'a> {
    request: &'a FileRequest<'a>,
    plan: &'a DiarizationPlan,
    language_evidence: &'a LanguageEvidence,
    /// Zero for a cache hit, which decoded nothing.
    decode_resample_seconds: f64,
    cache_lookup_seconds: f64,
}

/// What the analysis, the clustering and the merge are configured with.
struct Configured {
    config: DiarizeConfig,
    speaker_hint: Option<SpeakerCountHint>,
    /// The report so far: what happened before the analysis.
    performance: DiarizationPerformance,
}

/// Fixes the speaker-count hint and the clustering settings, starts the
/// performance report, and says on stderr which acceleration and which decoder
/// the run uses.
fn configure(step: &ConfigureRequest<'_>, out: &Out) -> Result<Configured, DictationError> {
    let ConfigureRequest {
        request,
        plan,
        language_evidence,
        decode_resample_seconds,
        cache_lookup_seconds,
    } = *step;
    let FileRequest { args, model, .. } = *request;
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

    let speaker_hint = speaker_hint_from_args(args)?;
    let config = DiarizeConfig {
        threshold: args.diarize_threshold,
        speaker_hint,
        exclusive_speech_embeddings: args.diarize_exclusive_speech_embeddings,
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

// ---- the analysis ----------------------------------------------------------------

/// What [`obtain_analysis`] needs besides the source and the output.
struct AnalysisRequest<'a> {
    services: Services<'a>,
    language: Language,
    prompt: Option<&'a str>,
    config: &'a DiarizeConfig,
    options: &'a TranscribeOptions,
    coverage_profile: &'a CoverageProfile,
    language_evidence: &'a LanguageEvidence,
    cache: Option<&'a CacheTarget<'a>>,
    cache_policy: CachePolicy,
    control: Option<&'a MeetingControl<'a>>,
}

/// The speaker analysis and the Whisper word timings of a file, and what
/// getting them cost.
struct Analyzed {
    analysis: DiarizationAnalysis,
    /// Word timings: `(start, end, text)`.
    words: Vec<(f64, f64, String)>,
    coverage_segments: Vec<TranscriptSegment>,
    /// They came from the cache: nothing was computed.
    cache_hit: bool,
    diarization_timings: DiarizationTimings,
    transcription_timings: DiarizationTranscriptionTimings,
    /// Seconds the two halves took together (overlapped where the platform allows).
    parallel_analysis_span_seconds: f64,
    cache_write_seconds: f64,
}

/// Adds what the analysis step measured to the performance report.
fn record_analysis(performance: &mut DiarizationPerformance, analyzed: &Analyzed) {
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
fn obtain_analysis(
    source: Source,
    request: &AnalysisRequest<'_>,
    out: &Out,
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
        cache_policy,
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
                policy: cache_policy,
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

/// What [`write_cache`] stores, and where, under which policy.
struct CacheWrite<'a> {
    target: &'a CacheTarget<'a>,
    policy: CachePolicy,
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
fn write_cache(step: &CacheWrite<'_>, out: &Out) -> Result<f64, DictationError> {
    let CacheWrite {
        target,
        policy,
        analysis,
        transcription,
        coverage_profile,
        language_evidence,
        exclusive_speech_embeddings,
    } = *step;
    let cache_write_started = Instant::now();
    let cache = crate::diarization_cache::DiarizationCache::new_with_coverage_segments_and_policy(
        target.identity.clone(),
        analysis.clone(),
        transcription.segments.clone(),
        transcription.coverage_segments.clone(),
        coverage_profile.clone(),
        language_evidence.detected_language.clone(),
        language_evidence.language_regions.clone(),
        exclusive_speech_embeddings,
    );
    match policy {
        CachePolicy::Normal => {
            crate::diarization_cache::save(target.path, &cache)?;
            let seconds = cache_write_started.elapsed().as_secs_f64();
            errln!(
                out,
                "Saved reusable diarization cache: {}",
                target.path.display()
            );
            Ok(seconds)
        }
        CachePolicy::RefreshNew => {
            crate::diarization_cache::save_new(target.path, &cache)?;
            Ok(cache_write_started.elapsed().as_secs_f64())
        }
    }
}

// ---- the speakers ----------------------------------------------------------------

/// What [`cluster_speakers`] needs.
struct ClusterRequest<'a> {
    analysis: &'a DiarizationAnalysis,
    config: &'a DiarizeConfig,
    control: Option<&'a MeetingControl<'a>>,
    progress: &'a Progress,
}

/// The speaker segments of an analysis, and how the speaker-count hint came out.
struct Clustered {
    speaker_segments: Vec<SpeakerSegment>,
    hint_outcome: Option<SpeakerHintOutcome>,
    seconds: f64,
}

/// Groups the analysis into speaker segments, applying the speaker-count hint.
fn cluster_speakers(step: &ClusterRequest<'_>, out: &Out) -> Result<Clustered, DictationError> {
    let ClusterRequest {
        analysis,
        config,
        control,
        progress,
    } = *step;
    let clustering_started = Instant::now();
    progress.meeting_phase("clustering");
    meeting_checkpoint(control, MeetingPhase::Clustering)?;
    let (speaker_segments, hint_outcome) = cluster_with_outcome(analysis, config)?;
    meeting_checkpoint(control, MeetingPhase::Clustering)?;
    let seconds = clustering_started.elapsed().as_secs_f64();
    errln!(out, "Found {} speaker segment(s)", speaker_segments.len());
    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        for s in &speaker_segments {
            errln!(out, "DIARSEG\t{:.3}\t{:.3}\t{}", s.start, s.end, s.speaker);
        }
    }
    Ok(Clustered {
        speaker_segments,
        hint_outcome,
        seconds,
    })
}

// ---- the merge -------------------------------------------------------------------

/// What [`merge_speakers`] needs.
struct MergeRequest<'a> {
    request: &'a FileRequest<'a>,
    plan: &'a DiarizationPlan,
    analysis: &'a DiarizationAnalysis,
    speakers: &'a [SpeakerSegment],
    /// Word timings: `(start, end, text)`.
    words: Vec<(f64, f64, String)>,
    coverage_segments: &'a [TranscriptSegment],
    hint_outcome: Option<SpeakerHintOutcome>,
    config: &'a DiarizeConfig,
    duration: f64,
    progress: &'a Progress,
}

/// The words merged into the speakers, with the report that explains it.
struct Merged {
    diarized: Vec<DiarizedSegment>,
    report: DiarizationReport,
    /// When the merge began: the performance report counts its diagnostics
    /// from here to the end of the shaping.
    started: Instant,
}

/// Merges the speakers into the words and builds the diarization report,
/// checking it and the source file one last time.
fn merge_speakers(step: MergeRequest<'_>, out: &Out) -> Result<Merged, DictationError> {
    let MergeRequest {
        request,
        plan,
        analysis,
        speakers,
        words,
        coverage_segments,
        hint_outcome,
        config,
        duration,
        progress,
    } = step;
    let FileRequest {
        args,
        file,
        control,
        ..
    } = *request;
    errln!(out, "Got {} word/segment(s) for merging", words.len());
    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        for (st, en, tx) in &words {
            errln!(out, "WORD\t{:.3}\t{:.3}\t{}", st, en, tx.replace('\t', " "));
        }
    }

    progress.meeting_phase("finalizing");
    let started = Instant::now();
    let transcript: Vec<TimestampedSegment> = words
        .into_iter()
        .map(|(start, end, text)| TimestampedSegment { start, end, text })
        .collect();

    let (diarized, attributions) = merge_with_diagnostics(speakers, &transcript);
    let mut report = analysis.report(
        speakers,
        config,
        attributions,
        args.diarize_diagnostics,
        plan.source_sha256.clone(),
        duration,
        crate::GIT_HASH.into(),
        env!("CARGO_PKG_VERSION").into(),
    )?;
    report.decoder = Some(plan.decoder_evidence.clone());
    report.speaker_hint_outcome = hint_outcome;
    report.asr_segments = coverage_segments
        .iter()
        .filter(|_| args.diarize_diagnostics)
        .map(
            |segment| sagascript_core::diarization_report::AsrSegmentEvidence {
                start: segment.start,
                end: segment.end,
                avg_logprob: segment.avg_logprob,
                no_speech_prob: Some(segment.no_speech_prob),
            },
        )
        .collect();
    report
        .validate()
        .map_err(|error| DictationError::DiarizationError(error.to_string()))?;
    verify_meeting_source_unchanged(file, &plan.source_sha256, control)?;
    Ok(Merged {
        diarized,
        report,
        started,
    })
}

// ---- the result ------------------------------------------------------------------

/// What [`assemble_result`] needs.
struct AssembleRequest<'a> {
    request: &'a FileRequest<'a>,
    source_sha256: String,
    duration: f64,
    coverage_profile: &'a CoverageProfile,
    language_evidence: &'a LanguageEvidence,
    coverage_segments: &'a [TranscriptSegment],
    speaker_hint: Option<SpeakerCountHint>,
    hint_outcome: Option<SpeakerHintOutcome>,
    merged: Merged,
    progress: &'a Progress,
    started: Instant,
}

/// The diarized words as text: what the plain output and the meeting document
/// are made of, and what the `--json` object lists.
struct DiarizedText {
    /// Without no-speech markers, consolidated, glossary applied: the plain
    /// output and the meeting document.
    plain_segments: Vec<DiarizedSegment>,
    /// Consolidated per speaker, glossary applied: `segments` of `--json`.
    segments: Vec<DiarizedSegment>,
    glossary_corrections: Vec<GlossaryCorrection>,
    /// The speakers, in the order they first speak.
    speakers: Vec<String>,
    plain: String,
}

/// Shapes the merged words into text. No output, no clock: the same input
/// gives the same text.
fn shape_text(
    diarized: &[DiarizedSegment],
    language: Language,
    glossary: &Glossary,
) -> DiarizedText {
    let plain_segments = prepare_diarized_plain_segments(diarized, language, glossary);
    let mut segments = consolidate(diarized);
    for segment in &mut segments {
        segment.text = normalize_nonspeech_markers(&segment.text, language);
    }
    let fragments = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>();
    let (corrected_fragments, projected_corrections) = glossary.correct_fragments(&fragments);
    for (segment, text) in segments.iter_mut().zip(corrected_fragments) {
        segment.text = text;
    }
    let glossary_corrections = projected_corrections
        .into_iter()
        .map(|(_, correction)| correction)
        .collect::<Vec<_>>();
    let speakers: Vec<String> = {
        let mut seen = HashSet::new();
        segments
            .iter()
            .map(|segment| segment.speaker.clone())
            .filter(|speaker| seen.insert(speaker.clone()))
            .collect()
    };
    let plain = assemble_diarized_plain_text(&plain_segments);
    DiarizedText {
        plain_segments,
        segments,
        glossary_corrections,
        speakers,
        plain,
    }
}

/// Assembles the result: the words as text and the `--json` object, or, for
/// `--meeting-json`, the shared meeting document in its place.
fn assemble_result(
    step: AssembleRequest<'_>,
    mut performance: DiarizationPerformance,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let AssembleRequest {
        request,
        source_sha256,
        duration,
        coverage_profile,
        language_evidence,
        coverage_segments,
        speaker_hint,
        hint_outcome,
        merged:
            Merged {
                diarized,
                report,
                started: merge_diagnostics_started,
            },
        progress,
        started,
    } = step;
    let FileRequest {
        args,
        file,
        language,
        model,
        glossary,
        control,
        ..
    } = *request;
    let text = shape_text(&diarized, language, glossary);
    let coverage = analyze_coverage_profile(coverage_profile, coverage_segments);
    let mut warnings = combined_warnings(
        &coverage,
        language,
        language_evidence.detected_language.as_ref(),
    );
    if let Some(diagnostics) = &language_evidence.language_regions {
        warnings.extend(diagnostics.warnings.clone());
    }
    emit_warnings(out, &warnings);

    performance.merge_diagnostics_seconds = merge_diagnostics_started.elapsed().as_secs_f64();
    let json_assembly_started = Instant::now();
    let mut json = serde_json::json!({
        "segments": text.segments,
        "speakers": text.speakers,
        "language": language,
        "model": model_id_string(model),
        "file": file.display().to_string(),
        "duration_seconds": duration,
        "coverage_ratio": coverage.coverage_ratio,
        "uncovered_spans": coverage.uncovered_spans,
        "detected_language": language_evidence.detected_language,
        "language_redetection_enabled": language_evidence.language_regions.is_some(),
        "language_regions": language_evidence.language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
        "warnings": warnings,
        "vocabulary_corrections": text.glossary_corrections,
        "diarization": report,
    });
    performance.json_assembly_seconds = json_assembly_started.elapsed().as_secs_f64();
    performance.total_seconds = started.elapsed().as_secs_f64();
    json["performance"] =
        serde_json::to_value(&performance).expect("diarization performance serializes");
    errln!(
        out,
        "Diarized transcription: {:.2}s total ({:.2}x realtime), cache_hit={}",
        performance.total_seconds,
        duration / performance.total_seconds.max(f64::EPSILON),
        performance.cache_hit
    );
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Finalizing)?;
        let meeting = build_meeting_document(
            source_sha256,
            request,
            duration,
            &text.plain_segments,
            speaker_hint,
            hint_outcome,
            report,
        )?;
        let json = serde_json::to_value(&meeting).map_err(|_| {
            DictationError::TranscriptionFailed(
                "Diarized meeting transcript serialization failed".to_string(),
            )
        })?;
        meeting_checkpoint(control, MeetingPhase::Finalizing)?;
        progress.meeting_phase("completed");
        return Ok(FileTranscription {
            json,
            plain: text.plain,
            meeting: Some(meeting),
        });
    }
    progress.meeting_phase("completed");
    Ok(FileTranscription {
        json,
        plain: text.plain,
        meeting: None,
    })
}

/// The shared meeting document of a diarized run: the plain segments, the
/// speaker hint and how it came out, and the diarization report.
fn build_meeting_document(
    source_sha256: String,
    request: &FileRequest<'_>,
    duration: f64,
    plain_segments: &[DiarizedSegment],
    speaker_hint: Option<SpeakerCountHint>,
    hint_outcome: Option<SpeakerHintOutcome>,
    report: DiarizationReport,
) -> Result<MeetingTranscript, DictationError> {
    let FileRequest {
        language, model, ..
    } = *request;
    meeting_transcript_from_plain_segments(
        source_sha256,
        language,
        model,
        duration,
        plain_segments,
    )?
    .with_speaker_hint(speaker_hint)
    .with_speaker_hint_outcome(hint_outcome)
    .with_diarization(report)
    .map_err(|error| DictationError::DiarizationError(error.to_string()))
}
