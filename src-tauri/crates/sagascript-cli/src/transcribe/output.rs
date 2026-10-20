//! What a `transcribe` run writes (#343): the per-file result, the three output
//! modes of the batch (`BatchSink`), the diarized plain text, the warnings and
//! the clipboard.

use std::path::{Path, PathBuf};

use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::diagnostics::{
    analyze_coverage, analyze_repetition, TranscriptionWarning,
};
use sagascript_core::transcription::whisper_backend::assemble_transcript;
use sagascript_core::transcription::{normalize_nonspeech_markers, Glossary, TranscriptSegment};

use super::args::TranscribeArgs;
use super::inference::LanguageEvidence;
use super::outcome::{combined_warnings, FileTranscription};
use super::resolve::model_id_string;
use super::vocab_correction::{apply_glossary_corrections, apply_hint_corrections};
use crate::exit::{dictation_error_code, ErrorObject, JsonOutput};
use crate::output::{errln, outln, Out, WriteError};

/// What the plain path knows when the backend has answered.
pub(super) struct PlainInput<'a> {
    /// What Whisper said, with its timing and confidence.
    pub(super) segments: Vec<TranscriptSegment>,
    pub(super) audio: &'a [f32],
    pub(super) language: Language,
    pub(super) model: WhisperModel,
    pub(super) file: &'a Path,
    pub(super) duration: f64,
    pub(super) glossary: &'a Glossary,
    /// The vocabulary `--correct-hints` repairs towards; `None` without the flag.
    pub(super) hint_vocabulary: Option<&'a [String]>,
    pub(super) language_evidence: &'a LanguageEvidence,
}

/// The plain path's result: the `--json` object, the text, and the warnings
/// the caller writes to stderr.
pub(super) struct PlainResult {
    pub(super) json: serde_json::Value,
    pub(super) text: String,
    pub(super) warnings: Vec<TranscriptionWarning>,
}

/// Turns what Whisper said into the plain result: the glossary and hint
/// corrections, the repetition loops set aside, the displayed text, the
/// coverage and language warnings, and the documented `--json` object. No
/// output, no clock and no backend: the same input gives the same result.
pub(super) fn assemble_plain_result(input: PlainInput<'_>) -> PlainResult {
    let PlainInput {
        mut segments,
        audio,
        language,
        model,
        file,
        duration,
        glossary,
        hint_vocabulary,
        language_evidence,
    } = input;
    let mut corrections = apply_glossary_corrections(&mut segments, glossary);
    if let Some(vocabulary) = hint_vocabulary {
        corrections.extend(apply_hint_corrections(&mut segments, vocabulary));
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
    let coverage = analyze_coverage(audio, &trusted_segments);
    let mut warnings = combined_warnings(
        &coverage,
        language,
        language_evidence.detected_language.as_ref(),
    );
    if let Some(diagnostics) = &language_evidence.language_regions {
        warnings.extend(diagnostics.warnings.clone());
    }
    warnings.extend(repetition.warnings.clone());

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
        "detected_language": language_evidence.detected_language,
        "language_redetection_enabled": language_evidence.language_regions.is_some(),
        "language_regions": language_evidence.language_regions.as_ref().map(|diagnostics| &diagnostics.regions),
        "warnings": warnings,
        "vocabulary_corrections": corrections,
    });
    PlainResult {
        json,
        text,
        warnings,
    }
}

#[derive(Debug)]
pub(super) struct BatchExecution {
    pub(super) source: PathBuf,
    pub(super) output: Result<FileTranscription, DictationError>,
}

/// One input's outcome in `--json` arrays and `--jsonl` lines:
/// `{"status":"ok","source":…,"result":…}` or `{"status":"error","source":…,"error":…}`.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub(super) enum BatchItem {
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
pub(super) enum BatchSink {
    Plain { headers: bool },
    Json { single: bool, items: Vec<BatchItem> },
    Jsonl,
}

impl BatchSink {
    pub(super) fn new(args: &TranscribeArgs, inputs: usize) -> Self {
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

    pub(super) fn emit(
        &mut self,
        out: &Out,
        args: &TranscribeArgs,
        clipboard: &dyn Clipboard,
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
                    clipboard.copy(&output.plain)?;
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
    pub(super) fn finish(self, out: &Out) -> Result<(), WriteError> {
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

/// Where `--clipboard` puts a transcript: a seam so a test needs no display
/// server. [`SystemClipboard`] is the one implementation that ships.
pub(super) trait Clipboard {
    fn copy(&self, text: &str) -> Result<(), DictationError>;
}

/// The clipboard of the desktop session.
pub(super) struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn copy(&self, text: &str) -> Result<(), DictationError> {
        copy_to_clipboard(text)
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
