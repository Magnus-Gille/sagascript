//! Opt-in vocabulary corrections of transcript segments (#343): the explicit
//! aliases of the glossary and the strict one-edit `--correct-hints` repair.

use sagascript_core::glossary::{squash_key, squash_windows, Glossary};
use sagascript_core::transcription::TranscriptSegment;

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
    glossary: &Glossary,
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

        let (corrected, segment_corrections) = correct_segment(
            &segment.text,
            vocabulary,
            glossary,
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

/// Shortest squashed hint term a split may repair to (#580); explicit aliases
/// need 5 characters.
const SQUASH_MIN_LEN: usize = 4;

/// Repairs split words first (exact squash match, 2–3 words, only to a term
/// no neighbour word is part of), then runs the one-edit repair on the text
/// in between, so a repaired term is never edited again.
fn correct_segment(
    text: &str,
    vocabulary: &[String],
    glossary: &Glossary,
    segment_index: usize,
    avg_logprob: f32,
    no_speech_prob: f32,
) -> (String, Vec<VocabularyCorrection>) {
    // Hint terms and the glossary's explicit aliases are the only targets: an
    // alias in an Auto or hint-only glossary does not exist, so none applies.
    let mut keys: Vec<(String, &String)> = vocabulary
        .iter()
        .map(|term| (squash_key(term), term))
        .filter(|(key, _)| key.chars().count() >= SQUASH_MIN_LEN)
        .collect();
    keys.extend(
        glossary
            .squash_aliases()
            .iter()
            .map(|(key, canonical)| (key.clone(), canonical)),
    );
    keys.sort();
    keys.dedup();
    let is_term = |word: &str| {
        let key = squash_key(word);
        vocabulary.iter().any(|term| squash_key(term) == key)
    };
    let mut windows = squash_windows(text, 3);
    windows.retain(|window| window.words.len() > 1);
    // Longest first at each start; the left-to-right scan keeps them disjoint.
    windows.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for window in windows {
        if spans.last().is_some_and(|(_, end, _)| window.start < *end) {
            continue;
        }
        if window
            .words
            .iter()
            .any(|(start, end)| is_term(&text[*start..*end]))
        {
            continue;
        }
        let mut matches = keys.iter().filter(|(key, _)| *key == window.key);
        if let (Some((_, term)), None) = (matches.next(), matches.next()) {
            spans.push((window.start, window.end, (*term).clone()));
        }
    }

    let mut corrected = String::with_capacity(text.len());
    let mut corrections = Vec::new();
    let mut cursor = 0;
    for (start, end, replacement) in spans {
        let (gap, gap_corrections) = correct_segment_text(
            &text[cursor..start],
            vocabulary,
            segment_index,
            avg_logprob,
            no_speech_prob,
        );
        corrected.push_str(&gap);
        corrections.extend(gap_corrections);
        corrections.push(VocabularyCorrection {
            segment_index,
            original: text[start..end].to_string(),
            replacement: replacement.clone(),
            method: "squash_split",
            avg_logprob: Some(avg_logprob),
            no_speech_prob,
        });
        corrected.push_str(&replacement);
        cursor = end;
    }
    let (tail, tail_corrections) = correct_segment_text(
        &text[cursor..],
        vocabulary,
        segment_index,
        avg_logprob,
        no_speech_prob,
    );
    corrected.push_str(&tail);
    corrections.extend(tail_corrections);
    (corrected, corrections)
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
