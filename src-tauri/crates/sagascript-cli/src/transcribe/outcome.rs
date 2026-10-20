//! What one file's pipeline hands back (#343): the result, the plain text of a diarized run and
//! the warnings both pipelines print.

#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizedSegment;
use sagascript_core::meeting::pipeline::narrate::Narrator;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::MeetingTranscript;
use sagascript_core::settings::Language;

use crate::output::errln;
use sagascript_core::transcription::diagnostics::{
    language_mismatch_warning, CoverageDiagnostics, LanguageDetection, TranscriptionWarning,
};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::whisper_backend::contains_no_speech_marker;

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

pub(super) fn emit_warnings(out: &dyn Narrator, warnings: &[TranscriptionWarning]) {
    for warning in warnings {
        errln!(out, "Warning [{}]: {}", warning.code, warning.message);
    }
}
