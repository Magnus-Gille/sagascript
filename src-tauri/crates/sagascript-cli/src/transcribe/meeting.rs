//! Diarized meeting runs (#343): the entry points the app and the reprocessing
//! pipeline call. Their progress and cancellation control, the hashing that binds a
//! meeting document to the exact bytes it was made from and the assembly of the
//! document live in core's `meeting/` (#374).

#[cfg(feature = "diarization")]
use std::path::{Path, PathBuf};

#[cfg(feature = "diarization")]
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::source::checked_meeting_metadata;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::MeetingTranscript;
#[cfg(feature = "diarization")]
use sagascript_core::settings::{Language, Settings, WhisperModel};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::{Glossary, WhisperBackend};

#[cfg(feature = "diarization")]
use super::diarize::{CachePolicy, OnnxAnalyzer};
#[cfg(feature = "diarization")]
use super::inputs::paths_refer_to_same_file;
#[cfg(feature = "diarization")]
use super::plan::{CacheRequest, DiarizationFlags, FileOptions, FileRequest};
#[cfg(feature = "diarization")]
use super::services::Services;
#[cfg(feature = "diarization")]
use super::transcribe_file;
#[cfg(feature = "diarization")]
use crate::output::Out;

// The control, the phases and the hashing moved to core (#374); the paths of the CLI's own
// modules, and of the app until it imports core, stay.
#[cfg(feature = "diarization")]
pub(crate) use sagascript_core::meeting::assemble::meeting_transcript_from_plain_segments;
#[cfg(feature = "diarization")]
pub(super) use sagascript_core::meeting::control::meeting_checkpoint;
pub use sagascript_core::meeting::control::{MeetingControl, MeetingPhase};
#[cfg(feature = "diarization")]
pub(crate) use sagascript_core::meeting::source::{
    stable_file_sha256, verify_meeting_source_unchanged,
};

/// Run the diarized file pipeline and return the validated shared meeting
/// document without loading settings, writing settings, or emitting stdout.
#[cfg(feature = "diarization")]
pub fn transcribe_meeting_file(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
) -> Result<MeetingTranscript, DictationError> {
    transcribe_meeting_file_inner(
        file,
        stored,
        language,
        model,
        glossary,
        process_services(backend),
        None,
        &meeting_options(sagascript_core::diarization::DEFAULT_THRESHOLD, None, None),
    )
}

/// Run the diarized file pipeline with explicit progress and cancellation
/// control. The caller owns the flag and must not clear it during a job.
#[cfg(feature = "diarization")]
pub fn transcribe_meeting_file_with_control(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: &MeetingControl<'_>,
) -> Result<MeetingTranscript, DictationError> {
    control.check()?;
    transcribe_meeting_file_inner(
        file,
        stored,
        language,
        model,
        glossary,
        process_services(backend),
        Some(control),
        &meeting_options(sagascript_core::diarization::DEFAULT_THRESHOLD, None, None),
    )
}

/// Run a full diarized meeting recomputation. This path never reads an
/// existing cache. An optional `cache_output` is written complete, and only if
/// nothing exists there. It is a private staging path: the reprocessing
/// orchestration publishes the cache at the user's destination after it has
/// validated the run (#342), so this function never creates a user-visible file.
#[cfg(feature = "diarization")]
#[allow(clippy::too_many_arguments)]
pub fn transcribe_meeting_file_full(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: Option<&MeetingControl<'_>>,
    threshold: f32,
    speaker_hint: Option<sagascript_core::diarization::speaker_hint::SpeakerCountHint>,
    cache_output: Option<&Path>,
) -> Result<MeetingTranscript, DictationError> {
    if !threshold.is_finite() {
        return Err(DictationError::SettingsError(
            "diarize threshold must be a finite number".to_string(),
        ));
    }
    if !(0.0..=2.0).contains(&threshold) {
        return Err(DictationError::SettingsError(
            "diarize threshold must be between 0.0 and 2.0".to_string(),
        ));
    }
    if let Some(cache_output) = cache_output {
        if paths_refer_to_same_file(file, cache_output) {
            return Err(DictationError::SettingsError(
                "--diarize-cache must not point to the input recording".to_string(),
            ));
        }
        match std::fs::symlink_metadata(cache_output) {
            Ok(_) => {
                return Err(DictationError::FileDecodeError(format!(
                    "Diarization cache output already exists: {}",
                    cache_output.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DictationError::FileDecodeError(format!(
                    "Failed to inspect diarization cache output '{}': {error}",
                    cache_output.display()
                )))
            }
        }
    }
    if let Some(control) = control {
        control.check()?;
    }
    transcribe_meeting_file_inner(
        file,
        stored,
        language,
        model,
        glossary,
        process_services(backend),
        control,
        &meeting_options(
            threshold,
            speaker_hint,
            cache_output.map(|path| CacheRequest {
                path: path.to_path_buf(),
                policy: CachePolicy::RefreshNew,
            }),
        ),
    )
}

/// The services of a meeting job in the app or of reprocessing: the Whisper
/// backend the caller owns, the ONNX analysis and the process-wide canceller
/// (which the app never installs a handler for; its jobs cancel through their
/// `MeetingControl`).
#[cfg(feature = "diarization")]
fn process_services(backend: &WhisperBackend) -> Services<'_> {
    Services {
        backend,
        analyzer: &OnnxAnalyzer,
        canceller: &crate::cancel::GLOBAL,
    }
}

/// The options of a meeting run, which has no command line: the diarized
/// pipeline with the meeting document as its result, the given clustering
/// threshold, speaker-count hint and cache, and nothing else switched on.
#[cfg(feature = "diarization")]
pub(super) fn meeting_options(
    threshold: f32,
    speaker_hint: Option<sagascript_core::diarization::speaker_hint::SpeakerCountHint>,
    cache: Option<CacheRequest>,
) -> FileOptions {
    FileOptions {
        diarization: Some(DiarizationFlags {
            cache,
            threshold,
            speaker_count: crate::speaker_args::SpeakerCountArgs {
                speakers: speaker_hint
                    .and_then(|hint| hint.exact)
                    .filter(|_| !speaker_hint.is_some_and(|h| h.force)),
                min_speakers: speaker_hint.and_then(|hint| hint.min),
                max_speakers: speaker_hint.and_then(|hint| hint.max),
                force_speakers: speaker_hint.filter(|h| h.force).and_then(|h| h.exact),
            },
            meeting_json: true,
            ..DiarizationFlags::default()
        }),
        ..FileOptions::default()
    }
}

#[cfg(feature = "diarization")]
#[allow(clippy::too_many_arguments)]
pub(super) fn transcribe_meeting_file_inner(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    services: Services<'_>,
    control: Option<&MeetingControl<'_>>,
    options: &FileOptions,
) -> Result<MeetingTranscript, DictationError> {
    let mut model_loaded = false;
    // The pipeline's progress lines go to the process's stderr, as they did
    // before the output layer; the meeting document is the return value.
    // This private `Out` must never write stdout: it is not the run's `Out`,
    // so a stdout write here would escape its closed-pipe handling and the
    // error-object bookkeeping of `exit::conclude`. `transcribe_file` writes
    // it with `errln!` only.
    let output = transcribe_file(
        &FileRequest {
            options,
            file,
            stored,
            language,
            model,
            glossary,
            correction_vocabulary: &[],
            control,
        },
        services,
        &mut model_loaded,
        &Out::stdio(),
    )?;
    output.meeting.ok_or_else(|| {
        DictationError::TranscriptionFailed(
            "Diarized meeting transcript was not produced".to_string(),
        )
    })
}

#[cfg(feature = "diarization")]
pub(super) fn validate_meeting_input_scope(
    original_inputs: &[PathBuf],
    recursive: bool,
    expanded: &[PathBuf],
) -> Result<(), DictationError> {
    if recursive || original_inputs.iter().any(|path| path.is_dir()) || expanded.len() != 1 {
        return Err(DictationError::SettingsError(
            "--meeting-json requires one explicit regular input file; directories, --recursive, and multiple inputs are unsupported"
                .to_string(),
        ));
    }
    let metadata = std::fs::metadata(&expanded[0]).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            expanded[0].display()
        ))
    })?;
    let _ = checked_meeting_metadata(metadata, "path")?;
    Ok(())
}
