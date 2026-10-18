//! A diarized run of one file (#343): `--diarize` and `--meeting-json`.
//!
//! The steps, in order: [`acquire_source`] settles where the audio comes from
//! (a cache hit, or the decoded file); the language evidence comes from the
//! cache or from loading the model and checking the audio; [`obtain_analysis`]
//! takes the speaker analysis and the Whisper word timings from the cache or
//! computes both, and writes the cache; the speakers are clustered; the words
//! are merged into them and the report built; and the result is assembled as
//! the `--json` object or the shared meeting document.

use std::collections::HashSet;
use std::time::Instant;

use sagascript_core::diarization::merge::{consolidate, merge_with_diagnostics};
use sagascript_core::diarization::{
    cluster_with_outcome, DiarizationAnalysis, DiarizationTimings, DiarizeConfig, DiarizedSegment,
    SpeakerSegment, TimestampedSegment,
};
use sagascript_core::diarization_report::DiarizationReport;
use sagascript_core::error::DictationError;
use sagascript_core::speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::{
    model, normalize_nonspeech_markers, ContextProfile, DiarizationTranscriptionTimings,
    TranscribeOptions, TranscriptSegment,
};

use super::args::speaker_hint_from_args;
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
use super::source::{acquire_source, Acquired, CacheTarget, Source};
use crate::output::{errln, Out};

/// What the steps of a diarized run share: the request and its plan, the
/// evidence gathered so far, and the performance report they all add to.
struct DiarizedRun<'a> {
    request: &'a FileRequest<'a>,
    plan: DiarizationPlan,
    progress: Progress,
    out: &'a Out,
    started: Instant,
    duration: f64,
    coverage_profile: CoverageProfile,
    language_evidence: LanguageEvidence,
    config: DiarizeConfig,
    speaker_hint: Option<SpeakerCountHint>,
    performance: DiarizationPerformance,
}

/// Runs the diarized path for one file.
pub(super) fn run_diarized(
    request: &FileRequest<'_>,
    started: Instant,
    plan: DiarizationPlan,
    backend: &dyn InferenceBackend,
    model_loaded: &mut bool,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let FileRequest {
        args,
        language,
        model,
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
    } = acquire_source(request, &plan, &progress, out)?;
    let duration = source.duration_seconds();
    let coverage_profile = source.coverage_profile();

    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    let language_evidence = match &source {
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
                &progress,
                out,
            )?
        }
    };
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }

    if args.correct_hints {
        return Err(DictationError::SettingsError(
            "--correct-hints is not available with --diarize because diarized output has no segment confidence"
                .to_string(),
        ));
    }

    let acceleration = model::acceleration_profile(model);
    let decode_resample_seconds = match &source {
        Source::Cached(_) => 0.0,
        Source::Decoded(audio) => audio.decode_resample_seconds,
    };
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

    let mut run = DiarizedRun {
        request,
        plan,
        progress,
        out,
        started,
        duration,
        coverage_profile,
        language_evidence,
        config,
        speaker_hint,
        performance,
    };
    run.progress.meeting_phase("analyzing");
    let analyzed = obtain_analysis(
        source,
        &AnalysisRequest {
            backend,
            language,
            prompt: run.plan.decoder_prompt.as_deref(),
            config: &run.config,
            options: &run.plan.options,
            coverage_profile: &run.coverage_profile,
            language_evidence: &run.language_evidence,
            cache: cache.as_ref(),
            cache_policy,
            control,
        },
        &mut run.performance,
        out,
    )?;
    run.record_timings(&analyzed);
    let (speaker_segments, hint_outcome) = run.cluster(&analyzed.analysis)?;
    run.merge_and_finish(analyzed, speaker_segments, hint_outcome)
}

impl DiarizedRun<'_> {
    fn record_timings(&mut self, analyzed: &Analyzed) {
        let diarization = &analyzed.diarization_timings;
        let transcription = &analyzed.transcription_timings;
        self.performance.diarization_model_load_seconds = diarization.model_load_seconds;
        self.performance.diarization_segmentation_seconds = diarization.segmentation_seconds;
        self.performance.diarization_segment_extraction_seconds =
            diarization.segment_extraction_seconds;
        self.performance.diarization_embeddings_seconds = diarization.embeddings_seconds;
        self.performance.whisper_inference_seconds = transcription.whisper_inference_seconds;
        self.performance.word_timestamp_attribution_seconds =
            transcription.word_timestamp_attribution_seconds;
    }

    /// Groups the analysis into speaker segments, applying the speaker-count hint.
    fn cluster(
        &mut self,
        analysis: &DiarizationAnalysis,
    ) -> Result<(Vec<SpeakerSegment>, Option<SpeakerHintOutcome>), DictationError> {
        let control = self.request.control;
        let clustering_started = Instant::now();
        self.progress.meeting_phase("clustering");
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        let (speaker_segments, hint_outcome) = cluster_with_outcome(analysis, &self.config)?;
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        self.performance.diarization_clustering_seconds =
            clustering_started.elapsed().as_secs_f64();
        errln!(
            self.out,
            "Found {} speaker segment(s)",
            speaker_segments.len()
        );
        if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
            for s in &speaker_segments {
                errln!(
                    self.out,
                    "DIARSEG\t{:.3}\t{:.3}\t{}",
                    s.start,
                    s.end,
                    s.speaker
                );
            }
        }
        Ok((speaker_segments, hint_outcome))
    }

    /// Merges the speakers into the words and assembles the result.
    fn merge_and_finish(
        self,
        analyzed: Analyzed,
        speaker_segments: Vec<SpeakerSegment>,
        hint_outcome: Option<SpeakerHintOutcome>,
    ) -> Result<FileTranscription, DictationError> {
        let out = self.out;
        let Analyzed {
            analysis,
            raw_segments,
            coverage_segments,
            ..
        } = analyzed;
        errln!(
            out,
            "Got {} word/segment(s) for merging",
            raw_segments.len()
        );
        if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
            for (st, en, tx) in &raw_segments {
                errln!(out, "WORD\t{:.3}\t{:.3}\t{}", st, en, tx.replace('\t', " "));
            }
        }

        self.progress.meeting_phase("finalizing");
        let merge_diagnostics_started = Instant::now();
        let (diarized, report) = self.merge_report(
            &analysis,
            &speaker_segments,
            raw_segments,
            &coverage_segments,
            hint_outcome,
        )?;
        self.assemble(
            &diarized,
            report,
            &coverage_segments,
            hint_outcome,
            merge_diagnostics_started,
        )
    }

    /// Merges the speakers into the words and builds the diarization report,
    /// checking it and the source file one last time.
    fn merge_report(
        &self,
        analysis: &DiarizationAnalysis,
        speaker_segments: &[SpeakerSegment],
        raw_segments: Vec<(f64, f64, String)>,
        coverage_segments: &[TranscriptSegment],
        hint_outcome: Option<SpeakerHintOutcome>,
    ) -> Result<(Vec<DiarizedSegment>, DiarizationReport), DictationError> {
        let FileRequest {
            args,
            file,
            control,
            ..
        } = *self.request;
        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let (diarized, attributions) = merge_with_diagnostics(speaker_segments, &transcript);
        let mut report = analysis.report(
            speaker_segments,
            &self.config,
            attributions,
            args.diarize_diagnostics,
            self.plan.source_sha256.clone(),
            self.duration,
            crate::GIT_HASH.into(),
            env!("CARGO_PKG_VERSION").into(),
        )?;
        report.decoder = Some(self.plan.decoder_evidence.clone());
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
        verify_meeting_source_unchanged(file, &self.plan.source_sha256, control)?;
        Ok((diarized, report))
    }

    /// The words as the result: plain text and the `--json` object, or the
    /// shared meeting document.
    fn assemble(
        mut self,
        diarized: &[DiarizedSegment],
        report: DiarizationReport,
        coverage_segments: &[TranscriptSegment],
        hint_outcome: Option<SpeakerHintOutcome>,
        merge_diagnostics_started: Instant,
    ) -> Result<FileTranscription, DictationError> {
        let FileRequest {
            args,
            file,
            language,
            model,
            glossary,
            control,
            ..
        } = *self.request;
        let out = self.out;
        let plain_segments = prepare_diarized_plain_segments(diarized, language, glossary);
        let mut consolidated = consolidate(diarized);
        for segment in &mut consolidated {
            segment.text = normalize_nonspeech_markers(&segment.text, language);
        }
        let fragments = consolidated
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();
        let (corrected_fragments, projected_corrections) = glossary.correct_fragments(&fragments);
        for (segment, text) in consolidated.iter_mut().zip(corrected_fragments) {
            segment.text = text;
        }
        let glossary_corrections = projected_corrections
            .into_iter()
            .map(|(_, correction)| correction)
            .collect::<Vec<_>>();
        let coverage = analyze_coverage_profile(&self.coverage_profile, coverage_segments);
        let mut warnings = combined_warnings(
            &coverage,
            language,
            self.language_evidence.detected_language.as_ref(),
        );
        if let Some(diagnostics) = &self.language_evidence.language_regions {
            warnings.extend(diagnostics.warnings.clone());
        }
        emit_warnings(out, &warnings);

        let speakers: Vec<String> = {
            let mut seen = HashSet::new();
            consolidated
                .iter()
                .map(|segment| segment.speaker.clone())
                .filter(|speaker| seen.insert(speaker.clone()))
                .collect()
        };
        let plain = assemble_diarized_plain_text(&plain_segments);
        self.performance.merge_diagnostics_seconds =
            merge_diagnostics_started.elapsed().as_secs_f64();
        let json_assembly_started = Instant::now();
        let mut json = serde_json::json!({
            "segments": consolidated,
            "speakers": speakers,
            "language": language,
            "model": model_id_string(model),
            "file": file.display().to_string(),
            "duration_seconds": self.duration,
            "coverage_ratio": coverage.coverage_ratio,
            "uncovered_spans": coverage.uncovered_spans,
            "detected_language": self.language_evidence.detected_language,
            "language_redetection_enabled": self.language_evidence.language_regions.is_some(),
            "language_regions": self.language_evidence.language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
            "warnings": warnings,
            "vocabulary_corrections": glossary_corrections,
            "diarization": report,
        });
        self.performance.json_assembly_seconds = json_assembly_started.elapsed().as_secs_f64();
        self.performance.total_seconds = self.started.elapsed().as_secs_f64();
        json["performance"] =
            serde_json::to_value(&self.performance).expect("diarization performance serializes");
        errln!(
            out,
            "Diarized transcription: {:.2}s total ({:.2}x realtime), cache_hit={}",
            self.performance.total_seconds,
            self.duration / self.performance.total_seconds.max(f64::EPSILON),
            self.performance.cache_hit
        );
        if args.meeting_json {
            meeting_checkpoint(control, MeetingPhase::Finalizing)?;
            let meeting = meeting_transcript_from_plain_segments(
                self.plan.source_sha256,
                language,
                model,
                self.duration,
                &plain_segments,
            )?
            .with_speaker_hint(self.speaker_hint)
            .with_speaker_hint_outcome(hint_outcome)
            .with_diarization(report)
            .map_err(|error| DictationError::DiarizationError(error.to_string()))?;
            let json = serde_json::to_value(&meeting).map_err(|_| {
                DictationError::TranscriptionFailed(
                    "Diarized meeting transcript serialization failed".to_string(),
                )
            })?;
            meeting_checkpoint(control, MeetingPhase::Finalizing)?;
            self.progress.meeting_phase("completed");
            return Ok(FileTranscription {
                json,
                plain,
                meeting: Some(meeting),
            });
        }
        self.progress.meeting_phase("completed");
        Ok(FileTranscription {
            json,
            plain,
            meeting: None,
        })
    }
}

/// What `obtain_analysis` needs besides the source and the output.
struct AnalysisRequest<'a> {
    backend: &'a dyn InferenceBackend,
    language: sagascript_core::settings::Language,
    prompt: Option<&'a str>,
    config: &'a DiarizeConfig,
    options: &'a TranscribeOptions,
    coverage_profile: &'a CoverageProfile,
    language_evidence: &'a LanguageEvidence,
    cache: Option<&'a CacheTarget<'a>>,
    cache_policy: CachePolicy,
    control: Option<&'a MeetingControl<'a>>,
}

/// The speaker analysis and the Whisper word timings of a file.
struct Analyzed {
    analysis: DiarizationAnalysis,
    /// Word timings: `(start, end, text)`.
    raw_segments: Vec<(f64, f64, String)>,
    coverage_segments: Vec<TranscriptSegment>,
    diarization_timings: DiarizationTimings,
    transcription_timings: DiarizationTranscriptionTimings,
}

/// Takes the analysis from a cache hit, or computes it (the diarization
/// analysis and the Whisper timestamps, overlapped where the platform allows)
/// and writes it to the cache when there is one.
fn obtain_analysis(
    source: Source,
    request: &AnalysisRequest<'_>,
    performance: &mut DiarizationPerformance,
    out: &Out,
) -> Result<Analyzed, DictationError> {
    let AnalysisRequest {
        backend,
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
            performance.cache_hit = true;
            let run = *run;
            return Ok(Analyzed {
                analysis: run.analysis,
                raw_segments: run.transcript,
                coverage_segments: run.coverage_segments,
                diarization_timings: DiarizationTimings::default(),
                transcription_timings: DiarizationTranscriptionTimings::default(),
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
        run_diarization_analysis(&audio.samples, backend, language, prompt, config, options)?;
    transcription.coverage_segments = canonicalize_coverage_segments(
        transcription.coverage_segments,
        coverage_profile.duration_seconds(),
    );
    meeting_checkpoint(control, MeetingPhase::Analyzing)?;
    performance.parallel_analysis_span_seconds = parallel_started.elapsed().as_secs_f64();

    if let Some(target) = cache {
        let cache_write_started = Instant::now();
        let cache =
            crate::diarization_cache::DiarizationCache::new_with_coverage_segments_and_policy(
                target.identity.clone(),
                analysis.clone(),
                transcription.segments.clone(),
                transcription.coverage_segments.clone(),
                coverage_profile.clone(),
                language_evidence.detected_language.clone(),
                language_evidence.language_regions.clone(),
                config.exclusive_speech_embeddings,
            );
        match cache_policy {
            CachePolicy::Normal => {
                crate::diarization_cache::save(target.path, &cache)?;
                performance.cache_write_seconds = cache_write_started.elapsed().as_secs_f64();
                errln!(
                    out,
                    "Saved reusable diarization cache: {}",
                    target.path.display()
                );
            }
            // `path` is a private staging path: the reprocessing
            // caller publishes the cache (and says so) only after
            // validating the whole run (#342).
            CachePolicy::RefreshNew => {
                crate::diarization_cache::save_new(target.path, &cache)?;
                performance.cache_write_seconds = cache_write_started.elapsed().as_secs_f64();
            }
        }
    }

    Ok(Analyzed {
        analysis,
        raw_segments: transcription.segments,
        coverage_segments: transcription.coverage_segments,
        diarization_timings,
        transcription_timings: transcription.timings,
    })
}
