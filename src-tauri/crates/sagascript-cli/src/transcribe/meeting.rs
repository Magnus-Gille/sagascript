//! Diarized meeting runs (#343): the entry points the app and the reprocessing
//! pipeline call, their progress and cancellation control, and the hashing that
//! binds a meeting document to the exact bytes it was made from.

#[cfg(feature = "diarization")]
use std::collections::HashSet;
#[cfg(feature = "diarization")]
use std::io::Read;
#[cfg(feature = "diarization")]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "diarization")]
use std::time::SystemTime;

#[cfg(feature = "diarization")]
use sha2::{Digest, Sha256};

#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizedSegment;
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
#[cfg(feature = "diarization")]
use sagascript_core::settings::{Language, Settings, WhisperModel};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::{Glossary, WhisperBackend};

#[cfg(feature = "diarization")]
use super::args::TranscribeArgs;
#[cfg(feature = "diarization")]
use super::diarize::{CachePolicy, OnnxAnalyzer};
#[cfg(feature = "diarization")]
use super::inputs::paths_refer_to_same_file;
#[cfg(feature = "diarization")]
use super::plan::FileRequest;
#[cfg(feature = "diarization")]
use super::resolve::model_id_string;
#[cfg(feature = "diarization")]
use super::services::Services;
#[cfg(feature = "diarization")]
use super::transcribe_file;
#[cfg(feature = "diarization")]
use crate::output::Out;

#[cfg(feature = "diarization")]
const MAX_MEETING_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPhase {
    Preparing,
    Decoding,
    LoadingModel,
    Analyzing,
    Clustering,
    Finalizing,
}

pub struct MeetingControl<'a> {
    pub cancellation: &'a AtomicBool,
    pub progress: &'a (dyn Fn(MeetingPhase) + Sync),
}

impl MeetingControl<'_> {
    pub fn check(&self) -> Result<(), DictationError> {
        if self.cancellation.load(Ordering::Acquire) {
            return Err(DictationError::Cancelled);
        }
        Ok(())
    }
}

#[cfg(feature = "diarization")]
pub(super) fn meeting_checkpoint(
    control: Option<&MeetingControl<'_>>,
    phase: MeetingPhase,
) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
        (control.progress)(phase);
        control.check()?;
    }
    Ok(())
}

#[cfg(feature = "diarization")]
pub(super) fn meeting_check(control: Option<&MeetingControl<'_>>) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
}

#[cfg(feature = "diarization")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MeetingFileMetadata {
    len: u64,
    modified: SystemTime,
}

#[cfg(feature = "diarization")]
fn checked_meeting_metadata(
    metadata: std::fs::Metadata,
    context: &str,
) -> Result<MeetingFileMetadata, DictationError> {
    if !metadata.is_file() {
        return Err(DictationError::FileDecodeError(format!(
            "Meeting input {context} is not a regular file"
        )));
    }
    if metadata.len() > MAX_MEETING_INPUT_BYTES {
        return Err(DictationError::FileDecodeError(
            "Meeting input exceeds the 2 GiB hashing limit".to_string(),
        ));
    }
    let modified = metadata.modified().map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Meeting input {context} modification time is unavailable: {error}"
        ))
    })?;
    Ok(MeetingFileMetadata {
        len: metadata.len(),
        modified,
    })
}

#[cfg(feature = "diarization")]
pub(crate) fn stable_file_sha256(
    file: &Path,
    control: Option<&MeetingControl<'_>>,
) -> Result<String, DictationError> {
    const CHUNK_SIZE: usize = 64 * 1024;
    let path_before = std::fs::metadata(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let path_before = checked_meeting_metadata(path_before, "path")?;
    let mut reader = std::fs::File::open(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to read meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let handle_before = checked_meeting_metadata(
        reader.metadata().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect opened meeting input '{}': {error}",
                file.display()
            ))
        })?,
        "opened handle",
    )?;
    if path_before != handle_before {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed while opening; refusing output".to_string(),
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; CHUNK_SIZE];
    let mut bytes_read = 0_u64;
    {
        let mut limited_reader = (&mut reader).take(MAX_MEETING_INPUT_BYTES + 1);
        loop {
            meeting_check(control)?;
            let count = limited_reader.read(&mut buffer).map_err(|error| {
                DictationError::FileDecodeError(format!(
                    "Failed to read meeting input '{}': {error}",
                    file.display()
                ))
            })?;
            if count == 0 {
                break;
            }
            bytes_read = bytes_read.checked_add(count as u64).ok_or_else(|| {
                DictationError::FileDecodeError(
                    "Meeting input size exceeded safe hashing bounds".to_string(),
                )
            })?;
            hasher.update(&buffer[..count]);
            meeting_check(control)?;
        }
    }
    if bytes_read > MAX_MEETING_INPUT_BYTES {
        return Err(DictationError::FileDecodeError(
            "Meeting input exceeds the 2 GiB hashing limit".to_string(),
        ));
    }
    let handle_after = checked_meeting_metadata(
        reader.metadata().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect opened meeting input '{}': {error}",
                file.display()
            ))
        })?,
        "opened handle",
    )?;
    let path_after = std::fs::metadata(file).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            file.display()
        ))
    })?;
    let path_after = checked_meeting_metadata(path_after, "path")?;
    if bytes_read != path_before.len
        || path_before != handle_before
        || handle_before != handle_after
        || path_before != path_after
    {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed while hashing; refusing output".to_string(),
        ));
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(feature = "diarization")]
pub(crate) fn verify_meeting_source_unchanged(
    file: &Path,
    source_sha256: &str,
    control: Option<&MeetingControl<'_>>,
) -> Result<(), DictationError> {
    let after = stable_file_sha256(file, control)?;
    if after != source_sha256 {
        return Err(DictationError::FileDecodeError(
            "Meeting input changed during transcription; refusing output".to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "diarization")]
pub(crate) fn meeting_transcript_from_plain_segments(
    source_sha256: String,
    language: Language,
    model: WhisperModel,
    duration: f64,
    plain_segments: &[DiarizedSegment],
) -> Result<MeetingTranscript, DictationError> {
    let mut speakers = Vec::new();
    let mut seen = HashSet::new();
    for segment in plain_segments {
        if seen.insert(segment.speaker.clone()) {
            speakers.push(MeetingSpeaker {
                id: segment.speaker.clone(),
                label: segment.speaker.clone(),
            });
        }
    }
    let segments = plain_segments
        .iter()
        .map(|segment| MeetingSegmentInput {
            start: segment.start,
            // Whisper can timestamp the last padded window past the decoded
            // audio. Clamp only finite ends here; invalid starts/non-finite
            // values still fail the shared validator, and legacy outputs stay
            // unchanged. Do not mutate the source diarization segments.
            end: if segment.end.is_finite() && duration.is_finite() {
                segment.end.min(duration)
            } else {
                segment.end
            },
            text: segment.text.clone(),
            speaker: segment.speaker.clone(),
        })
        .collect();
    MeetingTranscript::new(
        source_sha256,
        language.whisper_code().unwrap_or("auto"),
        model_id_string(model),
        duration,
        segments,
        speakers,
    )
    .map_err(|_| {
        DictationError::TranscriptionFailed(
            "Diarized meeting segments failed shared transcript validation".to_string(),
        )
    })
}

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
        backend,
        None,
        sagascript_core::diarization::DEFAULT_THRESHOLD,
        None,
        None,
        CachePolicy::Normal,
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
        backend,
        Some(control),
        sagascript_core::diarization::DEFAULT_THRESHOLD,
        None,
        None,
        CachePolicy::Normal,
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
    speaker_hint: Option<sagascript_core::speaker_hint::SpeakerCountHint>,
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
        backend,
        control,
        threshold,
        speaker_hint,
        cache_output,
        CachePolicy::RefreshNew,
    )
}

#[cfg(feature = "diarization")]
#[allow(clippy::too_many_arguments)]
fn transcribe_meeting_file_inner(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: Option<&MeetingControl<'_>>,
    threshold: f32,
    speaker_hint: Option<sagascript_core::speaker_hint::SpeakerCountHint>,
    cache_output: Option<&Path>,
    cache_policy: CachePolicy,
) -> Result<MeetingTranscript, DictationError> {
    let args = TranscribeArgs {
        progress_json: false,
        files: vec![file.to_path_buf()],
        recursive: false,
        language: Some(language.whisper_code().unwrap_or("auto").to_string()),
        profile: None,
        model: Some(model_id_string(model).to_string()),
        json: false,
        jsonl: false,
        fail_fast: true,
        clipboard: false,
        diarize: true,
        diarize_exclusive_speech_embeddings: false,
        meeting_json: true,
        diarize_threshold: threshold,
        diarize_diagnostics: false,
        speaker_count: crate::speaker_args::SpeakerCountArgs {
            speakers: speaker_hint
                .and_then(|hint| hint.exact)
                .filter(|_| !speaker_hint.is_some_and(|h| h.force)),
            min_speakers: speaker_hint.and_then(|hint| hint.min),
            max_speakers: speaker_hint.and_then(|hint| hint.max),
            force_speakers: speaker_hint.filter(|h| h.force).and_then(|h| h.exact),
        },
        diarize_cache: cache_output.map(Path::to_path_buf),
        prompt: None,
        prompt_file: None,
        glossary_boost: None,
        boost_terms_file: None,
        correct_hints: false,
        vad: false,
        no_vad: false,
        beam_size: None,
        parallel: None,
    };
    let mut model_loaded = false;
    // The pipeline's progress lines go to the process's stderr, as they did
    // before the output layer; the meeting document is the return value.
    // This private `Out` must never write stdout: it is not the run's `Out`,
    // so a stdout write here would escape its closed-pipe handling and the
    // error-object bookkeeping of `exit::conclude`. `transcribe_file` writes
    // it with `errln!` only.
    let output = transcribe_file(
        &FileRequest {
            args: &args,
            file,
            stored,
            language,
            model,
            glossary,
            correction_vocabulary: &[],
            control,
            cache_policy,
        },
        Services {
            backend,
            analyzer: &OnnxAnalyzer,
            canceller: &crate::cancel::GLOBAL,
        },
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
