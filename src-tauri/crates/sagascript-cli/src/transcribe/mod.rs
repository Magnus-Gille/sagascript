//! `sagascript transcribe`: one file or a batch through Whisper (or Pianissimo),
//! with optional diarization (#343).
//!
//! `run` and `run_inner` plan the run; `transcribe_file` does one input. The
//! parts are modules of their own:
//!
//! - `args`: the command line (`TranscribeArgs`) and its value parsers;
//! - `inputs`: expanding directories into the files to work on;
//! - `resolve`: language and model IDs, which model a run gets, the one-run glossary;
//! - `vocab_correction`: glossary aliases and the opt-in one-edit hint repair;
//! - `language_detection`, `language_regions`: the language check and the sampled
//!   region detection, with their diagnostics;
//! - `diarize`: the diarization cache policy, decoder options and concurrent analysis;
//! - `meeting`: the meeting entry points the app and reprocessing call, with their
//!   cancellation control and source hashing;
//! - `pianissimo`: the Pianissimo (engine host) path;
//! - `progress`: `--progress-json` events;
//! - `output`: the per-file result, the three output modes, warnings and the clipboard.
//!
//! The paths other code imports (`sagascript_cli::transcribe::*`) are
//! re-exported below.

#[cfg(feature = "diarization")]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::error::DictationError;
use sagascript_core::run_context::{file_beam_size, RunContext};
use sagascript_core::settings::{FileModel, Language, Settings, WhisperModel};
use sagascript_core::transcription::diagnostics::{analyze_coverage, analyze_repetition};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::model;
use sagascript_core::transcription::whisper_backend::assemble_transcript;
use sagascript_core::transcription::{
    normalize_nonspeech_markers, recommended_parallel_chunks, ContextProfile, Glossary,
    TranscribeOptions, TranscriptSegment, WhisperBackend,
};

use crate::output::{errln, Out};
use crate::run_request::RunRequest;

mod args;
mod diarize;
mod inference;
mod inputs;
mod language_detection;
mod language_regions;
mod meeting;
mod output;
mod pianissimo;
mod progress;
mod resolve;
mod vocab_correction;

pub use self::args::TranscribeArgs;
#[cfg(feature = "diarization")]
pub(crate) use self::meeting::{
    meeting_transcript_from_plain_segments, stable_file_sha256, verify_meeting_source_unchanged,
};
#[cfg(feature = "diarization")]
pub use self::meeting::{
    transcribe_meeting_file, transcribe_meeting_file_full, transcribe_meeting_file_with_control,
};
pub use self::meeting::{MeetingControl, MeetingPhase};
pub use self::output::copy_to_clipboard;
#[cfg(feature = "diarization")]
pub(crate) use self::output::prepare_diarized_plain_segments;
pub(crate) use self::resolve::{effective_glossary, resolve_one_run_prompt};
pub use self::resolve::{
    model_id_string, parse_language, parse_model, resolve_effective_model, resolve_file_model,
    resolve_file_model_for_run,
};

#[cfg(feature = "diarization")]
use self::args::speaker_hint_from_args;
use self::diarize::CachePolicy;
#[cfg(feature = "diarization")]
use self::diarize::{
    canonicalize_coverage_segments, effective_diarization_decoder_evidence,
    resolve_diarization_options, run_diarization_analysis, DiarizationPerformance,
};
use self::inference::{ensure_model_loaded, InferenceBackend};
use self::inputs::expand_inputs;
#[cfg(feature = "diarization")]
use self::inputs::paths_refer_to_same_file;
use self::language_detection::detect_file_language;
use self::language_regions::detect_file_language_regions;
#[cfg(feature = "diarization")]
use self::meeting::{meeting_checkpoint, validate_meeting_input_scope};
#[cfg(feature = "diarization")]
use self::output::assemble_diarized_plain_text;
use self::output::{
    combined_warnings, emit_warnings, BatchExecution, BatchSink, FileTranscription,
};
use self::pianissimo::run_pianissimo_batch;
#[cfg(feature = "diarization")]
use self::progress::{decoder_progress_value, emit_meeting_progress};
use self::progress::{emit_progress, emit_progress_with_decoder};
use self::vocab_correction::{apply_glossary_corrections, apply_hint_corrections};

pub fn run(args: TranscribeArgs, out: &Out) -> Result<(), DictationError> {
    crate::cancel::install();
    let progress_json = args.progress_json;
    let started = Instant::now();
    let result = run_inner(args, out);
    if crate::cancel::is_cancelled() {
        emit_progress(out, progress_json, started, "cancelled", None);
    }
    result
}

fn run_inner(args: TranscribeArgs, out: &Out) -> Result<(), DictationError> {
    #[cfg(feature = "diarization")]
    speaker_hint_from_args(&args)?;
    let stored = sagascript_core::settings::store::load();
    let RunContext {
        language, glossary, ..
    } = RunRequest::from(&args).resolve(&stored)?;

    let files = expand_inputs(&args.files, args.recursive)?;
    #[cfg(feature = "diarization")]
    if args.meeting_json {
        validate_meeting_input_scope(&args.files, args.recursive, &files)?;
    }
    if files.is_empty() {
        return Err(DictationError::FileDecodeError(
            "No supported audio/video files were found in the supplied inputs".to_string(),
        ));
    }
    if files.len() > 1 && args.clipboard {
        return Err(DictationError::SettingsError(
            "--clipboard is only available when transcribing one file".to_string(),
        ));
    }
    #[cfg(feature = "diarization")]
    if files.len() > 1 && args.diarize {
        return Err(DictationError::SettingsError(
            "--diarize is only available when transcribing one file".to_string(),
        ));
    }
    #[cfg(feature = "diarization")]
    let wants_diarization = args.diarize || args.meeting_json;
    #[cfg(not(feature = "diarization"))]
    let wants_diarization = false;
    let needs_whisper = wants_diarization
        || args.vad
        || args.beam_size.is_some()
        || args.parallel.is_some()
        || args.correct_hints
        || args.prompt.is_some()
        || args.prompt_file.is_some();
    let selected_model =
        resolve_file_model_for_run(args.model.as_deref(), language, &stored, needs_whisper)?;

    if selected_model == FileModel::PianissimoOriginal {
        #[cfg(feature = "diarization")]
        if args.diarize || args.meeting_json {
            return Err(DictationError::SettingsError(
                "Pianissimo supports plain Swedish file transcription only; select a Whisper model for diarization or meeting output".into(),
            ));
        }
        return run_pianissimo_batch(&args, &files, &stored, language, &glossary, out);
    }
    let FileModel::Whisper(model) = selected_model else {
        unreachable!()
    };

    let correction_vocabulary = if args.correct_hints {
        if glossary.decoder_prompt().is_none() {
            return Err(DictationError::SettingsError(
                "--correct-hints requires a non-empty --hint/--prompt, saved initial_prompt, or selected profile dictionary"
                    .to_string(),
            ));
        }
        let vocabulary = glossary.single_word_terms();
        if vocabulary.is_empty() {
            return Err(DictationError::SettingsError(
                "--correct-hints requires at least one single-word vocabulary item in the hint"
                    .to_string(),
            ));
        }
        vocabulary
    } else {
        Vec::new()
    };

    #[cfg(feature = "diarization")]
    let cache_may_bypass_model = args.diarize && args.diarize_cache.is_some();
    #[cfg(not(feature = "diarization"))]
    let cache_may_bypass_model = false;
    if !cache_may_bypass_model && !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }

    let backend = std::sync::Arc::new(WhisperBackend::new());
    let _abort_whisper = {
        let backend = backend.clone();
        crate::cancel::register(move || backend.request_cancel())
    };
    run_whisper_batch(
        &WhisperBatch {
            args: &args,
            files: &files,
            stored: &stored,
            language,
            model,
            glossary: &glossary,
            correction_vocabulary: &correction_vocabulary,
        },
        &*backend,
        &crate::cancel::GLOBAL,
        out,
    )
}

/// What a Whisper batch needs besides the backend, the canceller and the
/// output: the planned run.
struct WhisperBatch<'a> {
    args: &'a TranscribeArgs,
    files: &'a [PathBuf],
    stored: &'a Settings,
    language: Language,
    model: WhisperModel,
    glossary: &'a Glossary,
    correction_vocabulary: &'a [String],
}

/// Transcribes the files one after another with `backend`, writing each
/// outcome as it is done and the batch's summary at the end.
///
/// Model loading is lazy so a validated diarization-cache hit never maps
/// several gigabytes of weights that it will not use. Batch processing is
/// sequential, so one flag safely preserves the load-once behavior.
fn run_whisper_batch(
    batch: &WhisperBatch<'_>,
    backend: &dyn InferenceBackend,
    canceller: &crate::cancel::Canceller,
    out: &Out,
) -> Result<(), DictationError> {
    let WhisperBatch {
        args,
        files,
        stored,
        language,
        model,
        glossary,
        correction_vocabulary,
    } = *batch;
    let mut model_loaded = false;

    let mut sink = BatchSink::new(args, files.len());
    let (processed, failures) = process_batch_with(
        canceller,
        files,
        args.fail_fast,
        |index, file| {
            errln!(out, "[{}/{}] {}", index + 1, files.len(), file.display());
            transcribe_file(
                args,
                file,
                backend,
                stored,
                language,
                model,
                &mut model_loaded,
                glossary,
                correction_vocabulary,
                None,
                CachePolicy::Normal,
                out,
            )
        },
        |execution| sink.emit(out, args, execution),
    )?;
    sink.finish(out)?;

    if failures > 0 {
        return Err(DictationError::TranscriptionFailed(format!(
            "{failures} of {} batch item(s) failed",
            processed
        )));
    }
    Ok(())
}

fn process_batch<F, E>(
    files: &[PathBuf],
    fail_fast: bool,
    process: F,
    emit: E,
) -> Result<(usize, usize), DictationError>
where
    F: FnMut(usize, &Path) -> Result<FileTranscription, DictationError>,
    E: FnMut(BatchExecution) -> Result<(), DictationError>,
{
    process_batch_with(&crate::cancel::GLOBAL, files, fail_fast, process, emit)
}

fn process_batch_with<F, E>(
    canceller: &crate::cancel::Canceller,
    files: &[PathBuf],
    fail_fast: bool,
    mut process: F,
    mut emit: E,
) -> Result<(usize, usize), DictationError>
where
    F: FnMut(usize, &Path) -> Result<FileTranscription, DictationError>,
    E: FnMut(BatchExecution) -> Result<(), DictationError>,
{
    let mut processed = 0usize;
    let mut failures = 0usize;
    for (index, file) in files.iter().enumerate() {
        // Ctrl-C cancels the whole batch: no further files start and no
        // partial result for the interrupted one is emitted.
        canceller.check()?;
        let output = process(index, file);
        canceller.check()?;
        let failed = output.is_err();
        processed += 1;
        failures += usize::from(failed);
        emit(BatchExecution {
            source: file.clone(),
            output,
        })?;
        if failed && fail_fast {
            break;
        }
    }
    Ok((processed, failures))
}

#[allow(clippy::too_many_arguments)]
fn transcribe_file(
    args: &TranscribeArgs,
    file: &Path,
    backend: &dyn InferenceBackend,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    model_loaded: &mut bool,
    glossary: &Glossary,
    correction_vocabulary: &[String],
    control: Option<&MeetingControl<'_>>,
    cache_policy: CachePolicy,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    #[cfg(not(feature = "diarization"))]
    let _ = (control, cache_policy);

    let file_started = Instant::now();
    #[cfg(feature = "diarization")]
    let meeting_source_sha256 = if args.diarize {
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        let source_sha256 = stable_file_sha256(file, control)?;
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        Some(source_sha256)
    } else {
        None
    };
    #[cfg(feature = "diarization")]
    let decoder_prompt = glossary.decoder_prompt();
    #[cfg(feature = "diarization")]
    let diarization_options = args
        .diarize
        .then(|| resolve_diarization_options(args, stored))
        .transpose()?;
    #[cfg(feature = "diarization")]
    let diarization_decoder_evidence = diarization_options
        .as_ref()
        .map(effective_diarization_decoder_evidence)
        .transpose()?;
    #[cfg(feature = "diarization")]
    let diarization_decoder_progress = diarization_decoder_evidence
        .as_ref()
        .map(|decoder| decoder_progress_value(decoder, args.meeting_json));
    #[cfg(not(feature = "diarization"))]
    let diarization_decoder_progress: Option<serde_json::Value> = None;
    #[cfg(feature = "diarization")]
    if args
        .diarize_cache
        .as_deref()
        .is_some_and(|cache_path| paths_refer_to_same_file(file, cache_path))
    {
        return Err(DictationError::SettingsError(
            "--diarize-cache must not point to the input recording".to_string(),
        ));
    }
    #[cfg(feature = "diarization")]
    let cache_lookup_started = Instant::now();
    #[cfg(feature = "diarization")]
    let cache_identity = if args.diarize && args.diarize_cache.is_some() {
        let options = diarization_options
            .as_ref()
            .expect("diarization options are resolved for diarized cache identities");
        Some(match meeting_source_sha256.as_deref() {
            Some(source_sha256) => {
                crate::diarization_cache::CacheIdentity::for_source_sha256_with_options(
                    source_sha256,
                    language.whisper_code().unwrap_or("auto"),
                    model_id_string(model),
                    decoder_prompt.as_deref(),
                    options.beam_size,
                    options.temperature_fallback,
                    options.vad_model_path.is_some(),
                )?
            }
            None => crate::diarization_cache::CacheIdentity::for_input_with_options(
                file,
                language.whisper_code().unwrap_or("auto"),
                model_id_string(model),
                decoder_prompt.as_deref(),
                options.beam_size,
                options.temperature_fallback,
                options.vad_model_path.is_some(),
            )?,
        })
    } else {
        None
    };
    #[cfg(feature = "diarization")]
    let cached = if cache_policy.reads_existing_cache() {
        match (args.diarize_cache.as_deref(), cache_identity.as_ref()) {
            (Some(path), Some(identity)) => {
                match crate::diarization_cache::load_with_analysis_policy(
                    path,
                    identity,
                    args.diarize_exclusive_speech_embeddings,
                )? {
                    crate::diarization_cache::CacheLookup::Hit(cached) => {
                        errln!(out, "Reusing diarization cache: {}", path.display());
                        Some(cached)
                    }
                    crate::diarization_cache::CacheLookup::Miss(reason) => {
                        errln!(
                            out,
                            "Diarization cache miss ({reason}); computing analysis."
                        );
                        None
                    }
                }
            }
            _ => None,
        }
    } else {
        None
    };
    #[cfg(feature = "diarization")]
    let cache_lookup_seconds = cache_lookup_started.elapsed().as_secs_f64();
    #[cfg(feature = "diarization")]
    let cache_hit = args.diarize && cached.is_some();
    #[cfg(not(feature = "diarization"))]
    let cache_hit = false;

    #[cfg(feature = "diarization")]
    if args.diarize && !cache_hit && !sagascript_core::diarization::model::all_models_downloaded() {
        return Err(DictationError::DiarizationError(
            "Diarization models not found. Run: sagascript download-model diarization".to_string(),
        ));
    }

    #[cfg(feature = "diarization")]
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Decoding)?;
    }
    let (audio, decode_resample_seconds, duration) = if cache_hit {
        #[cfg(feature = "diarization")]
        {
            let duration = cached
                .as_ref()
                .expect("cache_hit requires cached data")
                .coverage_profile
                .duration_seconds();
            errln!(out, "Audio (cached): {duration:.1}s");
            (None, 0.0, duration)
        }
        #[cfg(not(feature = "diarization"))]
        unreachable!("cache hits require the diarization feature")
    } else {
        errln!(out, "Decoding {}...", file.display());
        let decode_started = Instant::now();
        // Byte-fraction decode bar for large files (#237): small files
        // decode sub-second, so only bar above 8 MB to avoid flicker.
        let decode_len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        let decode_bar = (decode_len > 8_000_000).then(|| {
            let pb = ProgressBar::new(100);
            pb.set_style(ProgressStyle::with_template("  Decoding [{bar:40}] {pos}%").unwrap());
            pb
        });
        let on_decode = |pct: u8| {
            emit_progress_with_decoder(
                out,
                args.progress_json,
                file_started,
                "decoding",
                Some(pct),
                diarization_decoder_progress.as_ref(),
            );
            if let Some(pb) = &decode_bar {
                pb.set_position(pct as u64);
            }
        };
        let on_resample = |pct: u8| {
            emit_progress_with_decoder(
                out,
                args.progress_json,
                file_started,
                "resampling",
                Some(pct),
                diarization_decoder_progress.as_ref(),
            );
            if let Some(pb) = &decode_bar {
                if pct == 0 {
                    pb.set_style(
                        ProgressStyle::with_template("  Resampling [{bar:40}] {pos}%").unwrap(),
                    );
                }
                pb.set_position(pct as u64);
            }
        };
        let checkpoint = || {
            crate::cancel::check()?;
            control.map_or(Ok(()), |control| control.check())
        };
        on_decode(0);
        let audio = sagascript_core::audio::decoder::decode_audio_file_with_stage_progress(
            file,
            Some(&checkpoint),
            &on_decode,
            &on_resample,
        )?;
        if let Some(pb) = decode_bar {
            pb.finish_and_clear();
        }
        let decode_resample_seconds = decode_started.elapsed().as_secs_f64();
        let duration = audio.len() as f64 / 16_000.0;
        errln!(out, "Audio: {:.1}s, {} samples", duration, audio.len());
        (Some(audio), decode_resample_seconds, duration)
    };
    #[cfg(feature = "diarization")]
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Decoding)?;
    }
    #[cfg(feature = "diarization")]
    let coverage_profile = if cache_hit {
        cached
            .as_ref()
            .expect("cache_hit requires cached data")
            .coverage_profile
            .clone()
    } else {
        CoverageProfile::from_audio(audio.as_deref().expect("cache misses decode audio"))
    };

    #[cfg(feature = "diarization")]
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    #[cfg(feature = "diarization")]
    if !cache_hit {
        if let Some(options) = diarization_options.as_ref() {
            if options.vad_model_path.is_some() {
                errln!(out, "Verifying Silero VAD model...");
                tokio::runtime::Runtime::new()
                    .map_err(|e| {
                        DictationError::ModelDownloadFailed(format!("tokio runtime: {e}"))
                    })?
                    .block_on(model::download_vad_model(|_, _| {}))?;
            }
        }
    }
    let (model_load_seconds, detected_language, language_regions, language_detection_seconds) =
        if cache_hit {
            #[cfg(feature = "diarization")]
            {
                let cached = cached.as_ref().expect("cache_hit requires cached data");
                (
                    0.0,
                    cached.detected_language.clone(),
                    cached.language_regions.clone(),
                    0.0,
                )
            }
            #[cfg(not(feature = "diarization"))]
            unreachable!("cache hits require the diarization feature")
        } else {
            #[cfg(feature = "diarization")]
            let context_profile = ContextProfile::for_diarization(args.diarize);
            #[cfg(not(feature = "diarization"))]
            let context_profile = ContextProfile::FlashAttention;
            emit_progress_with_decoder(
                out,
                args.progress_json,
                file_started,
                "loading",
                None,
                diarization_decoder_progress.as_ref(),
            );
            let model_load_seconds =
                ensure_model_loaded(out, backend, model, context_profile, model_loaded)?;
            crate::cancel::check()?;
            emit_progress_with_decoder(
                out,
                args.progress_json,
                file_started,
                "language_detection",
                None,
                diarization_decoder_progress.as_ref(),
            );
            let language_detection_started = Instant::now();
            let audio = audio.as_deref().expect("cache misses decode audio");
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
            let language_regions = match detect_file_language_regions(out, backend, audio, language)
            {
                Ok(diagnostics) => diagnostics,
                Err(error) => {
                    errln!(
                        out,
                        "Warning: language region detection was unavailable: {error}"
                    );
                    None
                }
            };
            crate::cancel::check()?;
            (
                model_load_seconds,
                detected_language,
                language_regions,
                language_detection_started.elapsed().as_secs_f64(),
            )
        };
    #[cfg(feature = "diarization")]
    if args.meeting_json {
        meeting_checkpoint(control, MeetingPhase::LoadingModel)?;
    }
    #[cfg(not(feature = "diarization"))]
    let _ = (
        model_load_seconds,
        file_started,
        decode_resample_seconds,
        language_detection_seconds,
    );

    // Diarization branch
    #[cfg(feature = "diarization")]
    if args.diarize {
        if args.correct_hints {
            return Err(DictationError::SettingsError(
                "--correct-hints is not available with --diarize because diarized output has no segment confidence"
                    .to_string(),
            ));
        }
        use sagascript_core::diarization::{
            cluster_with_outcome,
            merge::{consolidate, merge_with_diagnostics},
            DiarizationTimings, DiarizeConfig, TimestampedSegment,
        };
        use sagascript_core::transcription::DiarizationTranscriptionTimings;
        let effective_options = diarization_options
            .as_ref()
            .expect("diarization options are resolved for diarized runs");

        let acceleration = model::acceleration_profile(model);
        let mut performance = DiarizationPerformance {
            acceleration_backend: acceleration.backend,
            coreml_status: acceleration.coreml_status,
            model_load_seconds,
            decode_resample_seconds,
            language_detection_seconds,
            effective_beam_size: effective_options.beam_size,
            effective_temperature_fallback: effective_options.temperature_fallback,
            effective_vad: effective_options.vad_model_path.is_some(),
            ..DiarizationPerformance::default()
        };
        performance.cache_lookup_seconds = cache_lookup_seconds;
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
            effective_options.beam_size,
            effective_options.temperature_fallback,
            effective_options.vad_model_path.is_some()
        );

        emit_meeting_progress(
            out,
            args.progress_json,
            file_started,
            "analyzing",
            diarization_decoder_progress.as_ref(),
        );
        let (analysis, raw_segments, coverage_segments, diarization_timings, transcription_timings) =
            if let Some(cached) = cached {
                performance.cache_hit = true;
                let cached = *cached;
                (
                    cached.analysis,
                    cached.transcript,
                    cached
                        .coverage_segments
                        .expect("validated cache must contain coverage segments"),
                    DiarizationTimings::default(),
                    DiarizationTranscriptionTimings::default(),
                )
            } else {
                errln!(
                    out,
                    "Running speaker diarization and Whisper timestamps concurrently..."
                );
                let parallel_started = Instant::now();
                let audio = audio.as_deref().expect("cache misses decode audio");
                meeting_checkpoint(control, MeetingPhase::Analyzing)?;
                let (analysis, diarization_timings, mut transcription) = run_diarization_analysis(
                    audio,
                    backend,
                    language,
                    decoder_prompt.as_deref(),
                    &config,
                    effective_options,
                )?;
                transcription.coverage_segments = canonicalize_coverage_segments(
                    transcription.coverage_segments,
                    coverage_profile.duration_seconds(),
                );
                meeting_checkpoint(control, MeetingPhase::Analyzing)?;
                performance.parallel_analysis_span_seconds =
                    parallel_started.elapsed().as_secs_f64();

                if let (Some(path), Some(identity)) =
                    (args.diarize_cache.as_deref(), cache_identity.clone())
                {
                    let cache_write_started = Instant::now();
                    let cache =
                        crate::diarization_cache::DiarizationCache::new_with_coverage_segments_and_policy(
                            identity,
                            analysis.clone(),
                            transcription.segments.clone(),
                            transcription.coverage_segments.clone(),
                            coverage_profile.clone(),
                            detected_language.clone(),
                            language_regions.clone(),
                            config.exclusive_speech_embeddings,
                        );
                    match cache_policy {
                        CachePolicy::Normal => {
                            crate::diarization_cache::save(path, &cache)?;
                            performance.cache_write_seconds =
                                cache_write_started.elapsed().as_secs_f64();
                            errln!(out, "Saved reusable diarization cache: {}", path.display());
                        }
                        // `path` is a private staging path: the reprocessing
                        // caller publishes the cache (and says so) only after
                        // validating the whole run (#342).
                        CachePolicy::RefreshNew => {
                            crate::diarization_cache::save_new(path, &cache)?;
                            performance.cache_write_seconds =
                                cache_write_started.elapsed().as_secs_f64();
                        }
                    }
                }

                (
                    analysis,
                    transcription.segments,
                    transcription.coverage_segments,
                    diarization_timings,
                    transcription.timings,
                )
            };

        performance.diarization_model_load_seconds = diarization_timings.model_load_seconds;
        performance.diarization_segmentation_seconds = diarization_timings.segmentation_seconds;
        performance.diarization_segment_extraction_seconds =
            diarization_timings.segment_extraction_seconds;
        performance.diarization_embeddings_seconds = diarization_timings.embeddings_seconds;
        performance.whisper_inference_seconds = transcription_timings.whisper_inference_seconds;
        performance.word_timestamp_attribution_seconds =
            transcription_timings.word_timestamp_attribution_seconds;

        let clustering_started = Instant::now();
        emit_meeting_progress(
            out,
            args.progress_json,
            file_started,
            "clustering",
            diarization_decoder_progress.as_ref(),
        );
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        let (speaker_segments, hint_outcome) = cluster_with_outcome(&analysis, &config)?;
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        performance.diarization_clustering_seconds = clustering_started.elapsed().as_secs_f64();
        errln!(out, "Found {} speaker segment(s)", speaker_segments.len());
        if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
            for s in &speaker_segments {
                errln!(out, "DIARSEG\t{:.3}\t{:.3}\t{}", s.start, s.end, s.speaker);
            }
        }

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

        emit_meeting_progress(
            out,
            args.progress_json,
            file_started,
            "finalizing",
            diarization_decoder_progress.as_ref(),
        );
        let merge_diagnostics_started = Instant::now();
        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let (diarized, attributions) = merge_with_diagnostics(&speaker_segments, &transcript);
        let source_sha256 = meeting_source_sha256.expect("diarized output records its source hash");
        let mut report = analysis.report(
            &speaker_segments,
            &config,
            attributions,
            args.diarize_diagnostics,
            source_sha256.clone(),
            duration,
            crate::GIT_HASH.into(),
            env!("CARGO_PKG_VERSION").into(),
        )?;
        report.decoder = Some(
            diarization_decoder_evidence
                .clone()
                .expect("diarized runs resolve decoder evidence before inference"),
        );
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
        verify_meeting_source_unchanged(file, &source_sha256, control)?;
        let plain_segments = prepare_diarized_plain_segments(&diarized, language, glossary);
        let mut consolidated = consolidate(&diarized);
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
        let coverage = analyze_coverage_profile(&coverage_profile, &coverage_segments);
        let mut warnings = combined_warnings(&coverage, language, detected_language.as_ref());
        if let Some(diagnostics) = &language_regions {
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
        performance.merge_diagnostics_seconds = merge_diagnostics_started.elapsed().as_secs_f64();
        let json_assembly_started = Instant::now();
        let mut json = serde_json::json!({
            "segments": consolidated,
            "speakers": speakers,
            "language": language,
            "model": model_id_string(model),
            "file": file.display().to_string(),
            "duration_seconds": duration,
            "coverage_ratio": coverage.coverage_ratio,
            "uncovered_spans": coverage.uncovered_spans,
            "detected_language": detected_language,
            "language_redetection_enabled": language_regions.is_some(),
            "language_regions": language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
            "warnings": warnings,
            "vocabulary_corrections": glossary_corrections,
            "diarization": report,
        });
        performance.json_assembly_seconds = json_assembly_started.elapsed().as_secs_f64();
        performance.total_seconds = file_started.elapsed().as_secs_f64();
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
            let meeting = meeting_transcript_from_plain_segments(
                source_sha256,
                language,
                model,
                duration,
                &plain_segments,
            )?
            .with_speaker_hint(speaker_hint)
            .with_speaker_hint_outcome(hint_outcome)
            .with_diarization(report)
            .map_err(|error| DictationError::DiarizationError(error.to_string()))?;
            let json = serde_json::to_value(&meeting).map_err(|_| {
                DictationError::TranscriptionFailed(
                    "Diarized meeting transcript serialization failed".to_string(),
                )
            })?;
            meeting_checkpoint(control, MeetingPhase::Finalizing)?;
            emit_meeting_progress(
                out,
                args.progress_json,
                file_started,
                "completed",
                diarization_decoder_progress.as_ref(),
            );
            return Ok(FileTranscription {
                json,
                plain,
                meeting: Some(meeting),
            });
        }
        emit_meeting_progress(
            out,
            args.progress_json,
            file_started,
            "completed",
            diarization_decoder_progress.as_ref(),
        );
        return Ok(FileTranscription {
            json,
            plain,
            meeting: None,
        });
    }

    // Standard (non-diarized) transcription. Build options from the saved
    let audio = audio.expect("standard transcription decodes audio");
    // settings, with CLI flags overriding.
    let vad_enabled = if args.no_vad {
        false
    } else if args.vad {
        true
    } else {
        stored.vad_enabled
    };
    let vad_model_path = if vad_enabled {
        let path = model::vad_model_path();
        errln!(out, "Verifying Silero VAD model...");
        tokio::runtime::Runtime::new()
            .map_err(|e| DictationError::ModelDownloadFailed(format!("tokio runtime: {e}")))?
            .block_on(model::download_vad_model(|_, _| {}))?;
        path.to_str().map(str::to_string)
    } else {
        None
    };
    let mut opts = TranscribeOptions {
        prompt: glossary.decoder_prompt(),
        // File transcription isn't latency-sensitive, so default to beam search
        // (fewer repetition loops). Honor an explicit beam setting/flag.
        beam_size: args
            .beam_size
            .unwrap_or_else(|| file_beam_size(stored.beam_size)),
        temperature_fallback: stored.temperature_fallback,
        vad_model_path,
        // Coverage diagnostics need real segment bounds for both human and
        // JSON output. Text decoding remains in no-timestamps mode.
        segment_timestamps: true,
        parallel_chunks: 1,
    };
    opts.parallel_chunks = args
        .parallel
        .unwrap_or_else(|| recommended_parallel_chunks(audio.len(), model, opts.beam_size));
    if opts.beam_size >= 2 {
        errln!(out, "Beam search: width {}", opts.beam_size);
    }
    if opts.parallel_chunks > 1 {
        errln!(out, "Parallel Whisper states: {}", opts.parallel_chunks);
    }
    if vad_enabled {
        errln!(out, "VAD: enabled");
    }

    // "Preparing" narration (GUI parity, #237): lock acquisition plus
    // parallel-state creation are silent multi-second work on first use.
    errln!(out, "Preparing Whisper states...");
    // "Encoding" narration (GUI parity, #237): fires when compute resources
    // are held and mel/encoder work begins — the silent window before the
    // first Transcribing-bar update.
    emit_progress(out, args.progress_json, file_started, "preparing", None);
    let progress_json = args.progress_json;
    let on_encode_start = || {
        errln!(out, "Encoding audio...");
        emit_progress(out, progress_json, file_started, "encoding", None);
    };
    let segments = if duration > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap());
        let pb_cb = pb.clone();
        let progress_out = out.clone();
        let segments = backend.transcribe_segments(
            &audio,
            language,
            &opts,
            Box::new(move |pct| {
                crate::set_transcription_progress(&pb_cb, pct);
                if pct > 1 {
                    emit_progress(
                        &progress_out,
                        progress_json,
                        file_started,
                        "transcribing",
                        Some(pct.clamp(0, 100) as u8),
                    );
                }
            }),
            &on_encode_start,
        )?;
        pb.finish_and_clear();
        segments
    } else {
        errln!(out, "Transcribing...");
        let progress_out = out.clone();
        backend.transcribe_segments(
            &audio,
            language,
            &opts,
            Box::new(move |pct| {
                if pct > 1 {
                    emit_progress(
                        &progress_out,
                        progress_json,
                        file_started,
                        "transcribing",
                        Some(pct.clamp(0, 100) as u8),
                    );
                }
            }),
            &on_encode_start,
        )?
    };
    emit_progress(out, args.progress_json, file_started, "finalizing", None);
    let mut segments = segments;
    let mut corrections = apply_glossary_corrections(&mut segments, glossary);
    if args.correct_hints {
        corrections.extend(apply_hint_corrections(&mut segments, correction_vocabulary));
    }
    let repetition = analyze_repetition(&segments);
    let trusted_segments: Vec<TranscriptSegment> = segments
        .iter()
        .enumerate()
        .filter(|(index, _)| !repetition.quarantines_segment(*index))
        .map(|(_, segment)| segment.clone())
        .collect();
    // Keep timestamped segment text source-faithful, while the rendered
    // top-level text excludes quarantined loops and uses the same display
    // normalization as live dictation.
    let raw_text = assemble_transcript(&trusted_segments).trim().to_string();
    let text = normalize_nonspeech_markers(&raw_text, language);
    let coverage = analyze_coverage(&audio, &trusted_segments);
    let mut warnings = combined_warnings(&coverage, language, detected_language.as_ref());
    if let Some(diagnostics) = &language_regions {
        warnings.extend(diagnostics.warnings.clone());
    }
    warnings.extend(repetition.warnings.clone());
    emit_warnings(out, &warnings);

    // Per-segment confidence (#81): avg_logprob is the mean token
    // log-probability (null when a segment has no scoreable tokens);
    // no_speech_prob near 1.0 flags likely-hallucinated segments.
    let json_segments: Vec<serde_json::Value> = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            serde_json::json!({
                "start": segment.start,
                "end": segment.end,
                "text": segment.text.trim(),
                "avg_logprob": segment.avg_logprob,
                "no_speech_prob": segment.no_speech_prob,
                "quarantined": repetition.quarantines_segment(index),
            })
        })
        .collect();
    let json = serde_json::json!({
        "text": text,
        "segments": json_segments,
        "language": language,
        "model": model_id_string(model),
        "file": file.display().to_string(),
        "duration_seconds": duration,
        "coverage_ratio": coverage.coverage_ratio,
        "uncovered_spans": coverage.uncovered_spans,
        "repetition_spans": repetition.spans,
        "detected_language": detected_language,
        "language_redetection_enabled": language_regions.is_some(),
        "language_regions": language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
        "warnings": warnings,
        "vocabulary_corrections": corrections,
    });

    emit_progress(
        out,
        args.progress_json,
        file_started,
        "completed",
        Some(100),
    );
    Ok(FileTranscription {
        json,
        plain: text,
        #[cfg(feature = "diarization")]
        meeting: None,
    })
}

#[cfg(test)]
#[path = "../transcribe_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../transcribe_file_tests.rs"]
mod file_tests;
