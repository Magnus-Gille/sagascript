//! One file of a run: what it will do, decided before any work starts (#343).
//!
//! [`plan_file`] fixes the mode (plain or diarized) and, for a diarized run, the decoder, the
//! source hash and the progress snapshot. [`FileRequest`] is one input with everything it is
//! told, and [`FileOptions`] is what the command line says about it, narrowed by the caller.

use std::path::Path;
#[cfg(feature = "diarization")]
use std::path::PathBuf;
use std::time::Instant;

#[cfg(feature = "diarization")]
use sagascript_core::diarization::report::DecoderEvidence;
#[cfg(feature = "diarization")]
use sagascript_core::diarization::speaker_hint::SpeakerCountFlags;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, Settings, WhisperModel};
use sagascript_core::transcription::Glossary;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::TranscribeOptions;

#[cfg(feature = "diarization")]
use super::diarize::CachePolicy;
#[cfg(feature = "diarization")]
use super::diarize::{effective_diarization_decoder_evidence, resolve_diarization_options};
#[cfg(feature = "diarization")]
use super::progress::decoder_progress_value;
use sagascript_core::meeting::control::MeetingControl;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::control::{meeting_checkpoint, MeetingPhase};
#[cfg(feature = "diarization")]
use sagascript_core::meeting::source::stable_file_sha256;

/// One input of a run, with everything its processing is told. Built by the
/// batch (one per file) and by the meeting entry points.
pub(super) struct FileRequest<'a> {
    pub(super) options: &'a FileOptions,
    pub(super) file: &'a Path,
    pub(super) stored: &'a Settings,
    pub(super) language: Language,
    pub(super) model: WhisperModel,
    pub(super) glossary: &'a Glossary,
    pub(super) correction_vocabulary: &'a [String],
    /// Cancellation and phase reporting of a meeting job; `None` on the CLI.
    pub(super) control: Option<&'a MeetingControl<'a>>,
}

/// What a file's pipelines read of the command line, and nothing else (#343).
///
/// The batch narrows `TranscribeArgs` into it once ([`FileOptions::from_args`]);
/// the meeting entry points, which have no command line, build the few options
/// they use directly.
#[derive(Clone, Debug, Default)]
pub(super) struct FileOptions {
    /// `--progress-json`.
    pub(super) progress_json: bool,
    /// `--correct-hints`: a plain run repairs words towards the hint, a
    /// diarized run refuses it.
    pub(super) correct_hints: bool,
    pub(super) decoder: DecoderFlags,
    /// `--diarize` and what goes with it; `None` is a plain run.
    #[cfg(feature = "diarization")]
    pub(super) diarization: Option<DiarizationFlags>,
}

/// `--vad`, `--no-vad`, `--beam` and `--parallel`.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DecoderFlags {
    pub(super) vad: bool,
    pub(super) no_vad: bool,
    pub(super) beam_size: Option<u32>,
    pub(super) parallel: Option<usize>,
}

/// What a diarized run reads of the command line.
#[cfg(feature = "diarization")]
#[derive(Clone, Debug)]
pub(super) struct DiarizationFlags {
    /// `--diarize-cache` and what the run does with it; `None` is no cache.
    pub(super) cache: Option<CacheRequest>,
    /// `--diarize-exclusive-speech-embeddings`.
    pub(super) exclusive_speech_embeddings: bool,
    /// `--diarize-threshold`.
    pub(super) threshold: f32,
    /// `--diarize-diagnostics`.
    pub(super) diagnostics: bool,
    /// `--speakers`, `--min-speakers`, `--max-speakers`, `--force-speakers`,
    /// validated by the run that uses them.
    pub(super) speaker_count: SpeakerCountFlags,
    /// `--meeting-json`: the result is the shared meeting document.
    pub(super) meeting_json: bool,
}

#[cfg(feature = "diarization")]
impl Default for DiarizationFlags {
    fn default() -> Self {
        Self {
            cache: None,
            exclusive_speech_embeddings: false,
            threshold: sagascript_core::diarization::DEFAULT_THRESHOLD,
            diagnostics: false,
            speaker_count: SpeakerCountFlags::default(),
            meeting_json: false,
        }
    }
}

/// A diarization cache a run may read and, on a miss, writes: where it is and
/// which policy governs it.
#[cfg(feature = "diarization")]
#[derive(Clone, Debug)]
pub(super) struct CacheRequest {
    pub(super) path: PathBuf,
    pub(super) policy: CachePolicy,
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
    /// What the diarize flags say.
    pub(super) flags: DiarizationFlags,
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
    let mode = match &request.options.diarization {
        Some(flags) => {
            let plan = DiarizationPlan::new(request, flags)?;
            if flags
                .cache
                .as_ref()
                .is_some_and(|cache| paths_refer_to_same_file(request.file, &cache.path))
            {
                return Err(DictationError::SettingsError(
                    "--diarize-cache must not point to the input recording".to_string(),
                ));
            }
            FileMode::Diarized(Box::new(plan))
        }
        None => FileMode::Plain,
    };
    #[cfg(not(feature = "diarization"))]
    let mode = FileMode::Plain;
    Ok(FilePlan { started, mode })
}

#[cfg(feature = "diarization")]
impl DiarizationPlan {
    fn new(request: &FileRequest<'_>, flags: &DiarizationFlags) -> Result<Self, DictationError> {
        let FileRequest {
            options,
            file,
            glossary,
            control,
            ..
        } = *request;
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        let source_sha256 = stable_file_sha256(file, control)?;
        meeting_checkpoint(control, MeetingPhase::Preparing)?;
        let decoder_prompt = glossary.decoder_prompt();
        let decoder_options = resolve_diarization_options(&options.decoder)?;
        let decoder_evidence = effective_diarization_decoder_evidence(&decoder_options)?;
        let decoder_progress = decoder_progress_value(&decoder_evidence, flags.meeting_json);
        Ok(Self {
            source_sha256,
            decoder_prompt,
            options: decoder_options,
            decoder_evidence,
            decoder_progress,
            flags: flags.clone(),
        })
    }
}

/// Whether `left` and `right` are the same file: the same path once resolved, or on Unix the
/// same device and inode (a hard link).
#[cfg(feature = "diarization")]
pub(super) fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        if left == right {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) {
            return left.dev() == right.dev() && left.ino() == right.ino();
        }
    }
    false
}
