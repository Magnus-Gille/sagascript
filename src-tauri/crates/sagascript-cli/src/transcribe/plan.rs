//! What a run will do, decided before any work starts (#343).
//!
//! [`plan_run`] turns the command line and the saved settings into a
//! [`RunPlan`]: the files, the language and glossary, and which engine runs
//! them. It fails, in a fixed order, for everything that is wrong with the
//! request itself. What each file will do is decided by `file_plan`.

use std::path::PathBuf;

use sagascript_core::error::DictationError;
use sagascript_core::run_context::RunContext;
use sagascript_core::settings::{FileModel, Language, Settings, WhisperModel};
use sagascript_core::transcription::Glossary;

#[cfg(feature = "diarization")]
use super::args::speaker_hint_from_args;
use super::args::TranscribeArgs;
use super::inputs::expand_inputs;
#[cfg(feature = "diarization")]
use super::meeting::validate_meeting_input_scope;
use super::resolve::resolve_file_model_for_run;
use crate::run_request::RunRequest;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::pipeline::diarize::CachePolicy;
use sagascript_core::meeting::pipeline::inference::{not_downloaded, InferenceBackend};
#[cfg(feature = "diarization")]
use sagascript_core::meeting::pipeline::plan::{CacheRequest, DiarizationFlags};
use sagascript_core::meeting::pipeline::plan::{DecoderFlags, FileOptions};

// ---- the run ----------------------------------------------------------------------

/// What a `transcribe` invocation will do.
pub(super) struct RunPlan {
    pub(super) stored: Settings,
    pub(super) language: Language,
    pub(super) glossary: Glossary,
    pub(super) files: Vec<PathBuf>,
    pub(super) engine: EnginePlan,
}

/// Which engine runs the files, with what only that engine needs.
pub(super) enum EnginePlan {
    Pianissimo,
    Whisper {
        model: WhisperModel,
        /// The single-word hint items `--correct-hints` may repair a miss to;
        /// empty without the flag.
        correction_vocabulary: Vec<String>,
    },
}

/// Plans a run against the saved settings. `backend` is the one the run will
/// use, and its answer to which models are installed is the preflight's.
pub(super) fn plan_run(
    args: &TranscribeArgs,
    backend: &dyn InferenceBackend,
) -> Result<RunPlan, DictationError> {
    #[cfg(feature = "diarization")]
    speaker_hint_from_args(args)?;
    let stored = sagascript_core::machine::load();
    plan_run_with(args, stored, backend)
}

/// [`plan_run`] against the given settings.
///
/// The checks run in this order, and the first that fails ends the plan:
/// language and glossary, the inputs (none, `--meeting-json` scope, `--clipboard`
/// and `--diarize` with several), the model, Pianissimo's restrictions, and for
/// Whisper `--correct-hints` and the model's presence.
pub(super) fn plan_run_with(
    args: &TranscribeArgs,
    stored: Settings,
    backend: &dyn InferenceBackend,
) -> Result<RunPlan, DictationError> {
    let RunContext {
        language, glossary, ..
    } = RunRequest::from(args).resolve(&stored)?;

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

    let engine = match selected_model {
        FileModel::PianissimoOriginal => {
            #[cfg(feature = "diarization")]
            if args.diarize || args.meeting_json {
                return Err(DictationError::SettingsError(
                    "Pianissimo supports plain Swedish file transcription only; select a Whisper model for diarization or meeting output".into(),
                ));
            }
            EnginePlan::Pianissimo
        }
        FileModel::Whisper(model) => {
            let correction_vocabulary = correction_vocabulary(args, &glossary)?;
            #[cfg(feature = "diarization")]
            let cache_may_bypass_model = args.diarize && args.diarize_cache.is_some();
            #[cfg(not(feature = "diarization"))]
            let cache_may_bypass_model = false;
            if !cache_may_bypass_model && !backend.is_downloaded(model) {
                return Err(not_downloaded(model));
            }
            EnginePlan::Whisper {
                model,
                correction_vocabulary,
            }
        }
    };
    Ok(RunPlan {
        stored,
        language,
        glossary,
        files,
        engine,
    })
}

/// The vocabulary `--correct-hints` repairs towards, or none without the flag.
fn correction_vocabulary(
    args: &TranscribeArgs,
    glossary: &Glossary,
) -> Result<Vec<String>, DictationError> {
    if !args.correct_hints {
        return Ok(Vec::new());
    }
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
    Ok(vocabulary)
}

/// Narrows the command line to what the pipelines of one file read. A command line run always
/// uses `CachePolicy::Normal`.
pub(super) fn file_options(args: &TranscribeArgs) -> FileOptions {
    FileOptions {
        progress_json: args.progress_json,
        correct_hints: args.correct_hints,
        decoder: DecoderFlags {
            vad: args.vad,
            no_vad: args.no_vad,
            beam_size: args.beam_size,
            parallel: args.parallel,
        },
        #[cfg(feature = "diarization")]
        diarization: args.diarize.then(|| DiarizationFlags {
            cache: args.diarize_cache.clone().map(|path| CacheRequest {
                path,
                policy: CachePolicy::Normal,
            }),
            exclusive_speech_embeddings: args.diarize_exclusive_speech_embeddings,
            threshold: args.diarize_threshold,
            diagnostics: args.diarize_diagnostics,
            speaker_count: args.speaker_count.into(),
            meeting_json: args.meeting_json,
        }),
    }
}
