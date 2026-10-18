//! The Pianissimo path of a file run (#343): plain Swedish transcription through
//! the engine host, with its own per-file JSON.

use std::path::{Path, PathBuf};
use std::time::Instant;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, Settings};
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::Glossary;

use super::args::TranscribeArgs;
use super::output::{BatchSink, FileTranscription};
use super::process_batch;
use super::progress::emit_progress;
use crate::output::{errln, Out};

pub(super) struct PianissimoPerformance {
    pub(super) model_load_seconds: f64,
    pub(super) model_verification_seconds: f64,
    pub(super) decode_resample_seconds: f64,
    pub(super) total_seconds: f64,
    /// Engine the host reported in `hello` (`coreml`, `onnx`); `unknown` if it never connected.
    pub(super) engine: String,
    pub(super) engine_warm: bool,
    /// Per-window host stage timings.
    pub(super) windows: Vec<sagascript_core::transcription::engine_host::WindowTiming>,
}

/// Per-file JSON for `--model pianissimo-sv`. The key set is a compatibility
/// contract: only additive keys (`performance.engine`, `performance.engine_warm`,
/// `performance.windows`)
/// may be added to what the NeMo-based release emitted.
pub(super) fn pianissimo_file_json(
    text: &str,
    words: &[sagascript_core::transcription::pianissimo_backend::PianissimoWord],
    language: Language,
    file: &Path,
    duration: f64,
    corrections: &impl serde::Serialize,
    perf: &PianissimoPerformance,
) -> serde_json::Value {
    let segments: Vec<_> = words
        .iter()
        .map(|word| {
            serde_json::json!({
                "start": word.start,
                "end": word.end,
                "text": word.word,
                "avg_logprob": null,
                "no_speech_prob": null,
                "quarantined": false,
            })
        })
        .collect();
    serde_json::json!({
        "text": text,
        "segments": segments,
        "language": language,
        "model": "pianissimo-sv",
        "file": file.display().to_string(),
        "duration_seconds": duration,
        "coverage_ratio": null,
        "uncovered_spans": [],
        "repetition_spans": [],
        "detected_language": null,
        "language_redetection_enabled": false,
        "language_regions": null,
        "warnings": [],
        "vocabulary_corrections": corrections,
        "performance": {
            "model_load_seconds": perf.model_load_seconds,
            "model_verification_seconds": perf.model_verification_seconds,
            "decode_resample_seconds": perf.decode_resample_seconds,
            "total_seconds": perf.total_seconds,
            "engine": perf.engine,
            "engine_warm": perf.engine_warm,
            "windows": perf.windows.iter().map(|w| {
                let mut window = serde_json::json!({
                    "index": w.index,
                    "audio_seconds": (w.end_sample - w.start_sample) as f64 / 16_000.0,
                    "tokens": w.tokens,
                    "preprocess_ms": w.preprocess_ms,
                    "encode_ms": w.encode_ms,
                    "decode_ms": w.decode_ms,
                    "round_trip_ms": w.round_trip_ms,
                });
                if w.boost_us > 0 {
                    window["boost_us"] = serde_json::json!(w.boost_us);
                }
                window
            }).collect::<Vec<_>>(),
        },
    })
}

pub(super) fn run_pianissimo_batch(
    args: &TranscribeArgs,
    files: &[PathBuf],
    _stored: &Settings,
    language: Language,
    glossary: &Glossary,
    out: &Out,
) -> Result<(), DictationError> {
    if language != Language::Swedish {
        return Err(DictationError::SettingsError(
            "Pianissimo supports Swedish files only".into(),
        ));
    }
    if args.vad
        || args.beam_size.is_some()
        || args.parallel.is_some()
        || args.correct_hints
        || args.prompt.is_some()
        || args.prompt_file.is_some()
    {
        return Err(DictationError::SettingsError(
            "Pianissimo does not support --vad, --beam, --parallel, --correct-hints, or decoder --prompt/--prompt-file".into(),
        ));
    }
    if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
        return Err(DictationError::TranscriptionFailed(
            sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into(),
        ));
    }
    if !pianissimo_model::is_downloaded() {
        return Err(DictationError::TranscriptionFailed(
            "Pianissimo is not downloaded. Run: sagascript download-model pianissimo-sv".into(),
        ));
    }
    if glossary.decoder_prompt().is_some() {
        errln!(out, "Pianissimo does not use Whisper decoder hints; saved glossary replacements still apply after transcription.");
    }
    let load_started = Instant::now();
    let cancel_flag = crate::cancel::flag();
    let backend = std::sync::Arc::new(PianissimoBackend::start_with_cancel(
        cancel_flag,
        &sagascript_core::machine::load(),
    )?);
    if let Some(weight) = args.glossary_boost {
        let mut terms: Vec<String> = glossary
            .entries()
            .iter()
            .map(|e| e.canonical.clone())
            .collect();
        if let Some(path) = &args.boost_terms_file {
            let text = std::fs::read_to_string(path).map_err(|e| {
                DictationError::SettingsError(format!(
                    "Cannot read --boost-terms-file {}: {e}",
                    path.display()
                ))
            })?;
            terms.extend(text.lines().map(str::to_string));
        }
        match sagascript_core::transcription::engine_host::BoostSpec::new(terms, weight) {
            Some(spec) => {
                errln!(
                    out,
                    "Context biasing requested: {} terms, weight {}",
                    spec.terms.len(),
                    spec.weight
                );
                if spec.truncated > 0 {
                    errln!(
                        out,
                        "Warning: only the first {} distinct terms are used; {} more were ignored.",
                        spec.terms.len(),
                        spec.truncated
                    );
                }
                if spec.too_long > 0 {
                    errln!(
                        out,
                        "Warning: {} term(s) longer than 64 characters were ignored.",
                        spec.too_long
                    );
                }
                backend.set_boost(Some(spec));
            }
            None => errln!(
                out,
                "Context biasing requested but no terms/weight; decoding unchanged."
            ),
        }
    }
    let _abort_pianissimo = {
        let backend = backend.clone();
        crate::cancel::register(move || backend.request_abort())
    };
    let model_verification_seconds = load_started.elapsed().as_secs_f64();
    // Start the engine host and load the model once for the whole batch.
    let warm = backend.warm_up_with_cancel(cancel_flag)?;
    let mut sink = BatchSink::new(args, files.len());
    let (processed, failures) = process_batch(
        files,
        args.fail_fast,
        |index, file| {
            errln!(out, "[{}/{}] {}", index + 1, files.len(), file.display());
            let started = Instant::now();
            emit_progress(out, args.progress_json, started, "decoding", Some(0));
            let audio = sagascript_core::audio::decoder::decode_audio_file_with_stage_progress(
                file,
                Some(&crate::cancel::check),
                &|pct| emit_progress(out, args.progress_json, started, "decoding", Some(pct)),
                &|pct| emit_progress(out, args.progress_json, started, "resampling", Some(pct)),
            )?;
            let duration = audio.len() as f64 / 16_000.0;
            let decode_resample_seconds = started.elapsed().as_secs_f64();
            emit_progress(out, args.progress_json, started, "transcribing", Some(0));
            let result = backend.transcribe_with_cancel(
                &audio,
                |pct| {
                    emit_progress(out, args.progress_json, started, "transcribing", Some(pct));
                },
                cancel_flag,
            )?;
            let (text, corrections) = glossary.correct_text(&result.text);
            let output = pianissimo_file_json(
                &text,
                &result.words,
                language,
                file,
                duration,
                &corrections,
                &PianissimoPerformance {
                    model_load_seconds: if index == 0 { warm.load_seconds } else { 0.0 },
                    model_verification_seconds: if index == 0 {
                        model_verification_seconds
                    } else {
                        0.0
                    },
                    decode_resample_seconds,
                    total_seconds: started.elapsed().as_secs_f64(),
                    engine: backend.engine_name().unwrap_or_else(|| "unknown".into()),
                    engine_warm: index > 0 || warm.was_warm,
                    windows: result.window_timings.clone(),
                },
            );
            emit_progress(out, args.progress_json, started, "completed", Some(100));
            Ok(FileTranscription {
                json: output,
                plain: text,
                #[cfg(feature = "diarization")]
                meeting: None,
            })
        },
        |execution| sink.emit(out, args, execution),
    )?;
    if args.glossary_boost.is_some() {
        let report = backend.client().boost_report();
        match report.warning() {
            Some(warning) => errln!(out, "Warning: {warning}"),
            None if report.active_windows() > 0 => errln!(
                out,
                "Context biasing active on {} window(s); trie build {:.1} ms.",
                report.active_windows(),
                report.trie_build_us as f64 / 1000.0
            ),
            None => {}
        }
    }
    sink.finish(out)?;
    if failures > 0 {
        return Err(DictationError::TranscriptionFailed(format!(
            "{failures} of {processed} batch item(s) failed"
        )));
    }
    Ok(())
}
