//! `sagascript transcribe`: one file or a batch through Whisper (or Pianissimo),
//! with optional diarization (#343).
//!
//! `run` plans the command ([`plan`]) and hands it to an engine: the Whisper
//! batch below, or the Pianissimo one. `transcribe_file` does one input: it
//! plans the file and delegates to the plain or the diarized pipeline, whose
//! steps are modules of their own:
//!
//! - `args`: the command line (`TranscribeArgs`) and its value parsers;
//! - `plan`: what a run and each file will do, decided before any work starts;
//! - `inputs`: expanding directories into the files to work on;
//! - `resolve`: language and model IDs, which model a run gets, the one-run glossary;
//! - `source`: where the audio comes from: decoding, or a diarization cache hit;
//! - `inference`: the Whisper backend as a run uses it (`InferenceBackend`), model
//!   loading and the language check that follows;
//! - `plain`, `diarized`: the two pipelines of one file;
//! - `vocab_correction`: glossary aliases and the opt-in one-edit hint repair;
//! - `language_detection`, `language_regions`: the language check and the sampled
//!   region detection, with their diagnostics;
//! - `services`: the collaborators a file run is handed (the Whisper backend and,
//!   with diarization, the speaker analysis), which a test replaces;
//! - `diarize`: the diarization cache policy, the speaker analysis seam, decoder
//!   options and concurrent analysis;
//! - `meeting`: the meeting entry points the app and reprocessing call, with their
//!   cancellation control and source hashing;
//! - `pianissimo`: the Pianissimo (engine host) path;
//! - `progress`: `--progress-json` events;
//! - `output`: the per-file result, the three output modes, warnings and the clipboard.
//!
//! The paths other code imports (`sagascript_cli::transcribe::*`) are
//! re-exported below.

use std::path::{Path, PathBuf};
use std::time::Instant;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, Settings, WhisperModel};
use sagascript_core::transcription::{Glossary, WhisperBackend};

use crate::output::{errln, Out};

mod args;
mod diarize;
#[cfg(feature = "diarization")]
mod diarized;
mod inference;
mod inputs;
mod language_detection;
mod language_regions;
mod meeting;
mod output;
mod pianissimo;
mod plain;
mod plan;
mod progress;
mod resolve;
mod services;
mod source;
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
use self::diarize::{CachePolicy, OnnxAnalyzer};
#[cfg(feature = "diarization")]
use self::diarized::run_diarized;
use self::output::{BatchExecution, BatchSink, FileTranscription};
use self::pianissimo::{run_pianissimo_batch, InstalledPianissimo, PianissimoBatch};
use self::plain::run_plain;
use self::plan::{plan_file, plan_run, EnginePlan, FileMode, FileRequest, RunPlan};
use self::progress::emit_progress;
use self::services::Services;

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
    let RunPlan {
        stored,
        language,
        glossary,
        files,
        engine,
    } = plan_run(&args)?;
    match engine {
        EnginePlan::Pianissimo => run_pianissimo_batch(
            &PianissimoBatch {
                args: &args,
                files: &files,
                language,
                glossary: &glossary,
            },
            &InstalledPianissimo,
            &crate::cancel::GLOBAL,
            out,
        ),
        EnginePlan::Whisper {
            model,
            correction_vocabulary,
        } => {
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
                Services {
                    backend: &*backend,
                    #[cfg(feature = "diarization")]
                    analyzer: &OnnxAnalyzer,
                },
                &crate::cancel::GLOBAL,
                out,
            )
        }
    }
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

/// Transcribes the files one after another with `services`, writing each
/// outcome as it is done and the batch's summary at the end.
///
/// Model loading is lazy so a validated diarization-cache hit never maps
/// several gigabytes of weights that it will not use. Batch processing is
/// sequential, so one flag safely preserves the load-once behavior.
fn run_whisper_batch(
    batch: &WhisperBatch<'_>,
    services: Services<'_>,
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
                &FileRequest {
                    args,
                    file,
                    stored,
                    language,
                    model,
                    glossary,
                    correction_vocabulary,
                    control: None,
                    #[cfg(feature = "diarization")]
                    cache_policy: CachePolicy::Normal,
                },
                services,
                &mut model_loaded,
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

/// Transcribes one input: plans the file, then runs the pipeline its mode
/// calls for. `model_loaded` is the batch's memory of whether the model is
/// resident.
fn transcribe_file(
    request: &FileRequest<'_>,
    services: Services<'_>,
    model_loaded: &mut bool,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let plan = plan_file(request)?;
    match plan.mode {
        #[cfg(feature = "diarization")]
        FileMode::Diarized(diarization) => run_diarized(
            request,
            plan.started,
            *diarization,
            services,
            model_loaded,
            out,
        ),
        FileMode::Plain => run_plain(request, plan.started, services.backend, model_loaded, out),
    }
}

#[cfg(test)]
#[path = "../transcribe_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../transcribe_file_tests.rs"]
mod file_tests;
