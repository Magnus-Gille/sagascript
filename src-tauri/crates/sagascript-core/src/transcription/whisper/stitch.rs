use super::TranscriptSegment;

/// Return whether a raw Whisper segment contains its exact no-speech marker.
///
/// The marker is only interpreted while assembling user-facing text. Raw
/// timestamped segments remain unchanged for diagnostics.
pub fn contains_no_speech_marker(text: &str) -> bool {
    text.contains("<|nospeech|>")
}

/// Assemble the plain transcript from Whisper's raw segments.
///
/// Whisper normally includes a leading space on each new segment. When it does
/// not, preserve the expected sentence boundary in the user-facing transcript
/// without changing the raw, timestamped segment text.
pub fn assemble_transcript(segments: &[TranscriptSegment]) -> String {
    let mut transcript = String::new();

    for segment in segments {
        // Whisper can prefix a silence hallucination with its internal
        // no-speech token. Dropping only the marker would still expose the
        // fabricated words that follow it, so discard the whole segment from
        // the plain user-facing transcript. Raw timestamped segments remain
        // unchanged for diagnostics.
        if contains_no_speech_marker(&segment.text) {
            continue;
        }

        let previous_ends_with_whitespace = transcript
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace);
        let next_starts_with_whitespace =
            segment.text.chars().next().is_some_and(char::is_whitespace);
        let previous_ends_sentence = transcript
            .trim_end()
            .chars()
            .next_back()
            .is_some_and(|c| matches!(c, '.' | '?' | '!'));

        if !transcript.is_empty()
            && !segment.text.is_empty()
            && !previous_ends_with_whitespace
            && !next_starts_with_whitespace
            && previous_ends_sentence
        {
            transcript.push(' ');
        }

        transcript.push_str(&segment.text);
    }

    separate_sentence_boundaries(&transcript)
}

const MIN_STITCH_ANCHOR_WORDS: usize = 4;
const MIN_STITCH_ANCHOR_CHARS: usize = 12;
const MAX_STITCH_OVERLAP_WORDS: usize = 96;
const MAX_STITCH_UNANCHORED_WORDS: usize = 32;

#[derive(Debug)]
struct StitchWord {
    normalized: String,
    start: usize,
}

/// Join independently decoded, overlapping chunks without repeating their
/// shared prefix. Matching is deliberately conservative: short repetitions
/// are preserved rather than risking deletion of real speech.
pub(super) fn stitch_chunk_results(
    chunk_results: Vec<Vec<TranscriptSegment>>,
    audio_duration: f64,
) -> Vec<TranscriptSegment> {
    let mut stitched: Vec<TranscriptSegment> = Vec::new();
    let mut previous_end = 0.0;

    for chunk in chunk_results {
        if !stitched.is_empty() && !chunk.is_empty() {
            let previous_text = concatenated_segment_text(&stitched);
            let chunk_text = concatenated_segment_text(&chunk);
            if let Some(suffix_start) = overlapping_suffix_start(&previous_text, &chunk_text) {
                trim_segment_suffix(&mut stitched, suffix_start);
            }
        }

        for mut segment in chunk {
            if segment.text.trim().is_empty() {
                continue;
            }
            (segment.start, segment.end) =
                sanitize_segment_bounds(segment.start, segment.end, audio_duration, previous_end);
            previous_end = segment.end;
            stitched.push(segment);
        }
    }

    stitched
}

fn concatenated_segment_text(segments: &[TranscriptSegment]) -> String {
    let capacity = segments.iter().map(|segment| segment.text.len()).sum();
    let mut text = String::with_capacity(capacity);
    for segment in segments {
        text.push_str(&segment.text);
    }
    text
}

fn overlapping_suffix_start(previous: &str, next: &str) -> Option<usize> {
    let previous_words = stitch_words(previous);
    let next_words = stitch_words(next);
    let previous_floor = previous_words
        .len()
        .saturating_sub(MAX_STITCH_OVERLAP_WORDS);
    let next_limit = next_words.len().min(MAX_STITCH_OVERLAP_WORDS);
    let mut best: Option<(usize, usize, usize)> = None;

    for previous_index in previous_floor..previous_words.len() {
        for next_index in 0..next_limit {
            if previous_index < next_index {
                continue;
            }
            let aligned_start = previous_index - next_index;
            if aligned_start < previous_floor {
                continue;
            }

            let mut anchor_words = 0;
            while previous_index + anchor_words < previous_words.len()
                && next_index + anchor_words < next_limit
                && previous_words[previous_index + anchor_words].normalized
                    == next_words[next_index + anchor_words].normalized
            {
                anchor_words += 1;
            }
            if anchor_words < MIN_STITCH_ANCHOR_WORDS
                || next_index > MAX_STITCH_UNANCHORED_WORDS
                || previous_words.len() - previous_index - anchor_words
                    > MAX_STITCH_UNANCHORED_WORDS
            {
                continue;
            }
            let anchor_chars = next_words[next_index..next_index + anchor_words]
                .iter()
                .map(|word| word.normalized.chars().count())
                .sum::<usize>();
            if anchor_chars < MIN_STITCH_ANCHOR_CHARS {
                continue;
            }

            let candidate = (next_index, usize::MAX - anchor_words, aligned_start);
            if best.is_none_or(|current| candidate < current) {
                best = Some(candidate);
            }
        }
    }

    best.map(|(_, _, aligned_start)| previous_words[aligned_start].start)
}

fn stitch_words(text: &str) -> Vec<StitchWord> {
    let mut words = Vec::new();
    let mut start = None;

    for (index, character) in text.char_indices() {
        let continues_word = character.is_alphanumeric()
            || (start.is_some() && matches!(character, '\'' | '’' | '-'));
        if continues_word {
            if start.is_none() && character.is_alphanumeric() {
                start = Some(index);
            }
        } else if let Some(word_start) = start.take() {
            let normalized = text[word_start..index]
                .trim_end_matches(['\'', '’', '-'])
                .to_lowercase();
            if !normalized.is_empty() {
                words.push(StitchWord {
                    normalized,
                    start: word_start,
                });
            }
        }
    }

    if let Some(word_start) = start {
        let normalized = text[word_start..]
            .trim_end_matches(['\'', '’', '-'])
            .to_lowercase();
        if !normalized.is_empty() {
            words.push(StitchWord {
                normalized,
                start: word_start,
            });
        }
    }

    words
}

fn trim_segment_suffix(segments: &mut Vec<TranscriptSegment>, suffix_start: usize) {
    let mut remaining = suffix_start;
    let mut retained = Vec::with_capacity(segments.len());

    for mut segment in segments.drain(..) {
        if remaining > segment.text.len() {
            remaining -= segment.text.len();
            retained.push(segment);
            continue;
        }

        debug_assert!(segment.text.is_char_boundary(remaining));
        segment.text.truncate(remaining);
        let trimmed_len = segment.text.trim_end().len();
        segment.text.truncate(trimmed_len);
        if !segment.text.is_empty() {
            retained.push(segment);
        }
        break;
    }

    *segments = retained;
}

/// Insert a missing word boundary after sentence-ending punctuation when the
/// following character clearly begins a new sentence. Whisper can omit this
/// separator inside a single raw segment, so repairing only segment joins is
/// insufficient. Requiring an uppercase letter keeps decimals such as `1.2`
/// unchanged, while a one-character lookahead preserves the internal dots in
/// initialisms such as `U.S.A.`.
fn separate_sentence_boundaries(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut word_len = 0usize;
    let mut word_is_single_uppercase = false;

    while let Some(current) = chars.next() {
        let previous_word_is_initial = word_len == 1 && word_is_single_uppercase;
        let continues_initialism = current == '.'
            && previous_word_is_initial
            && chars.peek().is_some_and(|next| next.is_uppercase())
            && chars.clone().nth(1) == Some('.');
        result.push(current);
        if matches!(current, '.' | '?' | '!')
            && chars.peek().is_some_and(|next| next.is_uppercase())
            && !continues_initialism
        {
            result.push(' ');
        }

        if current.is_alphanumeric() {
            word_len += 1;
            word_is_single_uppercase = word_len == 1 && current.is_uppercase();
        } else if !matches!(current, '\'' | '’') {
            word_len = 0;
            word_is_single_uppercase = false;
        }
    }

    result
}

/// Remove any `<|...|>` special/timestamp tokens that could leak into segment
/// text now that generative timestamps are enabled.
pub(super) fn strip_timestamp_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<|") {
        out.push_str(&rest[..start]);
        match rest[start..].find("|>") {
            Some(end) => rest = &rest[start + end + 2..],
            None => {
                rest = &rest[start..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Bound whisper's segment timestamps to the source audio and preserve output
/// ordering. Whisper can report an end past the audio duration on the padded
/// final window, or (historically, with `no_timestamps`) an internal seek offset
/// as a negative start. Those are not valid media timestamps and must not leak
/// into the CLI JSON contract.
pub(super) fn sanitize_segment_bounds(
    raw_start: f64,
    raw_end: f64,
    audio_duration: f64,
    previous_end: f64,
) -> (f64, f64) {
    let duration = if audio_duration.is_finite() {
        audio_duration.max(0.0)
    } else {
        0.0
    };
    let floor = previous_end.clamp(0.0, duration);
    let start = if raw_start.is_finite() {
        raw_start.clamp(floor, duration)
    } else {
        floor
    };
    let end = if raw_end.is_finite() {
        raw_end.clamp(start, duration)
    } else {
        start
    };
    (start, end)
}

#[cfg(test)]
#[path = "stitch_tests.rs"]
mod tests;
