//! From the speaker segments of a diarized run to the meeting document (#343, #374): the plain
//! segments both the `--json` output and the meeting document are made of, and the document.

use std::collections::HashSet;

use crate::diarization::merge::consolidate;
use crate::diarization::DiarizedSegment;
use crate::error::DictationError;
use crate::glossary::Glossary;
use crate::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
use crate::models::{Language, WhisperModel};
use crate::transcription::normalize_nonspeech_markers;
use crate::transcription::whisper_backend::contains_no_speech_marker;

/// The segments of a diarized run as its output shows them: segments that are only a no-speech
/// marker dropped, neighbours of one speaker consolidated, non-speech markers in the language's
/// spelling, and the glossary applied to each.
pub fn prepare_diarized_plain_segments(
    segments: &[DiarizedSegment],
    language: Language,
    glossary: &Glossary,
) -> Vec<DiarizedSegment> {
    let filtered = segments
        .iter()
        .filter(|segment| !contains_no_speech_marker(&segment.text))
        .cloned()
        .collect::<Vec<_>>();
    let mut consolidated = consolidate(&filtered);
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

/// The shared meeting document of a diarized run, from its plain segments: one speaker per
/// distinct speaker ID in order of appearance, segment ends clamped to the audio duration.
pub fn meeting_transcript_from_plain_segments(
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
        model.id(),
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
