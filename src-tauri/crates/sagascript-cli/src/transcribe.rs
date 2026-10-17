use std::collections::HashSet;
#[cfg(feature = "diarization")]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
#[cfg(feature = "diarization")]
use std::time::SystemTime;

use clap::Args;
#[cfg(feature = "diarization")]
use sha2::{Digest, Sha256};

use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::decoder::SUPPORTED_EXTENSIONS;
#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizedSegment;
#[cfg(feature = "diarization")]
use sagascript_core::diarization_report::DecoderEvidence;
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
use sagascript_core::run_context::RunContext;
use sagascript_core::settings::{FileModel, FileModelPreference, Language, Settings, WhisperModel};
use sagascript_core::transcription::diagnostics::{
    analyze_coverage, analyze_language_windows, analyze_repetition, language_mismatch_warning,
    CoverageDiagnostics, LanguageDetection, LanguageRegionDiagnostics, LanguageWindow,
    TranscriptionWarning,
};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::diagnostics::{analyze_coverage_profile, CoverageProfile};
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::whisper_backend::assemble_transcript;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::contains_no_speech_marker;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::validate_diarization_options;
use sagascript_core::transcription::{
    normalize_nonspeech_markers, recommended_parallel_chunks, ContextProfile, Glossary,
    TranscribeOptions, TranscriptSegment, WhisperBackend,
};

use crate::exit::{dictation_error_code, ErrorObject, JsonOutput};
use crate::output::{errln, outln, Out, WriteError};
use crate::run_request::RunRequest;

#[derive(Args)]
pub struct TranscribeArgs {
    /// Emit JSON progress events on stderr even without a terminal. Percent is
    /// local to the named stage; stdout remains the transcript output. Diarized
    /// events include the effective decoder settings.
    #[arg(long)]
    pub progress_json: bool,
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

    /// File model ID to use [see: sagascript list-models]
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

    /// Experimental: embed only same-region samples where one track is active
    /// and all other diarization tracks are inactive.
    #[cfg(feature = "diarization")]
    #[arg(long, requires = "diarize")]
    pub diarize_exclusive_speech_embeddings: bool,

    /// Emit the validated shared meeting transcript JSON document.
    /// Requires diarization and cannot be combined with legacy JSON outputs.
    #[cfg(feature = "diarization")]
    #[arg(
        long,
        requires = "diarize",
        conflicts_with_all = ["json", "jsonl", "clipboard", "correct_hints"]
    )]
    pub meeting_json: bool,

    /// Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.34). Higher = fewer speakers.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "THRESHOLD", default_value = "0.34",
          value_parser = parse_diarize_threshold,
          help = "Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.34). Higher = fewer speakers.")]
    pub diarize_threshold: f32,

    /// Include local region/word evidence, distances and ASR diagnostics in JSON.
    /// These measurements are not calibrated speaker probabilities.
    #[cfg(feature = "diarization")]
    #[arg(long, requires = "diarize")]
    pub diarize_diagnostics: bool,

    #[cfg(feature = "diarization")]
    #[command(flatten)]
    pub speaker_count: crate::speaker_args::SpeakerCountArgs,

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

    /// Pianissimo only: steer decoding with the profile glossary terms (context
    /// biasing). The value is the logit bonus for dictionary-continuing tokens
    /// (try 1-5; 0 or absent = off).
    #[arg(long, value_name = "WEIGHT")]
    pub glossary_boost: Option<f32>,

    /// Pianissimo only: extra context-biasing terms, one per line (experiments).
    /// Used together with --glossary-boost.
    #[arg(long, value_name = "FILE", requires = "glossary_boost")]
    pub boost_terms_file: Option<PathBuf>,

    /// Opt in to strict one-edit vocabulary correction from the selected hint.
    /// Requires --json so every applied correction is reported with its source
    /// text and segment confidence. Only single-word, unambiguous hints apply.
    #[arg(long, requires = "json")]
    pub correct_hints: bool,

    /// Enable voice activity detection (Silero VAD) to skip non-speech regions,
    /// reducing silence hallucination and repetition loops. Downloads a small
    /// model on first use. Overrides the `vad_enabled` setting. Diarized VAD is
    /// opt-in even when the saved setting is enabled, and
    /// uses source-mapped segment timing instead of DTW word timing.
    #[arg(long)]
    pub vad: bool,

    /// Disable VAD for this run, even if the `vad_enabled` setting is on.
    #[arg(long, conflicts_with = "vad")]
    pub no_vad: bool,

    /// Beam search width: 0 = greedy (fast), >=2 = beam search (more accurate,
    /// slower). Overrides the saved `beam_size` setting. When omitted, a saved
    /// `beam_size` >=2 is used; otherwise ordinary file transcription defaults
    /// to 5. Diarized transcription always defaults to 0 regardless of saved
    /// settings and accepts explicit values of 0 or 2–8.
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

impl<'a> From<&'a TranscribeArgs> for RunRequest<'a> {
    fn from(args: &'a TranscribeArgs) -> Self {
        Self {
            profile: args.profile.as_deref(),
            language: args.language.as_deref(),
            prompt: args.prompt.as_deref(),
            prompt_file: args.prompt_file.as_deref(),
        }
    }
}

#[derive(Debug)]
struct FileTranscription {
    json: serde_json::Value,
    plain: String,
    #[cfg(feature = "diarization")]
    meeting: Option<MeetingTranscript>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CachePolicy {
    Normal,
    #[cfg(feature = "diarization")]
    RefreshNew,
}

impl CachePolicy {
    #[cfg(feature = "diarization")]
    fn reads_existing_cache(self) -> bool {
        matches!(self, Self::Normal)
    }
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
pub(crate) fn prepare_diarized_plain_segments(
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
            return Err(DictationError::Cancelled);
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

#[cfg(feature = "diarization")]
#[derive(Debug, Default, serde::Serialize)]
struct DiarizationPerformance {
    acceleration_backend: &'static str,
    coreml_status: &'static str,
    cache_hit: bool,
    effective_beam_size: u32,
    effective_temperature_fallback: bool,
    effective_vad: bool,
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
    output: Result<FileTranscription, DictationError>,
}

/// One input's outcome in `--json` arrays and `--jsonl` lines:
/// `{"status":"ok","source":…,"result":…}` or `{"status":"error","source":…,"error":…}`.
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
        /// For the single-input `--json` error object only; batch items keep
        /// their three keys.
        #[serde(skip)]
        code: String,
    },
}

/// Where `transcribe` writes each input's outcome (#364). The Whisper and
/// Pianissimo batches share it, so the three output modes are defined once:
///
/// - plain text: each transcript on stdout, under a `==> file <==` header when
///   there are several inputs;
/// - `--json`: one document after the batch: the result object for a single
///   input (or its error object when it failed), otherwise an array of items;
/// - `--jsonl`: one compact item per line, written as soon as the input is done.
///
/// A failed input is also reported as `Error transcribing …` on stderr.
enum BatchSink {
    Plain { headers: bool },
    Json { single: bool, items: Vec<BatchItem> },
    Jsonl,
}

impl BatchSink {
    fn new(args: &TranscribeArgs, inputs: usize) -> Self {
        if args.jsonl {
            Self::Jsonl
        } else if args.json {
            Self::Json {
                single: inputs == 1,
                items: Vec::with_capacity(inputs),
            }
        } else {
            Self::Plain {
                headers: inputs > 1,
            }
        }
    }

    fn emit(
        &mut self,
        out: &Out,
        args: &TranscribeArgs,
        execution: BatchExecution,
    ) -> Result<(), DictationError> {
        let file = execution.source;
        match execution.output {
            Ok(output) => {
                #[cfg(feature = "diarization")]
                if args.meeting_json {
                    let meeting = output.meeting.as_ref().ok_or_else(|| {
                        DictationError::TranscriptionFailed(
                            "meeting JSON was not produced by the diarized pipeline".to_string(),
                        )
                    })?;
                    out.json(meeting)?;
                    return Ok(());
                }
                if args.clipboard {
                    copy_to_clipboard(&output.plain)?;
                    errln!(out, "Copied to clipboard.");
                }
                let item = BatchItem::Ok {
                    source: file.display().to_string(),
                    result: output.json,
                };
                match self {
                    Self::Jsonl => out.jsonl(&item)?,
                    Self::Json { items, .. } => items.push(item),
                    Self::Plain { headers } => {
                        if *headers {
                            outln!(out, "==> {} <==", file.display())?;
                        }
                        outln!(out, "{}", output.plain)?;
                    }
                }
            }
            Err(error) => {
                errln!(out, "Error transcribing {}: {error}", file.display());
                let item = BatchItem::Error {
                    source: file.display().to_string(),
                    error: error.to_string(),
                    code: dictation_error_code(&error),
                };
                match self {
                    Self::Jsonl => out.jsonl(&item)?,
                    Self::Json { items, .. } => items.push(item),
                    Self::Plain { .. } => {}
                }
            }
        }
        Ok(())
    }

    /// Writes the `--json` document once the batch is done.
    fn finish(self, out: &Out) -> Result<(), WriteError> {
        let Self::Json { single, mut items } = self else {
            return Ok(());
        };
        if single && items.len() == 1 {
            match items.pop() {
                Some(BatchItem::Ok { result, .. }) => return out.json(&result),
                Some(BatchItem::Error {
                    source,
                    error,
                    code,
                }) => {
                    return ErrorObject::new(&code, Some(&source), &error)
                        .write(out, JsonOutput::Pretty)
                }
                None => {}
            }
        }
        out.json(&items)
    }
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
        cache_policy,
        &Out::stdio(),
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

/// The validated speaker-count hint from `--speakers`/`--min-speakers`/`--max-speakers`.
#[cfg(feature = "diarization")]
fn speaker_hint_from_args(
    args: &TranscribeArgs,
) -> Result<Option<sagascript_core::speaker_hint::SpeakerCountHint>, DictationError> {
    let hint = args
        .speaker_count
        .hint()
        .map_err(|message| DictationError::SettingsError(format!("--speakers: {message}")))?;
    if hint.is_some() && !args.diarize {
        return Err(DictationError::SettingsError(
            "--speakers, --min-speakers, --max-speakers and --force-speakers require --diarize"
                .to_string(),
        ));
    }
    Ok(hint)
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

    // Model loading is lazy so a validated diarization-cache hit never maps
    // several gigabytes of weights that it will not use. Batch processing is
    // sequential, so this flag safely preserves the load-once behavior.
    let backend = std::sync::Arc::new(WhisperBackend::new());
    let _abort_whisper = {
        let backend = backend.clone();
        crate::cancel::register(move || backend.request_cancel())
    };
    let mut model_loaded = false;

    let mut sink = BatchSink::new(&args, files.len());
    let (processed, failures) = process_batch(
        &files,
        args.fail_fast,
        |index, file| {
            errln!(out, "[{}/{}] {}", index + 1, files.len(), file.display());
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
                CachePolicy::Normal,
                out,
            )
        },
        |execution| sink.emit(out, &args, execution),
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

struct PianissimoPerformance {
    model_load_seconds: f64,
    model_verification_seconds: f64,
    decode_resample_seconds: f64,
    total_seconds: f64,
    /// Engine the host reported in `hello` (`coreml`, `onnx`); `unknown` if it never connected.
    engine: String,
    engine_warm: bool,
    /// Per-window host stage timings.
    windows: Vec<sagascript_core::transcription::engine_host::WindowTiming>,
}

/// Per-file JSON for `--model pianissimo-sv`. The key set is a compatibility
/// contract: only additive keys (`performance.engine`, `performance.engine_warm`,
/// `performance.windows`)
/// may be added to what the NeMo-based release emitted.
fn pianissimo_file_json(
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

fn run_pianissimo_batch(
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
    let backend = std::sync::Arc::new(PianissimoBackend::start_with_cancel(cancel_flag)?);
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

fn progress_step(phase: &str) -> Option<u8> {
    match phase {
        "decoding" => Some(1),
        "resampling" | "loading" | "language_detection" | "preparing" | "encoding" => Some(2),
        "transcribing" | "finalizing" | "completed" => Some(3),
        _ => None,
    }
}

fn progress_event(
    phase: &str,
    percent: Option<u8>,
    step: Option<u8>,
    steps_total: Option<u8>,
    elapsed_ms: u128,
) -> serde_json::Value {
    serde_json::json!({
        "event": "transcription_progress",
        "phase": phase,
        "step": step,
        "steps_total": steps_total,
        "percent": percent,
        "elapsed_ms": elapsed_ms,
    })
}

fn progress_event_with_decoder(
    phase: &str,
    percent: Option<u8>,
    step: Option<u8>,
    steps_total: Option<u8>,
    elapsed_ms: u128,
    decoder: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut event = progress_event(phase, percent, step, steps_total, elapsed_ms);
    if let Some(decoder) = decoder {
        event["decoder"] = decoder.clone();
    }
    event
}

fn emit_progress(out: &Out, enabled: bool, started: Instant, phase: &str, percent: Option<u8>) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event(
                phase,
                percent,
                progress_step(phase),
                Some(3),
                started.elapsed().as_millis(),
            )
        );
    }
}

fn emit_progress_with_decoder(
    out: &Out,
    enabled: bool,
    started: Instant,
    phase: &str,
    percent: Option<u8>,
    decoder: Option<&serde_json::Value>,
) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event_with_decoder(
                phase,
                percent,
                progress_step(phase),
                Some(3),
                started.elapsed().as_millis(),
                decoder,
            )
        );
    }
}

#[cfg(feature = "diarization")]
fn emit_meeting_progress(
    out: &Out,
    enabled: bool,
    started: Instant,
    phase: &str,
    decoder: Option<&serde_json::Value>,
) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event_with_decoder(
                phase,
                None,
                None,
                None,
                started.elapsed().as_millis(),
                decoder,
            )
        );
    }
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

fn ensure_model_loaded(
    out: &Out,
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
    errln!(out, "Loading model: {}...", model.display_name());
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
        let segments = backend.transcribe_sync_with_gap_recovery(
            &audio,
            language,
            &opts,
            move |pct| {
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
            },
            Some(&on_encode_start),
        )?;
        pb.finish_and_clear();
        segments
    } else {
        errln!(out, "Transcribing...");
        let progress_out = out.clone();
        backend.transcribe_sync_with_gap_recovery(
            &audio,
            language,
            &opts,
            move |pct| {
                if pct > 1 {
                    emit_progress(
                        &progress_out,
                        progress_json,
                        file_started,
                        "transcribing",
                        Some(pct.clamp(0, 100) as u8),
                    );
                }
            },
            Some(&on_encode_start),
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
    options: &TranscribeOptions,
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
            let transcription = backend.transcribe_sync_for_diarization_profiled_with_options(
                audio, language, prompt, options,
            );
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
        let transcription = backend.transcribe_sync_for_diarization_profiled_with_options(
            audio, language, prompt, options,
        )?;
        Ok((analysis, timings, transcription))
    }
}

#[cfg(feature = "diarization")]
fn canonicalize_coverage_segments(
    segments: Vec<TranscriptSegment>,
    duration: f64,
) -> Vec<TranscriptSegment> {
    if !duration.is_finite() || duration < 0.0 {
        return Vec::new();
    }

    segments
        .into_iter()
        .filter_map(|mut segment| {
            if !segment.start.is_finite() || !segment.end.is_finite() {
                return None;
            }
            segment.start = segment.start.clamp(0.0, duration);
            segment.end = segment.end.clamp(0.0, duration);
            (segment.end > segment.start).then_some(segment)
        })
        .collect()
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

fn emit_warnings(out: &Out, warnings: &[TranscriptionWarning]) {
    for warning in warnings {
        errln!(out, "Warning [{}]: {}", warning.code, warning.message);
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
    out: &Out,
    backend: &WhisperBackend,
    audio: &[f32],
    configured_language: Language,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    detect_language_regions_with(out, audio, configured_language, |window| {
        backend.detect_language(window)
    })
}

fn detect_language_regions_with(
    out: &Out,
    audio: &[f32],
    configured_language: Language,
    mut detect: impl FnMut(&[f32]) -> Result<Option<LanguageDetection>, DictationError>,
) -> Result<Option<LanguageRegionDiagnostics>, DictationError> {
    let windows = language_window_plan(audio.len(), configured_language);
    if windows.is_empty() {
        return Ok(None);
    }

    errln!(
        out,
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
    out: &Out,
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
        errln!(
            out,
            "Warning: language mismatch check skipped because no neutral multilingual model is downloaded."
        );
        return Ok(None);
    };
    errln!(
        out,
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
    errln!(
        out,
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

/// Resolve the diarization decoder independently from ordinary file defaults.
/// Explicit CLI values win. Omitted flags preserve the legacy greedy/no-VAD,
/// temperature-fallback decoding regardless of ordinary file settings.
#[cfg(feature = "diarization")]
fn resolve_diarization_options(
    args: &TranscribeArgs,
    _stored: &Settings,
) -> Result<TranscribeOptions, DictationError> {
    let beam_size = args.beam_size.unwrap_or(0);
    let vad_enabled = args.vad && !args.no_vad;
    let options = TranscribeOptions {
        prompt: None,
        beam_size,
        temperature_fallback: true,
        vad_model_path: vad_enabled.then(|| model::vad_model_path().to_string_lossy().into_owned()),
        segment_timestamps: true,
        parallel_chunks: 1,
    };
    validate_diarization_options(&options)?;
    Ok(options)
}

#[cfg(feature = "diarization")]
fn decoder_progress_value(decoder: &DecoderEvidence, meeting_json: bool) -> serde_json::Value {
    if meeting_json {
        // Native meeting JSON serializes typed f32 fields directly. Match that
        // wire representation without changing the legacy JSON report format.
        let native = serde_json::to_string(decoder).expect("decoder evidence serializes");
        serde_json::from_str(&native).expect("serialized decoder evidence is valid JSON")
    } else {
        serde_json::to_value(decoder).expect("decoder evidence serializes")
    }
}

#[cfg(feature = "diarization")]
fn effective_diarization_decoder_evidence(
    options: &TranscribeOptions,
) -> Result<DecoderEvidence, DictationError> {
    Ok(
        sagascript_core::transcription::whisper_backend::effective_diarization_decoder_config(
            options,
        )?
        .into(),
    )
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

/// Resolve a file run separately from the live `record` model. `Auto` keeps
/// the CLI's existing fallback semantics for installations without a file
/// preference; an explicit file choice is checked against the requested
/// language rather than silently changing engines.
pub fn resolve_file_model(
    model_arg: Option<&str>,
    language: Language,
    settings: &Settings,
) -> Result<FileModel, DictationError> {
    resolve_file_model_for_run(model_arg, language, settings, false)
}

/// [`resolve_file_model`] for a run that may need Whisper-only features
/// (diarization, meeting output, decoder hints, beam/parallel/VAD options).
/// An implicit choice (no `--model`, file preference `Auto`) then resolves to
/// the recommended Whisper model instead of Pianissimo.
pub fn resolve_file_model_for_run(
    model_arg: Option<&str>,
    language: Language,
    settings: &Settings,
    needs_whisper: bool,
) -> Result<FileModel, DictationError> {
    if let Some(id) = model_arg {
        let preference =
            FileModelPreference::parse_id(id).map_err(DictationError::SettingsError)?;
        if preference == FileModelPreference::Auto {
            return Err(DictationError::SettingsError(
                "--model needs a concrete model ID; omit it to use the file default".into(),
            ));
        }
        let mut run = settings.clone();
        run.file_transcription_model = preference;
        return run
            .effective_file_model_for(language)
            .map_err(DictationError::SettingsError);
    }
    if needs_whisper {
        return settings
            .effective_whisper_file_model_for(language)
            .map_err(DictationError::SettingsError);
    }
    settings
        .effective_file_model_for(language)
        .map_err(DictationError::SettingsError)
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

pub(crate) fn resolve_one_run_prompt(
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
#[path = "transcribe_tests.rs"]
mod tests;
