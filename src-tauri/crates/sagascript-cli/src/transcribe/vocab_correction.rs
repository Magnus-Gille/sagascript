//! Opt-in vocabulary corrections of transcript segments (#343): the explicit
//! aliases of the glossary and the strict one-edit `--correct-hints` repair.

use sagascript_core::transcription::{Glossary, TranscriptSegment};

/// Segment scores in this band are uncertain enough to consider an explicit
/// user-supplied spelling, but not so unreliable that a blind edit is useful.
const VOCAB_CORRECTION_MIN_LOGPROB: f32 = -0.8;

const VOCAB_CORRECTION_MAX_LOGPROB: f32 = -0.3;

const VOCAB_CORRECTION_MAX_NO_SPEECH_PROB: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub(super) struct VocabularyCorrection {
    pub(super) segment_index: usize,
    pub(super) original: String,
    pub(super) replacement: String,
    pub(super) method: &'static str,
    pub(super) avg_logprob: Option<f32>,
    pub(super) no_speech_prob: f32,
}

pub(super) fn apply_hint_corrections(
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

pub(super) fn apply_glossary_corrections(
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
