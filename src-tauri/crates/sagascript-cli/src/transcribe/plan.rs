//! What a run and each of its files will do, decided before any work starts
//! (#343).
//!
//! [`plan_run`] turns the command line and the saved settings into a
//! [`RunPlan`]: the files, the language and glossary, and which engine runs
//! them. It fails, in a fixed order, for everything that is wrong with the
//! request itself. [`plan_file`] does the same for one input: it fixes the mode
//! (plain or diarized) and, for a diarized run, the decoder, the source hash
//! and the progress snapshot.

use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(feature = "diarization")]
use sagascript_core::diarization_report::DecoderEvidence;
use sagascript_core::error::DictationError;
use sagascript_core::run_context::RunContext;
use sagascript_core::settings::{FileModel, Language, Settings, WhisperModel};
use sagascript_core::transcription::Glossary;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::TranscribeOptions;

#[cfg(feature = "diarization")]
use super::args::speaker_hint_from_args;
use super::args::TranscribeArgs;
#[cfg(feature = "diarization")]
use super::diarize::CachePolicy;
#[cfg(feature = "diarization")]
use super::diarize::{effective_diarization_decoder_evidence, resolve_diarization_options};
use super::inference::{not_downloaded, InferenceBackend};
use super::inputs::expand_inputs;
#[cfg(feature = "diarization")]
use super::inputs::paths_refer_to_same_file;
use super::meeting::MeetingControl;
#[cfg(feature = "diarization")]
use super::meeting::MeetingPhase;
#[cfg(feature = "diarization")]
use super::meeting::{meeting_checkpoint, stable_file_sha256, validate_meeting_input_scope};
#[cfg(feature = "diarization")]
use super::progress::decoder_progress_value;
use super::resolve::resolve_file_model_for_run;
use crate::run_request::RunRequest;

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
    let stored = sagascript_core::settings::store::load();
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

// ---- one file ---------------------------------------------------------------------

/// One input of a run, with everything its processing is told. Built by the
/// batch (one per file) and by the meeting entry points.
pub(super) struct FileRequest<'a> {
    pub(super) args: &'a TranscribeArgs,
    pub(super) file: &'a Path,
    pub(super) stored: &'a Settings,
    pub(super) language: Language,
    pub(super) model: WhisperModel,
    pub(super) glossary: &'a Glossary,
    pub(super) correction_vocabulary: &'a [String],
    /// Cancellation and phase reporting of a meeting job; `None` on the CLI.
    pub(super) control: Option<&'a MeetingControl<'a>>,
    /// What a diarized run does with `--diarize-cache`; a plain run has no cache.
    #[cfg(feature = "diarization")]
    pub(super) cache_policy: CachePolicy,
}

/// How one file will be processed.
pub(super) struct FilePlan {
    /// When the file's work began; every `elapsed_ms` counts from here.
    pub(super) started: Instant,
    pub(super) mode: FileMode,
}

pub(super) enum FileMode {
    Plain,
    #[cfg(feature = "diarization")]
    Diarized(Box<DiarizationPlan>),
}

/// What a diarized run fixes before it looks at the audio.
#[cfg(feature = "diarization")]
pub(super) struct DiarizationPlan {
    /// SHA-256 of the source, taken (and checked again at the end) so the
    /// meeting document is bound to the bytes it was made from.
    pub(super) source_sha256: String,
    /// The glossary's decoder prompt, part of the cache identity.
    pub(super) decoder_prompt: Option<String>,
    /// The decoder settings the diarized path uses.
    pub(super) options: TranscribeOptions,
    pub(super) decoder_evidence: DecoderEvidence,
    /// `decoder_evidence` as it is attached to every progress event.
    pub(super) decoder_progress: serde_json::Value,
}

/// Plans one file. For a diarized run this hashes the source, resolves the
/// decoder and checks `--diarize-cache` does not point at the recording, in
/// that order.
pub(super) fn plan_file(request: &FileRequest<'_>) -> Result<FilePlan, DictationError> {
    let started = Instant::now();
    // A plain-only build has no mode to choose.
    #[cfg(not(feature = "diarization"))]
    let _ = request;
    #[cfg(feature = "diarization")]
    let mode = {
        let diarization = request
            .args
            .diarize
            .then(|| DiarizationPlan::new(request))
            .transpose()?;
        if request
            .args
            .diarize_cache
            .as_deref()
            .is_some_and(|cache_path| paths_refer_to_same_file(request.file, cache_path))
        {
            return Err(DictationError::SettingsError(
                "--diarize-cache must not point to the input recording".to_string(),
            ));
        }
        match diarization {
            Some(diarization) => FileMode::Diarized(Box::new(diarization)),
            None => FileMode::Plain,
        }
    };
    #[cfg(not(feature = "diarization"))]
    let mode = FileMode::Plain;
    Ok(FilePlan { started, mode })
}

#[cfg(feature = "diarization")]
impl DiarizationPlan {
    fn new(request: &FileRequest<'_>) -> Result<Self, DictationError> {
        let FileRequest {
            args,
            file,
            stored,
            glossary,
            control,
            ..
        } = *request;
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        let source_sha256 = stable_file_sha256(file, control)?;
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        let decoder_prompt = glossary.decoder_prompt();
        let options = resolve_diarization_options(args, stored)?;
        let decoder_evidence = effective_diarization_decoder_evidence(&options)?;
        let decoder_progress = decoder_progress_value(&decoder_evidence, args.meeting_json);
        Ok(Self {
            source_sha256,
            decoder_prompt,
            options,
            decoder_evidence,
            decoder_progress,
        })
    }
}
