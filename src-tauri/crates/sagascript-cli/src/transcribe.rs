use std::collections::HashSet;
#[cfg(feature = "diarization")]
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
#[cfg(feature = "diarization")]
use std::time::SystemTime;

use clap::Args;
#[cfg(feature = "diarization")]
use sha2::{Digest, Sha256};

use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::decoder::{decode_audio_file, SUPPORTED_EXTENSIONS};
#[cfg(feature = "diarization")]
use sagascript_core::audio::decoder::decode_audio_file_with_control;
#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizedSegment;
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
use sagascript_core::settings::{HotkeyProfile, Language, Settings, WhisperModel};
use sagascript_core::transcription::diagnostics::{
    analyze_coverage, analyze_language_windows, analyze_repetition, language_mismatch_warning,
    CoverageDiagnostics, LanguageDetection, LanguageRegionDiagnostics, LanguageWindow,
    TranscriptionWarning,
};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::model;
use sagascript_core::transcription::whisper_backend::assemble_transcript;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::contains_no_speech_marker;
use sagascript_core::transcription::{
    normalize_nonspeech_markers, recommended_parallel_chunks, ContextProfile, Glossary,
    TranscribeOptions, TranscriptSegment, WhisperBackend,
};

#[derive(Args)]
pub struct TranscribeArgs {
    /// Audio/video files or directories to transcribe. Directories include
    /// supported files in deterministic filename order.
    #[arg(required = true, value_name = "INPUT")]
    pub files: Vec<PathBuf>,

    /// Recurse into input directories. Has no effect on explicit files.
    #[arg(long)]
    pub recursive: bool,

    /// Language for transcription [possible values: en, sv, no, fi, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// Whisper model ID to use [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Output result as JSON: text, language, model, duration, and a
    /// `segments` array with per-segment timing/confidence plus
    /// `coverage_ratio`, `uncovered_spans`, repetition quarantine spans,
    /// sampled language regions/probabilities, and warnings. Raw segments
    /// include `quarantined`.
    #[arg(long)]
    pub json: bool,

    /// Emit one compact {source,status,result|error} JSON object per input.
    #[arg(long, conflicts_with = "json")]
    pub jsonl: bool,

    /// Stop after the first item-level failure. The default continues and
    /// exits non-zero after emitting every successful/failed item.
    #[arg(long)]
    pub fail_fast: bool,

    /// Copy transcription result to clipboard
    #[arg(long)]
    pub clipboard: bool,

    /// Enable speaker diarization (requires diarization models — run: sagascript download-model diarization)
    #[cfg(feature = "diarization")]
    #[arg(long)]
    pub diarize: bool,

    /// Emit the validated shared meeting transcript JSON document.
    /// Requires diarization and cannot be combined with legacy JSON outputs.
    #[cfg(feature = "diarization")]
    #[arg(
        long,
        requires = "diarize",
        conflicts_with_all = ["json", "jsonl", "clipboard", "correct_hints"]
    )]
    pub meeting_json: bool,

    /// Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.75). Higher = fewer speakers.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "THRESHOLD", default_value = "0.75",
          value_parser = parse_diarize_threshold,
          help = "Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.75). Higher = fewer speakers.")]
    pub diarize_threshold: f32,

    /// Read/write reusable threshold-independent diarization analysis and
    /// word timestamps. A matching cache makes threshold-only retries fast.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "PATH", requires = "diarize")]
    pub diarize_cache: Option<PathBuf>,

    /// Hint the decoder with domain-specific vocabulary (Whisper initial prompt).
    /// Reduces mishearings of proper nouns, foreign names, and jargon by priming
    /// the model with likely terms.
    /// Example: --hint "Notre Dame, Sara, Estrid, Grimnir, MCP, Fortnox"
    #[arg(long, visible_alias = "hint", value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Read the hint/initial prompt from a file instead of the command line
    /// (handy for longer vocabulary lists). Mutually exclusive with --hint/--prompt.
    /// Leading/trailing whitespace is trimmed; an empty file means no hint.
    #[arg(
        long,
        visible_alias = "hint-file",
        value_name = "PATH",
        conflicts_with = "prompt"
    )]
    pub prompt_file: Option<PathBuf>,

    /// Opt in to strict one-edit vocabulary correction from the selected hint.
    /// Requires --json so every applied correction is reported with its source
    /// text and segment confidence. Only single-word, unambiguous hints apply.
    #[arg(long, requires = "json")]
    pub correct_hints: bool,

    /// Enable voice activity detection (Silero VAD) to skip non-speech regions,
    /// reducing silence hallucination and repetition loops. Downloads a small
    /// model on first use. Overrides the `vad_enabled` setting.
    #[arg(long)]
    pub vad: bool,

    /// Disable VAD for this run, even if the `vad_enabled` setting is on.
    #[arg(long, conflicts_with = "vad")]
    pub no_vad: bool,

    /// Beam search width: 0 = greedy (fast), >=2 = beam search (more accurate,
    /// slower). Overrides the saved `beam_size` setting. When omitted, a saved
    /// `beam_size` >=2 is used; otherwise file transcription defaults to 5
    /// (pass --beam 0 to force greedy).
    #[arg(long = "beam", value_name = "N")]
    pub beam_size: Option<u32>,

    /// Split long files across this many concurrent Whisper states (1–4).
    /// When omitted, long beam-search files use two states for measured-safe speedup.
    #[arg(
        long,
        value_name = "N",
        value_parser = parse_parallel_chunks
    )]
    #[cfg_attr(feature = "diarization", arg(conflicts_with = "diarize"))]
    pub parallel: Option<usize>,
}

#[derive(Debug)]
struct FileTranscription {
    json: serde_json::Value,
    plain: String,
    #[cfg(feature = "diarization")]
    meeting: Option<MeetingTranscript>,
}

#[cfg(feature = "diarization")]
fn assemble_diarized_plain_text(segments: &[DiarizedSegment]) -> String {
    segments
        .iter()
        .filter(|segment| !contains_no_speech_marker(&segment.text))
        .map(|segment| format!("[{}] {}", segment.speaker, segment.text.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(feature = "diarization")]
fn prepare_diarized_plain_segments(
    segments: &[DiarizedSegment],
    language: Language,
    glossary: &Glossary,
) -> Vec<DiarizedSegment> {
    let filtered = segments
        .iter()
        .filter(|segment| !contains_no_speech_marker(&segment.text))
        .cloned()
        .collect::<Vec<_>>();
    let mut consolidated = sagascript_core::diarization::merge::consolidate(&filtered);
    for segment in &mut consolidated {
        segment.text = normalize_nonspeech_markers(&segment.text, language);
    }
    let fragments = consolidated
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>();
    let (corrected_fragments, _) = glossary.correct_fragments(&fragments);
    for (segment, text) in consolidated.iter_mut().zip(corrected_fragments) {
        segment.text = text;
    }
    consolidated
}

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
            return Err(DictationError::TranscriptionFailed(
                "Meeting transcription cancelled".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(feature = "diarization")]
fn meeting_checkpoint(
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
fn meeting_check(control: Option<&MeetingControl<'_>>) -> Result<(), DictationError> {
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
fn stable_file_sha256(
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
fn verify_meeting_source_unchanged(
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
fn meeting_transcript_from_plain_segments(
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

#[cfg(feature = "diarization")]
#[derive(Debug, Default, serde::Serialize)]
struct DiarizationPerformance {
    acceleration_backend: &'static str,
    coreml_status: &'static str,
    cache_hit: bool,
    model_load_seconds: f64,
    decode_resample_seconds: f64,
    language_detection_seconds: f64,
    cache_lookup_seconds: f64,
    diarization_model_load_seconds: f64,
    diarization_segmentation_seconds: f64,
    diarization_segment_extraction_seconds: f64,
    diarization_embeddings_seconds: f64,
    diarization_clustering_seconds: f64,
    whisper_inference_seconds: f64,
    word_timestamp_attribution_seconds: f64,
    parallel_analysis_span_seconds: f64,
    cache_write_seconds: f64,
    merge_diagnostics_seconds: f64,
    json_assembly_seconds: f64,
    total_seconds: f64,
}

#[derive(Debug)]
struct BatchExecution {
    source: PathBuf,
    output: Result<FileTranscription, String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum BatchItem {
    Ok {
        source: String,
        result: serde_json::Value,
    },
    Error {
        source: String,
        error: String,
    },
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
    transcribe_meeting_file_inner(file, stored, language, model, glossary, backend, None)
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
    )
}

#[cfg(feature = "diarization")]
fn transcribe_meeting_file_inner(
    file: &Path,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    glossary: &Glossary,
    backend: &WhisperBackend,
    control: Option<&MeetingControl<'_>>,
) -> Result<MeetingTranscript, DictationError> {
    let args = TranscribeArgs {
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
        meeting_json: true,
        diarize_threshold: 0.75,
        diarize_cache: None,
        prompt: None,
        prompt_file: None,
        correct_hints: false,
        vad: false,
        no_vad: false,
        beam_size: None,
        parallel: None,
    };
    let mut model_loaded = false;
    let output = transcribe_file(
        &args,
        file,
        backend,
        stored,
        language,
        model,
        &mut model_loaded,
        glossary,
        &[],
        control,
    )?;
    output.meeting.ok_or_else(|| {
        DictationError::TranscriptionFailed(
            "Diarized meeting transcript was not produced".to_string(),
        )
    })
}

#[cfg(feature = "diarization")]
fn validate_meeting_input_scope(
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

pub fn run(args: TranscribeArgs) -> Result<(), DictationError> {
    let stored = sagascript_core::settings::store::load();
    let profile = args
        .profile
        .as_deref()
        .map(|profile_id| resolve_profile(&stored, profile_id))
        .transpose()?;
    let language = match (&profile, &args.language) {
        (Some(profile), _) => profile.language,
        (None, Some(language)) => parse_language(language)?,
        (None, None) => stored.language,
    };

    let glossary = effective_glossary(
        &stored,
        args.profile.as_deref(),
        args.prompt.as_deref(),
        args.prompt_file.as_deref(),
    )?;

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
    let model = resolve_effective_model(
        args.model.as_deref(),
        language,
        stored.auto_select_model,
        stored.whisper_model,
    )?;

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

    // Model loading is lazy so a validated diarization-cache hit never maps
    // several gigabytes of weights that it will not use. Batch processing is
    // sequential, so this flag safely preserves the load-once behavior.
    let backend = WhisperBackend::new();
    let mut model_loaded = false;

    let mut items = Vec::with_capacity(if args.json { files.len() } else { 0 });
    let (processed, failures) = process_batch(
        &files,
        args.fail_fast,
        |index, file| {
            eprintln!("[{}/{}] {}", index + 1, files.len(), file.display());
            transcribe_file(
                &args,
                file,
                &backend,
                &stored,
                language,
                model,
                &mut model_loaded,
                &glossary,
                &correction_vocabulary,
                None,
            )
        },
        |execution| {
            let file = execution.source;
            match execution.output {
                Ok(output) => {
                    #[cfg(feature = "diarization")]
                    if args.meeting_json {
                        let meeting = output.meeting.as_ref().ok_or_else(|| {
                            DictationError::TranscriptionFailed(
                                "meeting JSON was not produced by the diarized pipeline"
                                    .to_string(),
                            )
                        })?;
                        println!(
                            "{}",
                            serde_json::to_string_pretty(meeting).expect("meeting serializes")
                        );
                        return Ok(());
                    }
                    if args.clipboard {
                        copy_to_clipboard(&output.plain)?;
                        eprintln!("Copied to clipboard.");
                    }
                    let item = BatchItem::Ok {
                        source: file.display().to_string(),
                        result: output.json,
                    };
                    if args.jsonl {
                        emit_jsonl_item(&item)?;
                    } else if args.json {
                        items.push(item);
                    } else {
                        if files.len() > 1 {
                            println!("==> {} <==", file.display());
                        }
                        println!("{}", output.plain);
                    }
                }
                Err(error) => {
                    eprintln!("Error transcribing {}: {error}", file.display());
                    let item = BatchItem::Error {
                        source: file.display().to_string(),
                        error,
                    };
                    if args.jsonl {
                        emit_jsonl_item(&item)?;
                    } else if args.json {
                        items.push(item);
                    }
                }
            }
            Ok(())
        },
    )?;

    if args.json {
        if files.len() == 1 && failures == 0 {
            let BatchItem::Ok { result, .. } = &items[0] else {
                unreachable!("successful single item")
            };
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("result serializes")
            );
        } else {
            println!(
                "{}",
                serde_json::to_string_pretty(&items).expect("batch serializes")
            );
        }
    }

    if failures > 0 {
        return Err(DictationError::TranscriptionFailed(format!(
            "{failures} of {} batch item(s) failed",
            processed
        )));
    }
    Ok(())
}

fn emit_jsonl_item(item: &BatchItem) -> Result<(), DictationError> {
    println!(
        "{}",
        serde_json::to_string(item).expect("batch item serializes")
    );
    std::io::stdout().flush().map_err(|error| {
        DictationError::TranscriptionFailed(format!("Failed to flush JSONL output: {error}"))
    })
}

fn process_batch<F, E>(
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
        let output = process(index, file).map_err(|error| error.to_string());
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

fn ensure_model_loaded(
    backend: &WhisperBackend,
    model: WhisperModel,
    profile: ContextProfile,
    loaded: &mut bool,
) -> Result<f64, DictationError> {
    if *loaded {
        return Ok(0.0);
    }
    if !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }
    eprintln!("Loading model: {}...", model.display_name());
    let started = Instant::now();
    backend.load_model_with_profile(model, profile)?;
    *loaded = true;
    Ok(started.elapsed().as_secs_f64())
}

#[cfg(feature = "diarization")]
fn paths_refer_to_same_file(left: &Path, right: &Path) -> bool {
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

#[allow(clippy::too_many_arguments)]
fn transcribe_file(
    args: &TranscribeArgs,
    file: &Path,
    backend: &WhisperBackend,
    stored: &Settings,
    language: Language,
    model: WhisperModel,
    model_loaded: &mut bool,
    glossary: &Glossary,
    correction_vocabulary: &[String],
    control: Option<&MeetingControl<'_>>,
) -> Result<FileTranscription, DictationError> {
    #[cfg(not(feature = "diarization"))]
    let _ = control;

    let file_started = Instant::now();
    #[cfg(feature = "diarization")]
    let meeting_source_sha256 = if args.meeting_json {
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
        Some(crate::diarization_cache::CacheIdentity::for_input(
            file,
            language.whisper_code().unwrap_or("auto"),
            model_id_string(model),
            decoder_prompt.as_deref(),
        )?)
    } else {
        None
    };
    #[cfg(feature = "diarization")]
    let cached = match (args.diarize_cache.as_deref(), cache_identity.as_ref()) {
        (Some(path), Some(identity)) => match crate::diarization_cache::load(path, identity)? {
            crate::diarization_cache::CacheLookup::Hit(cached) => {
                eprintln!("Reusing diarization cache: {}", path.display());
                Some(cached)
            }
            crate::diarization_cache::CacheLookup::Miss(reason) => {
                eprintln!("Diarization cache miss ({reason}); computing analysis.");
                None
            }
        },
        _ => None,
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
            eprintln!("Audio (cached): {duration:.1}s");
            (None, 0.0, duration)
        }
        #[cfg(not(feature = "diarization"))]
        unreachable!("cache hits require the diarization feature")
    } else {
        eprintln!("Decoding {}...", file.display());
        let decode_started = Instant::now();
        #[cfg(feature = "diarization")]
        let audio = if args.meeting_json {
                let checkpoint = || meeting_check(control);
                decode_audio_file_with_control(file, &checkpoint)?
        } else {
            decode_audio_file(file)?
        };
        #[cfg(not(feature = "diarization"))]
        let audio = decode_audio_file(file)?;
        let decode_resample_seconds = decode_started.elapsed().as_secs_f64();
        let duration = audio.len() as f64 / 16_000.0;
        eprintln!("Audio: {:.1}s, {} samples", duration, audio.len());
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
            let model_load_seconds =
                ensure_model_loaded(backend, model, context_profile, model_loaded)?;
            let language_detection_started = Instant::now();
            let audio = audio.as_deref().expect("cache misses decode audio");
            let detected_language = match detect_file_language(backend, audio, model) {
                Ok(detection) => detection,
                Err(error) => {
                    eprintln!("Warning: local language detection was unavailable: {error}");
                    None
                }
            };
            let language_regions = match detect_file_language_regions(backend, audio, language) {
                Ok(diagnostics) => diagnostics,
                Err(error) => {
                    eprintln!("Warning: language region detection was unavailable: {error}");
                    None
                }
            };
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
        // The diarized path uses greedy timestamped decoding (DTW), so the
        // beam/VAD options don't apply — warn rather than silently ignore them.
        if args.beam_size.is_some() || args.vad || args.no_vad {
            eprintln!("Note: --beam / --vad have no effect with --diarize.");
        }
        use sagascript_core::diarization::{
            cluster,
            merge::{consolidate, merge_with_transcript},
            DiarizationTimings, DiarizeConfig, TimestampedSegment,
        };
        use sagascript_core::transcription::DiarizationTranscriptionTimings;

        let acceleration = model::acceleration_profile(model);
        let mut performance = DiarizationPerformance {
            acceleration_backend: acceleration.backend,
            coreml_status: acceleration.coreml_status,
            model_load_seconds,
            decode_resample_seconds,
            language_detection_seconds,
            ..DiarizationPerformance::default()
        };
        performance.cache_lookup_seconds = cache_lookup_seconds;
        eprintln!(
            "Whisper acceleration: {} (Core ML: {})",
            acceleration.backend, acceleration.coreml_status
        );

        let config = DiarizeConfig {
            threshold: args.diarize_threshold,
            ..DiarizeConfig::default()
        };

        let (analysis, raw_segments, diarization_timings, transcription_timings) =
            if let Some(cached) = cached {
                performance.cache_hit = true;
                let cached = *cached;
                (
                    cached.analysis,
                    cached.transcript,
                    DiarizationTimings::default(),
                    DiarizationTranscriptionTimings::default(),
                )
            } else {
                eprintln!("Running speaker diarization and Whisper timestamps concurrently...");
                let parallel_started = Instant::now();
                let audio = audio.as_deref().expect("cache misses decode audio");
                meeting_checkpoint(control, MeetingPhase::Analyzing)?;
                let (analysis, diarization_timings, transcription) = run_diarization_analysis(
                    audio,
                    backend,
                    language,
                    decoder_prompt.as_deref(),
                    &config,
                )?;
                meeting_checkpoint(control, MeetingPhase::Analyzing)?;
                performance.parallel_analysis_span_seconds =
                    parallel_started.elapsed().as_secs_f64();

                if let (Some(path), Some(identity)) =
                    (args.diarize_cache.as_deref(), cache_identity.clone())
                {
                    let cache_write_started = Instant::now();
                    crate::diarization_cache::save(
                        path,
                        &crate::diarization_cache::DiarizationCache::new(
                            identity,
                            analysis.clone(),
                            transcription.segments.clone(),
                            coverage_profile.clone(),
                            detected_language.clone(),
                            language_regions.clone(),
                        ),
                    )?;
                    performance.cache_write_seconds = cache_write_started.elapsed().as_secs_f64();
                    eprintln!("Saved reusable diarization cache: {}", path.display());
                }

                (
                    analysis,
                    transcription.segments,
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
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        let speaker_segments = cluster(&analysis, &config)?;
        meeting_checkpoint(control, MeetingPhase::Clustering)?;
        performance.diarization_clustering_seconds = clustering_started.elapsed().as_secs_f64();
        eprintln!("Found {} speaker segment(s)", speaker_segments.len());
        if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
            for s in &speaker_segments {
                eprintln!("DIARSEG\t{:.3}\t{:.3}\t{}", s.start, s.end, s.speaker);
            }
        }

        eprintln!("Got {} word/segment(s) for merging", raw_segments.len());
        if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
            for (st, en, tx) in &raw_segments {
                eprintln!("WORD\t{:.3}\t{:.3}\t{}", st, en, tx.replace('\t', " "));
            }
        }

        let merge_diagnostics_started = Instant::now();
        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let diarized = merge_with_transcript(&speaker_segments, &transcript);
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
        let diagnostic_segments: Vec<TranscriptSegment> = consolidated
            .iter()
            .map(|segment| TranscriptSegment {
                start: segment.start,
                end: segment.end,
                text: segment.text.clone(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            })
            .collect();
        let coverage = analyze_coverage_profile(&coverage_profile, &diagnostic_segments);
        let mut warnings = combined_warnings(&coverage, language, detected_language.as_ref());
        if let Some(diagnostics) = &language_regions {
            warnings.extend(diagnostics.warnings.clone());
        }
        emit_warnings(&warnings);

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
        });
        performance.json_assembly_seconds = json_assembly_started.elapsed().as_secs_f64();
        performance.total_seconds = file_started.elapsed().as_secs_f64();
        json["performance"] =
            serde_json::to_value(&performance).expect("diarization performance serializes");
        eprintln!(
            "Diarized transcription: {:.2}s total ({:.2}x realtime), cache_hit={}",
            performance.total_seconds,
            duration / performance.total_seconds.max(f64::EPSILON),
            performance.cache_hit
        );
        if args.meeting_json {
            meeting_checkpoint(control, MeetingPhase::Finalizing)?;
            let source_sha256 = meeting_source_sha256.expect("meeting JSON records source hash");
            verify_meeting_source_unchanged(file, &source_sha256, control)?;
            let meeting = meeting_transcript_from_plain_segments(
                source_sha256,
                language,
                model,
                duration,
                &plain_segments,
            )?;
            let json = serde_json::to_value(&meeting).map_err(|_| {
                DictationError::TranscriptionFailed(
                    "Diarized meeting transcript serialization failed".to_string(),
                )
            })?;
            meeting_checkpoint(control, MeetingPhase::Finalizing)?;
            return Ok(FileTranscription {
                json,
                plain,
                meeting: Some(meeting),
            });
        }
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
        eprintln!("Verifying Silero VAD model...");
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
        beam_size: args.beam_size.unwrap_or(if stored.beam_size >= 2 {
            stored.beam_size
        } else {
            sagascript_core::transcription::FILE_TRANSCRIBE_BEAM
        }),
        temperature_fallback: stored.temperature_fallback,
        vad_model_path,
        // Coverage diagnostics need real segment bounds for both human and
        // JSON output. Text decoding remains in no-timestamps mode.
        segment_timestamps: true,
        parallel_chunks: 1,
    };
    opts.parallel_chunks = args.parallel.unwrap_or_else(|| {
        recommended_parallel_chunks(audio.len(), model, opts.beam_size)
    });
    if opts.beam_size >= 2 {
        eprintln!("Beam search: width {}", opts.beam_size);
    }
    if opts.parallel_chunks > 1 {
        eprintln!("Parallel Whisper states: {}", opts.parallel_chunks);
    }
    if vad_enabled {
        eprintln!("VAD: enabled");
    }

    let mut segments = if duration > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap());
        let pb_cb = pb.clone();
        let segments =
            backend.transcribe_sync_with_options_segments(&audio, language, &opts, move |pct| {
                crate::set_transcription_progress(&pb_cb, pct);
            })?;
        pb.finish_and_clear();
        segments
    } else {
        eprintln!("Transcribing...");
        backend.transcribe_sync_with_options_segments(&audio, language, &opts, |_| {})?
    };
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
    emit_warnings(&warnings);

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

    Ok(FileTranscription {
        json,
        plain: text,
        #[cfg(feature = "diarization")]
        meeting: None,
    })
}

/// Overlap the CPU/ONNX diarization analysis with Metal Whisper inference on
/// macOS. These workloads have no shared native context: only the Whisper half
/// touches `backend`, while the worker owns fresh ONNX sessions. CPU-only
/// targets remain sequential to avoid two heavy CPU runtimes contending.
#[cfg(feature = "diarization")]
fn run_diarization_analysis(
    audio: &[f32],
    backend: &WhisperBackend,
    language: Language,
    prompt: Option<&str>,
    config: &sagascript_core::diarization::DiarizeConfig,
) -> Result<
    (
        sagascript_core::diarization::DiarizationAnalysis,
        sagascript_core::diarization::DiarizationTimings,
        sagascript_core::transcription::DiarizationTranscription,
    ),
    DictationError,
> {
    #[cfg(target_os = "macos")]
    {
        std::thread::scope(|scope| {
            let diarization = scope.spawn(|| sagascript_core::diarization::analyze(audio, config));
            let transcription =
                backend.transcribe_sync_for_diarization_profiled(audio, language, prompt);
            let (analysis, timings) = diarization.join().map_err(|_| {
                DictationError::DiarizationError(
                    "Diarization analysis worker terminated unexpectedly".to_string(),
                )
            })??;
            Ok((analysis, timings, transcription?))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (analysis, timings) = sagascript_core::diarization::analyze(audio, config)?;
        let transcription =
            backend.transcribe_sync_for_diarization_profiled(audio, language, prompt)?;
        Ok((analysis, timings, transcription))
    }
}

fn expand_inputs(inputs: &[PathBuf], recursive: bool) -> Result<Vec<PathBuf>, DictationError> {
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();

    for input in inputs {
        if input.is_dir() {
            let mut directory_files = Vec::new();
            collect_directory_files(input, recursive, &mut directory_files)?;
            directory_files.sort();
            for file in directory_files {
                if seen.insert(file.clone()) {
                    expanded.push(file);
                }
            }
        } else if seen.insert(input.clone()) {
            // Keep explicit unsupported, missing, or corrupt files in the work
            // list so they produce an item-level error without hiding later
            // valid inputs.
            expanded.push(input.clone());
        }
    }

    Ok(expanded)
}

fn collect_directory_files(
    directory: &Path,
    recursive: bool,
    output: &mut Vec<PathBuf>,
) -> Result<(), DictationError> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to read directory '{}': {error}",
            directory.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to read an entry in '{}': {error}",
                directory.display()
            ))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect '{}': {error}",
                entry.path().display()
            ))
        })?;
        let path = entry.path();
        if file_type.is_file() && has_supported_extension(&path) {
            output.push(path);
        } else if recursive && file_type.is_dir() {
            collect_directory_files(&path, true, output)?;
        }
    }

    Ok(())
}

fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

/// Segment scores in this band are uncertain enough to consider an explicit
/// user-supplied spelling, but not so unreliable that a blind edit is useful.
const VOCAB_CORRECTION_MIN_LOGPROB: f32 = -0.8;
const VOCAB_CORRECTION_MAX_LOGPROB: f32 = -0.3;
const VOCAB_CORRECTION_MAX_NO_SPEECH_PROB: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct VocabularyCorrection {
    segment_index: usize,
    original: String,
    replacement: String,
    method: &'static str,
    avg_logprob: Option<f32>,
    no_speech_prob: f32,
}

fn apply_hint_corrections(
    segments: &mut [TranscriptSegment],
    vocabulary: &[String],
) -> Vec<VocabularyCorrection> {
    let mut corrections = Vec::new();
    for (segment_index, segment) in segments.iter_mut().enumerate() {
        let Some(avg_logprob) = segment.avg_logprob else {
            continue;
        };
        if !(VOCAB_CORRECTION_MIN_LOGPROB..=VOCAB_CORRECTION_MAX_LOGPROB).contains(&avg_logprob)
            || segment.no_speech_prob > VOCAB_CORRECTION_MAX_NO_SPEECH_PROB
        {
            continue;
        }

        let (corrected, segment_corrections) = correct_segment_text(
            &segment.text,
            vocabulary,
            segment_index,
            avg_logprob,
            segment.no_speech_prob,
        );
        segment.text = corrected;
        corrections.extend(segment_corrections);
    }
    corrections
}

fn apply_glossary_corrections(
    segments: &mut [TranscriptSegment],
    glossary: &Glossary,
) -> Vec<VocabularyCorrection> {
    let fragments = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>();
    let (corrected_fragments, applied) = glossary.correct_fragments(&fragments);
    let mut corrections = Vec::with_capacity(applied.len());
    for (segment_index, correction) in applied {
        corrections.push(VocabularyCorrection {
            segment_index,
            original: correction.original,
            replacement: correction.replacement,
            method: "explicit_alias",
            avg_logprob: segments[segment_index].avg_logprob,
            no_speech_prob: segments[segment_index].no_speech_prob,
        });
    }
    for (segment, text) in segments.iter_mut().zip(corrected_fragments) {
        segment.text = text;
    }
    corrections
}

fn correct_segment_text(
    text: &str,
    vocabulary: &[String],
    segment_index: usize,
    avg_logprob: f32,
    no_speech_prob: f32,
) -> (String, Vec<VocabularyCorrection>) {
    let mut corrected = String::with_capacity(text.len());
    let mut corrections = Vec::new();
    let mut word = String::new();

    let mut finish_word = |word: &mut String, corrected: &mut String| {
        if word.is_empty() {
            return;
        }
        let word_lowercase = word.to_lowercase();
        if vocabulary
            .iter()
            .any(|candidate| candidate.to_lowercase() == word_lowercase)
        {
            corrected.push_str(word);
            word.clear();
            return;
        }
        let matches: Vec<&String> = vocabulary
            .iter()
            .filter(|candidate| levenshtein_distance_one_or_less(word, candidate) == Some(1))
            .collect();
        if matches.len() == 1 {
            let replacement = matches[0].clone();
            corrections.push(VocabularyCorrection {
                segment_index,
                original: word.clone(),
                replacement: replacement.clone(),
                method: "fuzzy_one_edit",
                avg_logprob: Some(avg_logprob),
                no_speech_prob,
            });
            corrected.push_str(&replacement);
        } else {
            corrected.push_str(word);
        }
        word.clear();
    };

    for character in text.chars() {
        if character.is_alphabetic() {
            word.push(character);
        } else {
            finish_word(&mut word, &mut corrected);
            corrected.push(character);
        }
    }
    finish_word(&mut word, &mut corrected);
    (corrected, corrections)
}

/// Returns `Some(distance)` only for strings at edit distance zero or one.
/// Anything farther is irrelevant to this deliberately strict correction path.
fn levenshtein_distance_one_or_less(left: &str, right: &str) -> Option<usize> {
    let left: Vec<char> = left.to_lowercase().chars().collect();
    let right: Vec<char> = right.to_lowercase().chars().collect();
    let length_difference = left.len().abs_diff(right.len());
    if length_difference > 1 {
        return None;
    }

    let (mut left_index, mut right_index, mut edits) = (0, 0, 0);
    while left_index < left.len() && right_index < right.len() {
        if left[left_index] == right[right_index] {
            left_index += 1;
            right_index += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return None;
            }
            if left.len() > right.len() {
                left_index += 1;
            } else if right.len() > left.len() {
                right_index += 1;
            } else {
                left_index += 1;
                right_index += 1;
            }
        }
    }
    edits += left.len() - left_index + right.len() - right_index;
    (edits <= 1).then_some(edits)
}

fn combined_warnings(
    coverage: &CoverageDiagnostics,
    configured_language: Language,
    detected_language: Option<&LanguageDetection>,
) -> Vec<TranscriptionWarning> {
    let mut warnings = coverage.warnings.clone();
    if let Some(detected) = detected_language {
        if let Some(warning) = language_mismatch_warning(configured_language, detected) {
            warnings.push(warning);
        }
    }
    warnings
}

fn emit_warnings(warnings: &[TranscriptionWarning]) {
    for warning in warnings {
        eprintln!("Warning [{}]: {}", warning.code, warning.message);
    }
}

const LANGUAGE_WINDOW_SECONDS: usize = 20;
const LANGUAGE_WINDOW_SAMPLES: usize = LANGUAGE_WINDOW_SECONDS * 16_000;
const LANGUAGE_REDETECTION_MIN_SECONDS: usize = 60;
const LANGUAGE_REDETECTION_MAX_WINDOWS: usize = 60;
const LANGUAGE_WINDOW_MIN_RMS: f64 = 0.0015;

/// Re-evaluate only long auto-language files. The sampling cap keeps detection
/// bounded for multi-hour sources; this function is called independently for
/// each source when native batch mode invokes the single-file pipeline.
fn detect_file_language_regions(
    backend: &WhisperBackend,
    audio: &[f32],
    configured_language: Language,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    detect_language_regions_with(audio, configured_language, |window| {
        backend.detect_language(window)
    })
}

fn detect_language_regions_with(
    audio: &[f32],
    configured_language: Language,
    mut detect: impl FnMut(&[f32]) -> Result<Option<LanguageDetection>, DictationError>,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    let windows = language_window_plan(audio.len(), configured_language);
    if windows.is_empty() {
        return Ok(None);
    }

    eprintln!(
        "Checking {} sampled window(s) for sustained language changes...",
        windows.len()
    );
    let mut detections = Vec::new();
    for window in windows {
        let samples = &audio[window.start_sample..window.end_sample];
        if samples.len() < LANGUAGE_WINDOW_SAMPLES / 2
            || root_mean_square(samples) < LANGUAGE_WINDOW_MIN_RMS
        {
            continue;
        }
        let Some(detection) = detect(samples)? else {
            continue;
        };
        detections.push(LanguageWindow {
            sequence: window.sequence,
            start: window.start_sample as f64 / 16_000.0,
            end: window.end_sample as f64 / 16_000.0,
            language: detection.language,
            probability: detection.probability,
        });
    }

    Ok(Some(analyze_language_windows(&detections)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlannedLanguageWindow {
    sequence: usize,
    start_sample: usize,
    end_sample: usize,
}

fn language_window_plan(
    audio_samples: usize,
    configured_language: Language,
) -> Vec<PlannedLanguageWindow> {
    if configured_language != Language::Auto
        || audio_samples < LANGUAGE_REDETECTION_MIN_SECONDS * 16_000
    {
        return Vec::new();
    }

    let total_windows = audio_samples.div_ceil(LANGUAGE_WINDOW_SAMPLES);
    let step = total_windows.div_ceil(LANGUAGE_REDETECTION_MAX_WINDOWS);
    (0..total_windows)
        .step_by(step.max(1))
        .enumerate()
        .map(|(sequence, source_window)| {
            let start_sample = source_window * LANGUAGE_WINDOW_SAMPLES;
            PlannedLanguageWindow {
                sequence,
                start_sample,
                end_sample: (start_sample + LANGUAGE_WINDOW_SAMPLES).min(audio_samples),
            }
        })
        .collect()
}

fn root_mean_square(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples
        .iter()
        .map(|&sample| f64::from(sample) * f64::from(sample))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt()
}

fn detect_file_language(
    transcription_backend: &WhisperBackend,
    audio: &[f32],
    transcription_model: WhisperModel,
) -> Result<Option<LanguageDetection>, DictationError> {
    if transcription_model.is_english_only() {
        return Ok(None);
    }
    if !transcription_model.is_language_optimized() {
        return transcription_backend.detect_language(audio);
    }

    let Some(detection_model) = neutral_language_detection_model(model::is_model_downloaded) else {
        eprintln!(
            "Warning: language mismatch check skipped because no neutral multilingual model is downloaded."
        );
        return Ok(None);
    };
    eprintln!(
        "Checking language with neutral model: {}...",
        detection_model.display_name()
    );
    let detection_backend = WhisperBackend::new();
    detection_backend.load_model(detection_model)?;
    let initial_detection = detection_backend.detect_language(audio)?;
    let Some(initial) = initial_detection else {
        return Ok(None);
    };
    if initial.probability >= 0.90 {
        return Ok(Some(initial));
    }

    // Release the cheap detector before loading the larger fallback. The
    // transcription backend is still resident, so keeping both detection
    // contexts alive here would needlessly increase peak memory.
    drop(detection_backend);

    let Some(accurate_model) =
        accurate_language_detection_model(model::is_model_downloaded, detection_model)
    else {
        return Ok(Some(initial));
    };
    eprintln!(
        "Language check was uncertain ({} p={:.3}); verifying with {}...",
        initial.language,
        initial.probability,
        accurate_model.display_name()
    );
    let accurate_backend = WhisperBackend::new();
    accurate_backend.load_model(accurate_model)?;
    accurate_backend.detect_language(audio)
}

fn neutral_language_detection_model(
    is_downloaded: impl Fn(WhisperModel) -> bool,
) -> Option<WhisperModel> {
    // Start with a small neutral model so the common, confident case stays
    // cheap. An uncertain result is escalated below.
    [
        WhisperModel::Base,
        WhisperModel::Tiny,
        WhisperModel::Small,
        WhisperModel::Medium,
        WhisperModel::LargeV3TurboQ8,
        WhisperModel::LargeV3Turbo,
    ]
    .into_iter()
    .find(|&candidate| is_downloaded(candidate))
}

fn accurate_language_detection_model(
    is_downloaded: impl Fn(WhisperModel) -> bool,
    exclude: WhisperModel,
) -> Option<WhisperModel> {
    // Older Whisper language classifiers can confuse accented English with
    // Swedish. Large v3 Turbo is used only when the cheap detector is below
    // the strong-confidence threshold.
    [WhisperModel::LargeV3TurboQ8, WhisperModel::LargeV3Turbo]
        .into_iter()
        .find(|&candidate| candidate != exclude && is_downloaded(candidate))
}

/// Validates a `--diarize-threshold` value: must parse as a finite f32 in the
/// documented 0.0-2.0 range. NaN/infinite or out-of-range values silently
/// produce degenerate agglomerative clustering downstream, so reject them at
/// the CLI boundary rather than at the clustering call site.
#[cfg(feature = "diarization")]
fn parse_diarize_threshold(s: &str) -> Result<f32, String> {
    let value: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if !value.is_finite() {
        return Err(format!(
            "diarize-threshold must be a finite number, got '{s}'"
        ));
    }
    if !(0.0..=2.0).contains(&value) {
        return Err(format!(
            "diarize-threshold must be between 0.0 and 2.0, got {value}"
        ));
    }
    Ok(value)
}

fn parse_parallel_chunks(s: &str) -> Result<usize, String> {
    let value = s
        .parse::<usize>()
        .map_err(|_| format!("'{s}' is not a valid chunk count"))?;
    if !(1..=4).contains(&value) {
        return Err(format!("parallel must be between 1 and 4, got {value}"));
    }
    Ok(value)
}

#[cfg(test)]
mod parallel_chunks_tests {
    use super::parse_parallel_chunks;

    #[test]
    fn accepts_supported_parallel_chunk_counts() {
        assert_eq!(parse_parallel_chunks("1"), Ok(1));
        assert_eq!(parse_parallel_chunks("4"), Ok(4));
    }

    #[test]
    fn rejects_zero_and_unbounded_parallelism() {
        assert!(parse_parallel_chunks("0").is_err());
        assert!(parse_parallel_chunks("5").is_err());
        assert!(parse_parallel_chunks("many").is_err());
    }
}

pub(crate) fn resolve_profile(
    settings: &Settings,
    profile_id: &str,
) -> Result<HotkeyProfile, DictationError> {
    let profile = settings
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| {
            DictationError::SettingsError(format!("Unknown dictation profile '{profile_id}'"))
        })?;
    if profile.language == Language::Auto {
        return Err(DictationError::SettingsError(
            "Profile-scoped dictionaries require an explicit language".to_string(),
        ));
    }
    Ok(profile)
}

pub fn parse_language(s: &str) -> Result<Language, DictationError> {
    match s {
        "en" | "english" => Ok(Language::English),
        "sv" | "swedish" => Ok(Language::Swedish),
        "no" | "norwegian" => Ok(Language::Norwegian),
        "fi" | "finnish" => Ok(Language::Finnish),
        "auto" => Ok(Language::Auto),
        other => Err(DictationError::SettingsError(format!(
            "Unknown language '{other}'. Valid: en, sv, no, fi, auto"
        ))),
    }
}

/// Resolves the whisper model to use for a run: an explicit `--model` argument
/// always wins; otherwise, if `auto_select_model` is set, the model
/// recommended for `language` is used; otherwise `fallback` (the stored
/// `whisper_model` setting) is used, ignoring `language`.
///
/// This is the single source of truth for the branch shared by
/// `transcribe::run()` and `record::run()` — keep their call sites in sync
/// with this function rather than re-deriving the logic inline.
///
/// Note: this intentionally does not delegate to
/// `Settings::effective_model()` (core/settings/manager.rs), because that
/// method always uses the *stored* `Settings::language`, while callers here
/// need to honor a `--language` override that differs from the stored value
/// (e.g. `--language sv` with auto-select on should recommend the Swedish
/// model even if the stored language is English).
pub fn resolve_effective_model(
    model_arg: Option<&str>,
    language: Language,
    auto_select_model: bool,
    fallback: WhisperModel,
) -> Result<WhisperModel, DictationError> {
    match model_arg {
        Some(s) => parse_model(s),
        None => Ok(if auto_select_model {
            WhisperModel::recommended(language)
        } else {
            fallback
        }),
    }
}

/// Resolve a non-empty one-run hint and compose it through Settings' scoped
/// glossary helper. The helper owns legacy-global hint-only behavior and
/// selected explicit-profile aliases; CLI never parses those sources directly.
pub(crate) fn effective_glossary(
    stored: &Settings,
    profile_id: Option<&str>,
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&Path>,
) -> Result<Glossary, DictationError> {
    let one_run_prompt = resolve_one_run_prompt(cli_prompt, cli_prompt_file)?;
    let source =
        stored.effective_glossary_source_with_prompt(profile_id, one_run_prompt.as_deref());
    Ok(Glossary::parse(&source))
}

fn resolve_one_run_prompt(
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&Path>,
) -> Result<Option<String>, DictationError> {
    if let Some(path) = cli_prompt_file {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            DictationError::FileDecodeError(format!(
                "Failed to read prompt file '{}': {e}",
                path.display()
            ))
        })?;
        let trimmed = contents.trim();
        return Ok((!trimmed.is_empty()).then(|| trimmed.to_string()));
    }
    let Some(prompt) = cli_prompt else {
        return Ok(None);
    };
    let trimmed = prompt.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

pub fn parse_model(s: &str) -> Result<WhisperModel, DictationError> {
    match s {
        "tiny.en" => Ok(WhisperModel::TinyEn),
        "tiny" => Ok(WhisperModel::Tiny),
        "base.en" => Ok(WhisperModel::BaseEn),
        "base" => Ok(WhisperModel::Base),
        "fi-whisper-tiny" => Ok(WhisperModel::FinnishWhisperTiny),
        "kb-whisper-tiny" => Ok(WhisperModel::KbWhisperTiny),
        "kb-whisper-base" => Ok(WhisperModel::KbWhisperBase),
        "kb-whisper-small" => Ok(WhisperModel::KbWhisperSmall),
        "kb-whisper-medium" => Ok(WhisperModel::KbWhisperMedium),
        "kb-whisper-large" => Ok(WhisperModel::KbWhisperLarge),
        "nb-whisper-tiny" => Ok(WhisperModel::NbWhisperTiny),
        "nb-whisper-base" => Ok(WhisperModel::NbWhisperBase),
        "nb-whisper-small" => Ok(WhisperModel::NbWhisperSmall),
        "nb-whisper-medium" => Ok(WhisperModel::NbWhisperMedium),
        "nb-whisper-large" => Ok(WhisperModel::NbWhisperLarge),
        "small.en" => Ok(WhisperModel::SmallEn),
        "small" => Ok(WhisperModel::Small),
        "medium.en" => Ok(WhisperModel::MediumEn),
        "medium" => Ok(WhisperModel::Medium),
        "large-v3-turbo" => Ok(WhisperModel::LargeV3Turbo),
        "large-v3-turbo-q8_0" => Ok(WhisperModel::LargeV3TurboQ8),
        other => Err(DictationError::SettingsError(format!(
            "Unknown model '{other}'. Run 'sagascript list-models' to see available models."
        ))),
    }
}

pub fn model_id_string(model: WhisperModel) -> &'static str {
    match model {
        WhisperModel::TinyEn => "tiny.en",
        WhisperModel::Tiny => "tiny",
        WhisperModel::BaseEn => "base.en",
        WhisperModel::Base => "base",
        WhisperModel::FinnishWhisperTiny => "fi-whisper-tiny",
        WhisperModel::KbWhisperTiny => "kb-whisper-tiny",
        WhisperModel::KbWhisperBase => "kb-whisper-base",
        WhisperModel::KbWhisperSmall => "kb-whisper-small",
        WhisperModel::KbWhisperMedium => "kb-whisper-medium",
        WhisperModel::KbWhisperLarge => "kb-whisper-large",
        WhisperModel::NbWhisperTiny => "nb-whisper-tiny",
        WhisperModel::NbWhisperBase => "nb-whisper-base",
        WhisperModel::NbWhisperSmall => "nb-whisper-small",
        WhisperModel::NbWhisperMedium => "nb-whisper-medium",
        WhisperModel::NbWhisperLarge => "nb-whisper-large",
        WhisperModel::SmallEn => "small.en",
        WhisperModel::Small => "small",
        WhisperModel::MediumEn => "medium.en",
        WhisperModel::Medium => "medium",
        WhisperModel::LargeV3Turbo => "large-v3-turbo",
        WhisperModel::LargeV3TurboQ8 => "large-v3-turbo-q8_0",
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<(), DictationError> {
    use arboard::Clipboard;
    let mut clipboard = Clipboard::new()
        .map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;
    clipboard
        .set_text(text)
        .map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "diarization")]
    #[test]
    fn cache_path_cannot_alias_the_input_recording() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-cache-alias-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let input = root.join("recording.m4a");
        let other = root.join("cache.json");
        std::fs::write(&input, b"audio").unwrap();
        std::fs::write(&other, b"cache").unwrap();

        assert!(paths_refer_to_same_file(&input, &input));
        assert!(!paths_refer_to_same_file(&input, &other));
        #[cfg(unix)]
        {
            let hard_link = root.join("recording-cache.json");
            std::fs::hard_link(&input, &hard_link).unwrap();
            assert!(paths_refer_to_same_file(&input, &hard_link));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_expansion_filters_sorts_recurses_and_deduplicates() {
        let root =
            std::env::temp_dir().join(format!("sagascript-batch-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let nested = root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let a = root.join("a.WAV");
        let b = root.join("b.mp3");
        let ignored = root.join("notes.txt");
        let c = nested.join("c.m4a");
        for path in [&a, &b, &ignored, &c] {
            std::fs::write(path, b"fixture").unwrap();
        }

        assert_eq!(
            expand_inputs(std::slice::from_ref(&root), false).unwrap(),
            vec![a.clone(), b.clone()]
        );
        assert_eq!(
            expand_inputs(&[root.clone(), a.clone()], true).unwrap(),
            vec![a, b, c]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_unsupported_or_missing_inputs_remain_item_level_work() {
        let inputs = vec![PathBuf::from("bad.txt"), PathBuf::from("missing.wav")];
        assert_eq!(expand_inputs(&inputs, false).unwrap(), inputs);
    }

    #[test]
    fn batch_continues_after_item_failure_by_default() {
        let files = vec![
            PathBuf::from("one.wav"),
            PathBuf::from("bad.wav"),
            PathBuf::from("three.wav"),
        ];
        let mut visited = Vec::new();
        let mut executions = Vec::new();
        let counts = process_batch(
            &files,
            false,
            |_, file| {
                visited.push(file.to_path_buf());
                if file == Path::new("bad.wav") {
                    Err(DictationError::FileDecodeError(
                        "corrupt fixture".to_string(),
                    ))
                } else {
                    Ok(FileTranscription {
                        json: serde_json::json!({"text": file.display().to_string()}),
                        plain: file.display().to_string(),
                        #[cfg(feature = "diarization")]
                        meeting: None,
                    })
                }
            },
            |execution| {
                executions.push(execution);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(visited, files);
        assert_eq!(counts, (3, 1));
        assert_eq!(executions.len(), 3);
        assert!(executions[0].output.is_ok());
        assert!(executions[1].output.is_err());
        assert!(executions[2].output.is_ok());
    }

    #[test]
    fn batch_fail_fast_stops_after_first_failure() {
        let files = vec![PathBuf::from("bad.wav"), PathBuf::from("never.wav")];
        let mut executions = Vec::new();
        let counts = process_batch(
            &files,
            true,
            |_, file| {
                Err(DictationError::FileDecodeError(format!(
                    "cannot decode {}",
                    file.display()
                )))
            },
            |execution| {
                executions.push(execution);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(counts, (1, 1));
        assert_eq!(executions.len(), 1);
        assert_eq!(executions[0].source, PathBuf::from("bad.wav"));
    }

    #[test]
    fn supported_extensions_are_case_insensitive() {
        assert!(has_supported_extension(Path::new("AUDIO.FLAC")));
        assert!(has_supported_extension(Path::new("video.mov")));
        assert!(!has_supported_extension(Path::new("transcript.json")));
    }

    #[test]
    fn explicit_language_keeps_region_redetection_fast_path_disabled() {
        let one_hour = 60 * 60 * 16_000;
        for language in [
            Language::English,
            Language::Swedish,
            Language::Norwegian,
            Language::Finnish,
        ] {
            assert!(language_window_plan(one_hour, language).is_empty());
            let result = detect_language_regions_with(&vec![0.1; 60 * 16_000], language, |_| {
                panic!("explicit language must not invoke region detection")
            })
            .unwrap();
            assert!(result.is_none());
        }
    }

    #[test]
    fn auto_language_region_detection_is_bounded_and_skips_silence() {
        let nine_hours = 9 * 60 * 60 * 16_000;
        assert!(
            language_window_plan(nine_hours, Language::Auto).len()
                <= LANGUAGE_REDETECTION_MAX_WINDOWS
        );

        let mut audio = vec![0.05; 100 * 16_000];
        audio[40 * 16_000..60 * 16_000].fill(0.0);
        let detections = ["en", "en", "sv", "sv"];
        let mut call_index = 0usize;
        let diagnostics = detect_language_regions_with(&audio, Language::Auto, |_| {
            let language = detections[call_index];
            call_index += 1;
            Ok(Some(LanguageDetection {
                language: language.to_string(),
                probability: 0.97,
            }))
        })
        .unwrap()
        .expect("long auto-language audio enables region detection");

        assert_eq!(call_index, 4, "silent window must not be classified");
        assert_eq!(diagnostics.regions.len(), 2);
        assert_eq!(diagnostics.warnings[0].code, "mixed_language_audio");
    }

    fn segment(text: &str, avg_logprob: Option<f32>, no_speech_prob: f32) -> TranscriptSegment {
        TranscriptSegment {
            start: 0.0,
            end: 1.0,
            text: text.to_string(),
            avg_logprob,
            no_speech_prob,
        }
    }

    #[test]
    fn cli_plain_assembly_matches_core_no_speech_filter() {
        let segments = vec![
            segment(" First.", Some(-0.1), 0.01),
            segment("<|nospeech|>-Hej Tack! Tack! Tack!", Some(-0.1), 0.9),
            segment(" Second.", Some(-0.1), 0.01),
        ];

        assert_eq!(assemble_transcript(&segments), " First. Second.");
        assert_eq!(
            segments[1].text, "<|nospeech|>-Hej Tack! Tack! Tack!",
            "display assembly must not mutate diagnostic segment text"
        );
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarized_plain_assembly_discards_no_speech_segments() {
        let segments = vec![
            DiarizedSegment {
                start: 0.0,
                end: 1.0,
                speaker: "SPEAKER_0".to_string(),
                text: "First.".to_string(),
            },
            DiarizedSegment {
                start: 1.0,
                end: 1.3,
                speaker: "SPEAKER_1".to_string(),
                text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
            },
            DiarizedSegment {
                start: 1.3,
                end: 2.0,
                speaker: "SPEAKER_1".to_string(),
                text: "Second.".to_string(),
            },
        ];

        assert_eq!(
            assemble_diarized_plain_text(&segments),
            "[SPEAKER_0] First.\n[SPEAKER_1] Second."
        );
        assert!(contains_no_speech_marker(&segments[1].text));
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn diarized_plain_filter_precedes_consolidation_without_mutating_diagnostics() {
        let segments = vec![
            DiarizedSegment {
                start: 0.0,
                end: 1.0,
                speaker: "SPEAKER_0".to_string(),
                text: "First hello Music Music Music".to_string(),
            },
            DiarizedSegment {
                start: 1.0,
                end: 1.3,
                speaker: "SPEAKER_0".to_string(),
                text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
            },
            DiarizedSegment {
                start: 1.3,
                end: 2.0,
                speaker: "SPEAKER_0".to_string(),
                text: "Second.".to_string(),
            },
        ];
        let consolidated = sagascript_core::diarization::merge::consolidate(&segments);
        let diagnostic_text = consolidated[0].text.clone();

        let glossary = Glossary::parse("Greeting = hello");
        let plain_segments =
            prepare_diarized_plain_segments(&segments, Language::English, &glossary);

        assert_eq!(consolidated.len(), 1);
        assert_eq!(consolidated[0].text, diagnostic_text);
        assert_eq!(
            diagnostic_text,
            "First hello Music Music Music <|nospeech|>-Hej Tack! Tack! Tack! Second."
        );
        assert_eq!(
            assemble_diarized_plain_text(&plain_segments),
            "[SPEAKER_0] First Greeting [MUSIC] Second."
        );
        assert_eq!(plain_segments.len(), 1);
        assert!(plain_segments
            .iter()
            .all(|segment| !contains_no_speech_marker(&segment.text)));
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_document_uses_plain_segments_and_canonical_metadata() {
        let segments = vec![
            DiarizedSegment {
                start: 1.0,
                end: 2.0,
                speaker: "SPEAKER_1".to_string(),
                text: "second".to_string(),
            },
            DiarizedSegment {
                start: 0.0,
                end: 1.0,
                speaker: "SPEAKER_0".to_string(),
                text: "first".to_string(),
            },
            DiarizedSegment {
                start: 2.0,
                end: 3.0,
                speaker: "SPEAKER_1".to_string(),
                text: "third".to_string(),
            },
        ];
        let document = meeting_transcript_from_plain_segments(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            Language::Swedish,
            WhisperModel::KbWhisperBase,
            3.0,
            &segments,
        )
        .expect("plain diarized segments are valid");

        assert_eq!(document.language, "sv");
        assert_eq!(document.model, "kb-whisper-base");
        assert_eq!(document.segments[0].id, "seg-000001");
        assert_eq!(document.segments[0].text, "first");
        assert_eq!(document.segments[1].text, "second");
        assert_eq!(document.speakers[0].id, "SPEAKER_1");
        assert_eq!(document.speakers[0].label, "SPEAKER_1");
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_document_clamps_finite_padded_end_without_mutating_source() {
        let segments = vec![DiarizedSegment {
            start: 0.5, end: 1.2, speaker: "S0".into(), text: "last word".into(),
        }];
        let document = meeting_transcript_from_plain_segments(
            "0".repeat(64), Language::English, WhisperModel::BaseEn, 1.0, &segments,
        ).expect("last padded Whisper window can overshoot decoded audio");
        assert_eq!(document.segments[0].start, 0.5);
        assert_eq!(document.segments[0].end, 1.0);
        assert_eq!(document.segments[0].text, "last word");
        assert_eq!(segments[0].end, 1.2);
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_document_still_rejects_nonfinite_and_outside_starts() {
        for (start, end) in [(0.5, f64::NAN), (0.5, f64::INFINITY), (1.1, 1.2), (-0.1, 0.5)] {
            let segments = vec![DiarizedSegment {
                start, end, speaker: "S0".into(), text: "invalid".into(),
            }];
            assert!(meeting_transcript_from_plain_segments(
                "0".repeat(64), Language::English, WhisperModel::BaseEn, 1.0, &segments,
            ).is_err());
        }
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_source_hash_is_bounded_and_detects_mutation() {
        let path = std::env::temp_dir().join(format!(
            "sagascript-meeting-hash-{}.wav",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"first bytes").expect("fixture write");
        let before = stable_file_sha256(&path, None).expect("hash fixture");
        assert_eq!(before.len(), 64);
        assert!(before.bytes().all(|byte| byte.is_ascii_hexdigit()));
        std::fs::write(&path, b"other bytes").expect("mutate fixture");
        assert!(verify_meeting_source_unchanged(&path, &before, None).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_control_precancel_checks_before_file_or_model_access() {
        use std::sync::atomic::AtomicBool;

        let cancellation = AtomicBool::new(true);
        let progress = |_| panic!("cancelled job must not emit progress");
        let control = MeetingControl {
            cancellation: &cancellation,
            progress: &progress,
        };
        let backend = WhisperBackend::new();
        let error = transcribe_meeting_file_with_control(
            Path::new("/definitely/missing/meeting.wav"),
            &Settings::default(),
            Language::English,
            WhisperModel::Base,
            &Glossary::parse(""),
            &backend,
            &control,
        )
        .expect_err("pre-cancelled job must fail");
        assert_eq!(
            error.to_string(),
            "Transcription failed: Meeting transcription cancelled"
        );
        assert!(cancellation.load(Ordering::Acquire));
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_control_emits_ordered_phases_and_never_clears_cancellation() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};

        let cancellation = AtomicBool::new(false);
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&phases);
        let progress = move |phase| observed.lock().expect("phase lock").push(phase);
        let control = MeetingControl {
            cancellation: &cancellation,
            progress: &progress,
        };
        let expected = [
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ];
        for phase in expected {
            meeting_checkpoint(Some(&control), phase).expect("checkpoint is live");
        }
        assert_eq!(*phases.lock().expect("phase lock"), expected);
        assert_eq!(
            serde_json::to_string(&MeetingPhase::LoadingModel).expect("phase serializes"),
            "\"loading_model\""
        );

        cancellation.store(true, Ordering::Release);
        assert!(meeting_checkpoint(Some(&control), MeetingPhase::Finalizing).is_err());
        assert!(cancellation.load(Ordering::Acquire));
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn meeting_json_scope_requires_one_explicit_regular_file() {
        let root =
            std::env::temp_dir().join(format!("sagascript-meeting-scope-{}", uuid::Uuid::new_v4()));
        let file = root.join("input.wav");
        let second = root.join("second.wav");
        std::fs::create_dir_all(&root).expect("fixture directory");
        std::fs::write(&file, b"audio fixture").expect("fixture file");
        std::fs::write(&second, b"second fixture").expect("second fixture");

        assert!(validate_meeting_input_scope(
            std::slice::from_ref(&file),
            false,
            std::slice::from_ref(&file)
        )
        .is_ok());
        assert!(validate_meeting_input_scope(
            &[file.clone(), second.clone()],
            false,
            &[file.clone(), second]
        )
        .is_err());
        assert!(validate_meeting_input_scope(std::slice::from_ref(&root), false, &[]).is_err());
        assert!(validate_meeting_input_scope(
            std::slice::from_ref(&file),
            true,
            std::slice::from_ref(&file)
        )
        .is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn explicit_aliases_correct_without_confidence_guessing() {
        let glossary = Glossary::parse("OpenRouter = open router\nmerge = merch");
        let mut segments = vec![segment(" Open router och Merch.", None, 0.0)];
        let corrections = apply_glossary_corrections(&mut segments, &glossary);

        assert_eq!(segments[0].text, " OpenRouter och merge.");
        assert_eq!(corrections.len(), 2);
        assert_eq!(corrections[0].method, "explicit_alias");
        assert_eq!(corrections[0].avg_logprob, None);
    }

    #[test]
    fn explicit_phrase_aliases_cross_whisper_segment_boundaries() {
        let glossary = Glossary::parse("OpenRouter = open router");
        let mut segments = vec![
            segment("Jag använder open", Some(-0.2), 0.0),
            segment(" router varje dag.", Some(-0.4), 0.0),
        ];
        let corrections = apply_glossary_corrections(&mut segments, &glossary);

        assert_eq!(segments[0].text, "Jag använder OpenRouter");
        assert_eq!(segments[1].text, " varje dag.");
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].segment_index, 0);
        assert_eq!(corrections[0].avg_logprob, Some(-0.2));
    }

    #[test]
    fn correction_vocabulary_accepts_only_single_word_hint_items() {
        assert_eq!(
            Glossary::parse("Grimnir, grimnir, Erik Fredlund\nHugin, M5, AI-agent")
                .single_word_terms(),
            vec!["Grimnir", "Hugin"]
        );
    }

    #[test]
    fn corrects_repeated_unambiguous_one_edit_hint_matches() {
        let mut segments = vec![segment(" Grimner och Grimner.", Some(-0.313), 0.0)];
        let corrections = apply_hint_corrections(&mut segments, &["Grimnir".to_string()]);

        assert_eq!(segments[0].text, " Grimnir och Grimnir.");
        assert_eq!(corrections.len(), 2);
        assert_eq!(corrections[0].original, "Grimner");
        assert_eq!(corrections[0].replacement, "Grimnir");
        assert_eq!(corrections[0].segment_index, 0);
        assert_eq!(corrections[0].method, "fuzzy_one_edit");
        assert_eq!(corrections[0].avg_logprob, Some(-0.313));
    }

    #[test]
    fn does_not_correct_confident_suspect_or_non_speech_segments() {
        let vocabulary = ["Grimnir".to_string()];
        for (avg_logprob, no_speech_prob) in [
            (Some(-0.2), 0.0),
            (Some(-0.9), 0.0),
            (None, 0.0),
            (Some(-0.313), 0.6),
        ] {
            let mut segments = vec![segment(" Grimner", avg_logprob, no_speech_prob)];
            assert!(apply_hint_corrections(&mut segments, &vocabulary).is_empty());
            assert_eq!(segments[0].text, " Grimner");
        }
    }

    #[test]
    fn does_not_correct_ambiguous_or_more_distant_matches() {
        let mut segments = vec![segment(" Grimner Grimmer", Some(-0.313), 0.0)];
        let corrections = apply_hint_corrections(
            &mut segments,
            &["Grimnir".to_string(), "Grimnera".to_string()],
        );

        assert!(corrections.is_empty());
        assert_eq!(segments[0].text, " Grimner Grimmer");
    }

    #[test]
    fn does_not_replace_a_word_that_is_already_an_explicit_hint() {
        let mut segments = vec![segment(" Grimner", Some(-0.313), 0.0)];
        let corrections = apply_hint_corrections(
            &mut segments,
            &["Grimner".to_string(), "Grimnir".to_string()],
        );

        assert!(corrections.is_empty());
        assert_eq!(segments[0].text, " Grimner");
    }

    // -- resolve_one_run_prompt --

    #[test]
    fn one_run_prompt_inline_is_trimmed() {
        let got = resolve_one_run_prompt(Some("  Notre Dame  "), None).unwrap();
        assert_eq!(got.as_deref(), Some("Notre Dame"));
    }

    #[test]
    fn one_run_prompt_empty_input_is_deferred_to_saved_settings() {
        assert!(resolve_one_run_prompt(None, None).unwrap().is_none());
        assert!(resolve_one_run_prompt(Some("   "), None).unwrap().is_none());
    }

    #[test]
    fn one_run_prompt_file_wins_and_is_trimmed() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "sagascript_hint_test_{}_{}.txt",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, "  Estrid, Grimnir\n").unwrap();

        // File beats the inline flag; saved settings are composed by the core helper.
        let got = resolve_one_run_prompt(Some("inline"), Some(&path)).unwrap();
        assert_eq!(got.as_deref(), Some("Estrid, Grimnir"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn one_run_prompt_empty_file_is_deferred_to_saved_settings() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "sagascript_hint_test_{}_{}.txt",
            std::process::id(),
            line!()
        ));
        std::fs::write(&path, "   \n\t").unwrap();

        let got = resolve_one_run_prompt(None, Some(&path)).unwrap();
        assert!(got.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn one_run_prompt_missing_file_errors() {
        let path = Path::new("/nonexistent/sagascript/hint/vocab.txt");
        let err = resolve_one_run_prompt(None, Some(path)).unwrap_err();
        assert!(matches!(err, DictationError::FileDecodeError(_)));
    }

    #[test]
    fn scoped_glossary_keeps_global_aliases_hint_only_and_profiles_isolated() {
        let mut settings = Settings {
            initial_prompt: "merge = merch\nOpenRouter".to_string(),
            hotkey_profiles: vec![
                HotkeyProfile {
                    id: "swedish".to_string(),
                    name: "Swedish".to_string(),
                    shortcut: "F13".to_string(),
                    language: Language::Swedish,
                },
                HotkeyProfile {
                    id: "english".to_string(),
                    name: "English".to_string(),
                    shortcut: "F14".to_string(),
                    language: Language::English,
                },
            ],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("swedish".to_string(), "merge = merch".to_string());

        let global = effective_glossary(&settings, None, None, None).unwrap();
        assert_eq!(global.correct_text("merch").0, "merch");

        let swedish = effective_glossary(&settings, Some("swedish"), None, None).unwrap();
        assert_eq!(swedish.correct_text("merch").0, "merge");

        let english = effective_glossary(&settings, Some("english"), None, None).unwrap();
        assert_eq!(english.correct_text("merch").0, "merch");
    }

    #[test]
    fn one_run_prompt_replaces_global_hint_but_keeps_profile_scope() {
        let mut settings = Settings {
            initial_prompt: "merge = merch".to_string(),
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "F13".to_string(),
                language: Language::Swedish,
            }],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("swedish".to_string(), "fika = coffee".to_string());

        let glossary = effective_glossary(
            &settings,
            Some("swedish"),
            Some("  OpenRouter = router  "),
            None,
        )
        .unwrap();

        assert_eq!(glossary.correct_text("merch").0, "merch");
        assert_eq!(glossary.correct_text("coffee").0, "fika");

        let empty_override = effective_glossary(&settings, Some("swedish"), Some("  \n"), None)
            .unwrap();
        assert_eq!(empty_override.correct_text("merch").0, "merch");
        assert_eq!(empty_override.correct_text("coffee").0, "fika");
    }

    #[test]
    fn auto_or_missing_profile_never_infers_alias_scope() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "F13".to_string(),
                language: Language::Swedish,
            }],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("swedish".to_string(), "merge = merch".to_string());

        let glossary = effective_glossary(&settings, None, None, None).unwrap();
        assert_eq!(glossary.correct_text("merch").0, "merch");
    }

    // -- parse_language --

    #[test]
    fn parse_language_valid_codes() {
        assert_eq!(parse_language("en").unwrap(), Language::English);
        assert_eq!(parse_language("sv").unwrap(), Language::Swedish);
        assert_eq!(parse_language("no").unwrap(), Language::Norwegian);
        assert_eq!(parse_language("fi").unwrap(), Language::Finnish);
        assert_eq!(parse_language("auto").unwrap(), Language::Auto);
    }

    #[test]
    fn parse_language_long_names() {
        assert_eq!(parse_language("english").unwrap(), Language::English);
        assert_eq!(parse_language("swedish").unwrap(), Language::Swedish);
        assert_eq!(parse_language("norwegian").unwrap(), Language::Norwegian);
        assert_eq!(parse_language("finnish").unwrap(), Language::Finnish);
    }

    #[test]
    fn parse_language_invalid() {
        assert!(parse_language("de").is_err());
        assert!(parse_language("").is_err());
        assert!(parse_language("ENGLISH").is_err()); // case-sensitive
    }

    #[test]
    fn parse_language_error_message() {
        let err = parse_language("fr").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("fr"), "error should mention input: {msg}");
        assert!(msg.contains("en"), "error should list valid options: {msg}");
    }

    // -- parse_model --

    #[test]
    fn parse_model_all_valid_ids() {
        let cases = [
            ("tiny.en", WhisperModel::TinyEn),
            ("tiny", WhisperModel::Tiny),
            ("base.en", WhisperModel::BaseEn),
            ("base", WhisperModel::Base),
            ("fi-whisper-tiny", WhisperModel::FinnishWhisperTiny),
            ("kb-whisper-tiny", WhisperModel::KbWhisperTiny),
            ("kb-whisper-base", WhisperModel::KbWhisperBase),
            ("kb-whisper-small", WhisperModel::KbWhisperSmall),
            ("kb-whisper-medium", WhisperModel::KbWhisperMedium),
            ("kb-whisper-large", WhisperModel::KbWhisperLarge),
            ("nb-whisper-tiny", WhisperModel::NbWhisperTiny),
            ("nb-whisper-base", WhisperModel::NbWhisperBase),
            ("nb-whisper-small", WhisperModel::NbWhisperSmall),
            ("nb-whisper-medium", WhisperModel::NbWhisperMedium),
            ("nb-whisper-large", WhisperModel::NbWhisperLarge),
            ("small.en", WhisperModel::SmallEn),
            ("small", WhisperModel::Small),
            ("medium.en", WhisperModel::MediumEn),
            ("medium", WhisperModel::Medium),
            ("large-v3-turbo", WhisperModel::LargeV3Turbo),
            ("large-v3-turbo-q8_0", WhisperModel::LargeV3TurboQ8),
        ];
        for (id, expected) in cases {
            assert_eq!(parse_model(id).unwrap(), expected, "parse_model({id})");
        }
    }

    #[test]
    fn parse_model_invalid() {
        assert!(parse_model("large-v3").is_err());
        assert!(parse_model("fi-whisper-medium").is_err());
        assert!(parse_model("").is_err());
        assert!(parse_model("BASE").is_err()); // case-sensitive
    }

    #[test]
    fn parse_model_error_message() {
        let err = parse_model("nonexistent").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nonexistent"));
        assert!(msg.contains("list-models"));
    }

    // -- model_id_string --

    #[test]
    fn model_id_string_all_variants() {
        let models = [
            (WhisperModel::TinyEn, "tiny.en"),
            (WhisperModel::Tiny, "tiny"),
            (WhisperModel::BaseEn, "base.en"),
            (WhisperModel::Base, "base"),
            (WhisperModel::FinnishWhisperTiny, "fi-whisper-tiny"),
            (WhisperModel::KbWhisperTiny, "kb-whisper-tiny"),
            (WhisperModel::KbWhisperBase, "kb-whisper-base"),
            (WhisperModel::KbWhisperSmall, "kb-whisper-small"),
            (WhisperModel::NbWhisperTiny, "nb-whisper-tiny"),
            (WhisperModel::NbWhisperBase, "nb-whisper-base"),
            (WhisperModel::NbWhisperSmall, "nb-whisper-small"),
            (WhisperModel::SmallEn, "small.en"),
            (WhisperModel::Small, "small"),
            (WhisperModel::MediumEn, "medium.en"),
            (WhisperModel::Medium, "medium"),
            (WhisperModel::LargeV3Turbo, "large-v3-turbo"),
            (WhisperModel::LargeV3TurboQ8, "large-v3-turbo-q8_0"),
        ];
        for (model, expected) in models {
            assert_eq!(model_id_string(model), expected);
        }
    }

    #[test]
    fn model_id_string_roundtrip_with_parse() {
        let all_models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::FinnishWhisperTiny,
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
            WhisperModel::LargeV3TurboQ8,
        ];
        for model in all_models {
            let id = model_id_string(model);
            let parsed = parse_model(id).unwrap();
            assert_eq!(parsed, model, "roundtrip failed for {id}");
        }
    }

    // -- resolve_effective_model --
    //
    // These exercise the exact branch used by both transcribe::run() and
    // record::run(): explicit arg wins -> auto_select_model recommends by
    // language -> otherwise the stored fallback model (language ignored).

    #[test]
    fn resolve_effective_model_none_auto_recommends_by_language() {
        let result =
            resolve_effective_model(None, Language::Swedish, true, WhisperModel::Base).unwrap();
        assert_eq!(result, WhisperModel::KbWhisperBase);
    }

    #[test]
    fn resolve_effective_model_none_no_auto_uses_fallback_ignoring_language() {
        let result =
            resolve_effective_model(None, Language::Swedish, false, WhisperModel::LargeV3Turbo)
                .unwrap();
        assert_eq!(result, WhisperModel::LargeV3Turbo);
    }

    #[test]
    fn resolve_effective_model_explicit_arg_wins_over_auto_select() {
        let result =
            resolve_effective_model(Some("tiny.en"), Language::Swedish, true, WhisperModel::Base)
                .unwrap();
        assert_eq!(result, WhisperModel::TinyEn);
    }

    #[test]
    fn resolve_effective_model_invalid_arg_errors() {
        assert!(
            resolve_effective_model(Some("bogus"), Language::Auto, true, WhisperModel::Base)
                .is_err()
        );
    }

    #[test]
    fn neutral_language_detector_prefers_downloaded_base() {
        let selected = neutral_language_detection_model(|model| {
            matches!(model, WhisperModel::Base | WhisperModel::Tiny)
        });
        assert_eq!(selected, Some(WhisperModel::Base));
    }

    #[test]
    fn neutral_language_detector_falls_back_and_can_be_unavailable() {
        let selected =
            neutral_language_detection_model(|model| model == WhisperModel::LargeV3Turbo);
        assert_eq!(selected, Some(WhisperModel::LargeV3Turbo));
        assert_eq!(neutral_language_detection_model(|_| false), None);
    }

    #[test]
    fn uncertain_language_detector_escalates_to_downloaded_turbo() {
        let selected = accurate_language_detection_model(
            |model| {
                matches!(
                    model,
                    WhisperModel::LargeV3TurboQ8 | WhisperModel::LargeV3Turbo
                )
            },
            WhisperModel::Base,
        );
        assert_eq!(selected, Some(WhisperModel::LargeV3TurboQ8));
        assert_eq!(
            accurate_language_detection_model(
                |model| model == WhisperModel::LargeV3Turbo,
                WhisperModel::LargeV3Turbo,
            ),
            None
        );
    }
}

// -- diarize_threshold validation --
//
// Exercised via clap's try_parse_from against a small wrapper so the
// value_parser attribute itself (not just the bare parsing function) is
// under test.
#[cfg(all(test, feature = "diarization"))]
mod diarize_threshold_tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: TranscribeArgs,
    }

    fn parse_threshold(value: &str) -> Result<f32, String> {
        TestCli::try_parse_from(["sagascript", "file.wav", "--diarize-threshold", value])
            .map(|cli| cli.args.diarize_threshold)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn rejects_nan() {
        assert!(parse_threshold("nan").is_err());
    }

    #[test]
    fn rejects_negative() {
        assert!(parse_threshold("-1.0").is_err());
    }

    #[test]
    fn rejects_above_range() {
        assert!(parse_threshold("3.0").is_err());
    }

    #[test]
    fn accepts_default_value() {
        assert_eq!(parse_threshold("0.75").unwrap(), 0.75);
    }

    #[test]
    fn accepts_in_range_value() {
        assert_eq!(parse_threshold("1.5").unwrap(), 1.5);
    }

    #[test]
    fn default_applies_when_flag_omitted() {
        let cli = TestCli::try_parse_from(["sagascript", "file.wav"]).unwrap();
        assert_eq!(cli.args.diarize_threshold, 0.75);
    }

    #[test]
    fn accepts_boundary_values() {
        assert_eq!(parse_threshold("0.0").unwrap(), 0.0);
        assert_eq!(parse_threshold("2.0").unwrap(), 2.0);
    }

    #[test]
    fn meeting_json_requires_diarize_and_conflicts_with_legacy_outputs() {
        assert!(TestCli::try_parse_from(["sagascript", "file.wav", "--meeting-json"]).is_err());
        let cli =
            TestCli::try_parse_from(["sagascript", "file.wav", "--diarize", "--meeting-json"])
                .expect("meeting JSON should parse with diarization");
        assert!(cli.args.meeting_json);
        for conflict in ["--json", "--jsonl", "--clipboard", "--correct-hints"] {
            assert!(
                TestCli::try_parse_from([
                    "sagascript",
                    "file.wav",
                    "--diarize",
                    "--meeting-json",
                    conflict,
                ])
                .is_err(),
                "meeting JSON should conflict with {conflict}"
            );
        }
    }
}
