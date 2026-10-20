//! The Pianissimo path of a file run (#343): plain Swedish transcription through
//! the engine host, with its own per-file JSON.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use sagascript_core::error::DictationError;
use sagascript_core::settings::Language;
use sagascript_core::transcription::engine_host::{BoostReport, BoostSpec, WindowTiming};
use sagascript_core::transcription::pianissimo_backend::{
    PianissimoBackend, PianissimoResult, WarmInfo,
};
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::Glossary;

use super::args::TranscribeArgs;
use super::outcome::FileTranscription;
use super::output::{BatchSink, Clipboard};
use super::process_batch_with;
use super::progress::emit_progress;
use crate::cancel::Canceller;
use crate::output::{errln, Out};

// ---- the engine host as a run uses it ----------------------------------------------

/// A started Pianissimo engine: what a batch asks of it once it exists.
///
/// The same pattern as [`InferenceBackend`](super::inference::InferenceBackend):
/// `PianissimoBackend` (the engine-host client) is the one implementation that
/// ships, and a test supplies its own, so the batch (its checks, its cancellation,
/// its per-file results and its context biasing) runs without the engine host.
///
/// `Send + Sync` because the cancellation hook holds a handle to it.
pub(super) trait PianissimoEngine: Send + Sync {
    /// Steers decoding with dictionary terms (context biasing); `None` clears it.
    fn set_boost(&self, boost: Option<BoostSpec>);

    /// Starts the host and loads the model now, reporting whether it was warm.
    /// Returns promptly when `cancelled` is set.
    fn warm_up(&self, cancelled: &AtomicBool) -> Result<WarmInfo, DictationError>;

    /// Transcribes 16 kHz mono samples as a file job; `progress` gets 0..=100.
    fn transcribe(
        &self,
        samples: &[f32],
        progress: &dyn Fn(u8),
        cancelled: &AtomicBool,
    ) -> Result<PianissimoResult, DictationError>;

    /// The engine the host reported (`coreml`, `onnx`), once connected.
    fn engine_name(&self) -> Option<String>;

    /// What happened to the dictionary on the windows sent so far.
    fn boost_report(&self) -> BoostReport;

    /// Cancels the running transcription (best effort).
    fn request_abort(&self);
}

impl PianissimoEngine for PianissimoBackend {
    fn set_boost(&self, boost: Option<BoostSpec>) {
        PianissimoBackend::set_boost(self, boost);
    }

    fn warm_up(&self, cancelled: &AtomicBool) -> Result<WarmInfo, DictationError> {
        self.warm_up_with_cancel(cancelled)
    }

    fn transcribe(
        &self,
        samples: &[f32],
        progress: &dyn Fn(u8),
        cancelled: &AtomicBool,
    ) -> Result<PianissimoResult, DictationError> {
        self.transcribe_with_cancel(samples, progress, cancelled)
    }

    fn engine_name(&self) -> Option<String> {
        PianissimoBackend::engine_name(self)
    }

    fn boost_report(&self) -> BoostReport {
        self.client().boost_report()
    }

    fn request_abort(&self) {
        PianissimoBackend::request_abort(self);
    }
}

/// Where a run finds the engine host: whether this machine can run it, whether
/// its model is installed, and how to start it.
pub(super) trait PianissimoHost {
    /// Whether the OS and the CPU can run the engine host.
    fn runtime_supported(&self) -> bool;

    /// Whether the Pianissimo model is installed.
    fn model_installed(&self) -> bool;

    /// Attaches to the engine host without loading the model. Fails at once if
    /// `cancelled` is set.
    fn start(&self, cancelled: &AtomicBool) -> Result<Arc<dyn PianissimoEngine>, DictationError>;
}

/// The engine host of an installed copy.
pub(super) struct InstalledPianissimo;

impl PianissimoHost for InstalledPianissimo {
    fn runtime_supported(&self) -> bool {
        sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os()
    }

    fn model_installed(&self) -> bool {
        pianissimo_model::is_downloaded()
    }

    fn start(&self, cancelled: &AtomicBool) -> Result<Arc<dyn PianissimoEngine>, DictationError> {
        Ok(Arc::new(PianissimoBackend::start_with_cancel(
            cancelled,
            &sagascript_core::machine::load(),
        )?))
    }
}

// ---- the per-file result -----------------------------------------------------------

pub(super) struct PianissimoPerformance {
    pub(super) model_load_seconds: f64,
    pub(super) model_verification_seconds: f64,
    pub(super) decode_resample_seconds: f64,
    pub(super) total_seconds: f64,
    /// Engine the host reported in `hello` (`coreml`, `onnx`); `unknown` if it never connected.
    pub(super) engine: String,
    pub(super) engine_warm: bool,
    /// Per-window host stage timings.
    pub(super) windows: Vec<WindowTiming>,
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

// ---- the batch -----------------------------------------------------------------------

/// What a Pianissimo batch is asked to do: the planned run.
pub(super) struct PianissimoBatch<'a> {
    pub(super) args: &'a TranscribeArgs,
    pub(super) files: &'a [PathBuf],
    pub(super) language: Language,
    pub(super) glossary: &'a Glossary,
    /// Where `--clipboard` puts the transcript.
    pub(super) clipboard: &'a dyn Clipboard,
}

/// Transcribes the files one after another through the engine host, writing
/// each outcome as it is done and the batch's summary at the end.
///
/// The host is started and the model loaded once for the whole batch. The
/// checks come first, in a fixed order, and none of them starts the host.
pub(super) fn run_pianissimo_batch(
    batch: &PianissimoBatch<'_>,
    host: &dyn PianissimoHost,
    canceller: &Canceller,
    out: &Out,
) -> Result<(), DictationError> {
    let PianissimoBatch {
        args,
        files,
        language,
        glossary,
        clipboard,
    } = *batch;
    check_request(batch, host, out)?;
    let load_started = Instant::now();
    let cancel_flag = canceller.flag();
    let engine = host.start(cancel_flag)?;
    if let Some(weight) = args.glossary_boost {
        request_boost(&*engine, args, glossary, weight, out)?;
    }
    let _abort_pianissimo = {
        let engine = engine.clone();
        canceller.register(move || engine.request_abort())
    };
    let model_verification_seconds = load_started.elapsed().as_secs_f64();
    let warm = engine.warm_up(cancel_flag)?;
    let mut sink = BatchSink::new(args, files.len());
    let (processed, failures) = process_batch_with(
        canceller,
        files,
        args.fail_fast,
        |index, file| {
            errln!(out, "[{}/{}] {}", index + 1, files.len(), file.display());
            transcribe_one(
                &FileRun {
                    args,
                    language,
                    glossary,
                    engine: &*engine,
                    canceller,
                    first_file: index == 0,
                    warm,
                    model_verification_seconds,
                },
                file,
                out,
            )
        },
        |execution| sink.emit(out, args, clipboard, execution),
    )?;
    if args.glossary_boost.is_some() {
        report_boost(&engine.boost_report(), out);
    }
    sink.finish(out)?;
    if failures > 0 {
        return Err(DictationError::TranscriptionFailed(format!(
            "{failures} of {processed} batch item(s) failed"
        )));
    }
    Ok(())
}

/// Refuses what Pianissimo cannot do, in this order: a language other than
/// Swedish, a decoder option it does not have, an OS without the engine host,
/// a model that is not installed. Then notes that the saved glossary applies
/// only after transcription.
fn check_request(
    batch: &PianissimoBatch<'_>,
    host: &dyn PianissimoHost,
    out: &Out,
) -> Result<(), DictationError> {
    let PianissimoBatch {
        args,
        language,
        glossary,
        ..
    } = *batch;
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
    if !host.runtime_supported() {
        return Err(DictationError::TranscriptionFailed(
            sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into(),
        ));
    }
    if !host.model_installed() {
        return Err(DictationError::TranscriptionFailed(
            "Pianissimo is not downloaded. Run: sagascript download-model pianissimo-sv".into(),
        ));
    }
    if glossary.decoder_prompt().is_some() {
        errln!(out, "Pianissimo does not use Whisper decoder hints; saved glossary replacements still apply after transcription.");
    }
    Ok(())
}

/// Steers the engine with the glossary terms (and the terms file, if any) at
/// the given weight, saying on stderr how many terms it uses and what it dropped.
fn request_boost(
    engine: &dyn PianissimoEngine,
    args: &TranscribeArgs,
    glossary: &Glossary,
    weight: f32,
    out: &Out,
) -> Result<(), DictationError> {
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
    match BoostSpec::new(terms, weight) {
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
            engine.set_boost(Some(spec));
        }
        None => errln!(
            out,
            "Context biasing requested but no terms/weight; decoding unchanged."
        ),
    }
    Ok(())
}

/// Says whether the dictionary took effect on the windows that were sent.
fn report_boost(report: &BoostReport, out: &Out) {
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

/// What transcribing one file needs besides the file and the output.
struct FileRun<'a> {
    args: &'a TranscribeArgs,
    language: Language,
    glossary: &'a Glossary,
    engine: &'a dyn PianissimoEngine,
    canceller: &'a Canceller,
    /// The batch's first file carries the model load time; later ones find it warm.
    first_file: bool,
    warm: WarmInfo,
    model_verification_seconds: f64,
}

/// Decodes one file, has the engine transcribe it, applies the glossary and
/// narrates the phases as `--progress-json` events.
fn transcribe_one(
    run: &FileRun<'_>,
    file: &Path,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let FileRun {
        args,
        language,
        glossary,
        engine,
        canceller,
        first_file,
        warm,
        model_verification_seconds,
    } = *run;
    let started = Instant::now();
    emit_progress(out, args.progress_json, started, "decoding", Some(0));
    let audio = sagascript_core::audio::decoder::decode_audio_file_with_stage_progress(
        file,
        Some(&|| canceller.check()),
        &|pct| emit_progress(out, args.progress_json, started, "decoding", Some(pct)),
        &|pct| emit_progress(out, args.progress_json, started, "resampling", Some(pct)),
    )?;
    let duration = audio.len() as f64 / 16_000.0;
    let decode_resample_seconds = started.elapsed().as_secs_f64();
    emit_progress(out, args.progress_json, started, "transcribing", Some(0));
    let result = engine.transcribe(
        &audio,
        &|pct| {
            emit_progress(out, args.progress_json, started, "transcribing", Some(pct));
        },
        canceller.flag(),
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
            model_load_seconds: if first_file { warm.load_seconds } else { 0.0 },
            model_verification_seconds: if first_file {
                model_verification_seconds
            } else {
                0.0
            },
            decode_resample_seconds,
            total_seconds: started.elapsed().as_secs_f64(),
            engine: engine.engine_name().unwrap_or_else(|| "unknown".into()),
            engine_warm: !first_file || warm.was_warm,
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
}
