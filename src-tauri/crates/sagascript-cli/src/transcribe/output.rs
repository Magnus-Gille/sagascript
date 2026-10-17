//! What a `transcribe` run writes (#343): the per-file result, the three output
//! modes of the batch (`BatchSink`), the diarized plain text, the warnings and
//! the clipboard.

use std::path::PathBuf;

#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizedSegment;
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::settings::Language;
use sagascript_core::transcription::diagnostics::{
    language_mismatch_warning, CoverageDiagnostics, LanguageDetection, TranscriptionWarning,
};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::contains_no_speech_marker;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::{normalize_nonspeech_markers, Glossary};

use super::args::TranscribeArgs;
use crate::exit::{dictation_error_code, ErrorObject, JsonOutput};
use crate::output::{errln, outln, Out, WriteError};

#[derive(Debug)]
pub(super) struct FileTranscription {
    pub(super) json: serde_json::Value,
    pub(super) plain: String,
    #[cfg(feature = "diarization")]
    pub(super) meeting: Option<MeetingTranscript>,
}

#[cfg(feature = "diarization")]
pub(super) fn assemble_diarized_plain_text(segments: &[DiarizedSegment]) -> String {
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

pub(super) fn combined_warnings(
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

pub(super) fn emit_warnings(out: &Out, warnings: &[TranscriptionWarning]) {
    for warning in warnings {
        errln!(out, "Warning [{}]: {}", warning.code, warning.message);
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
