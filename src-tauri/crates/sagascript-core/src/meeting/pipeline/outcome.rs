//! What one file's pipeline hands back (#343): the result, the plain text of a diarized run and
//! the warnings both pipelines print.

use super::narrate::Narrator;
#[cfg(feature = "diarization")]
use crate::diarization::DiarizedSegment;
#[cfg(feature = "diarization")]
use crate::meeting::MeetingTranscript;
use crate::models::Language;

use super::narrate::errln;
use crate::transcription::diagnostics::{
    language_mismatch_warning, CoverageDiagnostics, LanguageDetection, TranscriptionWarning,
};
#[cfg(feature = "diarization")]
use crate::transcription::whisper_backend::contains_no_speech_marker;

#[derive(Debug)]
pub struct FileTranscription {
    pub json: serde_json::Value,
    pub plain: String,
    #[cfg(feature = "diarization")]
    pub meeting: Option<MeetingTranscript>,
}

#[cfg(feature = "diarization")]
pub fn assemble_diarized_plain_text(segments: &[DiarizedSegment]) -> String {
    segments
        .iter()
        .filter(|segment| !contains_no_speech_marker(&segment.text))
        .map(|segment| format!("[{}] {}", segment.speaker, segment.text.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn combined_warnings(
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

pub fn emit_warnings(out: &dyn Narrator, warnings: &[TranscriptionWarning]) {
    for warning in warnings {
        errln!(out, "Warning [{}]: {}", warning.code, warning.message);
    }
}
