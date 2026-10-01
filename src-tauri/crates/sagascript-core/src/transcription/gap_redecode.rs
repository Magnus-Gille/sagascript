//! Recovery for speech that Whisper's long-audio windowing skipped (#273).
//!
//! whisper.cpp advances a full 30 s window when a decode ends in a lone
//! timestamp token ("single timestamp ending - skip entire chunk"). If the
//! decoder stopped early inside a window, everything after that point in the
//! window is never transcribed. The coverage check detects such gaps; this
//! module re-decodes just those spans once and merges the result in time order.
//!
//! Cost and hallucination guards: the RMS-based speech detector can fire on
//! music or noise, so only dense-speech spans are retried, the total
//! re-decoded audio is capped, and recovered segments must pass Whisper's own
//! confidence checks. Spans that are not recovered keep the coverage warning.

use tracing::{info, warn};

use super::diagnostics::{analyze_coverage, UncoveredSpan};
use super::whisper_backend::contains_no_speech_marker;
use super::TranscriptSegment;
use crate::error::DictationError;

const SAMPLE_RATE_HZ: f64 = 16_000.0;
/// Re-decode at most this fraction of the file...
pub const MAX_REDECODE_FRACTION: f64 = 0.20;
/// ...and never more than this many seconds in total.
pub const MAX_REDECODE_SECONDS: f64 = 600.0;
/// Short files may always spend this much (cost is trivial at this size).
pub const MIN_REDECODE_BUDGET_SECONDS: f64 = 60.0;
/// Spans with a lower share of speech frames are likely noise or music.
pub const MIN_SPAN_SPEECH_RATIO: f64 = 0.30;
/// Recovered segments below this mean token log-probability are dropped.
pub const MIN_RECOVERED_AVG_LOGPROB: f32 = -1.0;
/// Longest boundary overlap (in words) that is de-duplicated.
const MAX_BOUNDARY_WORDS: usize = 8;
/// Shortest boundary overlap (in words) treated as a duplicate.
const MIN_BOUNDARY_WORDS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RedecodeLimits {
    /// Total audio (seconds) that may be re-decoded.
    pub max_total_seconds: f64,
    /// Minimum speech-frame share of a span for it to be retried.
    pub min_speech_ratio: f64,
    /// Whisper's configured no-speech threshold; `0.0` disables the check.
    pub no_speech_threshold: f32,
    /// Minimum acceptable mean token log-probability of a recovered segment.
    pub min_avg_logprob: f32,
}

impl RedecodeLimits {
    pub fn for_audio(audio_seconds: f64, no_speech_threshold: f32) -> Self {
        let cap = (audio_seconds * MAX_REDECODE_FRACTION)
            .min(MAX_REDECODE_SECONDS)
            .max(MIN_REDECODE_BUDGET_SECONDS.min(audio_seconds));
        Self {
            max_total_seconds: cap,
            min_speech_ratio: MIN_SPAN_SPEECH_RATIO,
            no_speech_threshold,
            min_avg_logprob: MIN_RECOVERED_AVG_LOGPROB,
        }
    }
}

fn segment_is_trustworthy(segment: &TranscriptSegment, limits: &RedecodeLimits) -> bool {
    let no_speech = limits.no_speech_threshold > 0.0
        && segment.no_speech_prob > limits.no_speech_threshold;
    let low_confidence = segment
        .avg_logprob
        .is_some_and(|logprob| logprob < limits.min_avg_logprob);
    !(segment.text.trim().is_empty()
        || contains_no_speech_marker(&segment.text)
        || no_speech
        || low_confidence)
}

/// Normalised word for boundary comparison (case and edge punctuation ignored).
fn norm_word(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn norm_words(text: &str) -> Vec<String> {
    text.split_whitespace().map(norm_word).collect()
}

/// Longest k in MIN..=MAX such that the last k words of `before` equal the
/// first k words of `after`.
fn boundary_overlap(before: &[String], after: &[String]) -> usize {
    let max = MAX_BOUNDARY_WORDS.min(before.len()).min(after.len());
    (MIN_BOUNDARY_WORDS..=max)
        .rev()
        .find(|&k| before[before.len() - k..] == after[..k])
        .unwrap_or(0)
}

fn strip_leading_words(text: &str, count: usize) -> String {
    let mut rest = text.trim_start();
    for _ in 0..count {
        rest = match rest.find(char::is_whitespace) {
            Some(index) => rest[index..].trim_start(),
            None => "",
        };
    }
    if rest.is_empty() {
        String::new()
    } else {
        format!(" {rest}")
    }
}

fn strip_trailing_words(text: &str, count: usize) -> String {
    let mut rest = text.trim_end();
    for _ in 0..count {
        rest = match rest.rfind(char::is_whitespace) {
            Some(index) => rest[..index].trim_end(),
            None => "",
        };
    }
    if rest.trim().is_empty() {
        String::new()
    } else {
        rest.to_string()
    }
}

/// Drop words at the span edges that merely repeat the neighbouring
/// transcript (window overlap makes Whisper restate them).
fn dedupe_boundaries(
    recovered: &mut Vec<TranscriptSegment>,
    previous: Option<&TranscriptSegment>,
    next: Option<&TranscriptSegment>,
) {
    if let (Some(previous), Some(first)) = (previous, recovered.first_mut()) {
        let overlap = boundary_overlap(&norm_words(&previous.text), &norm_words(&first.text));
        if overlap > 0 {
            first.text = strip_leading_words(&first.text, overlap);
        }
    }
    recovered.retain(|segment| !segment.text.trim().is_empty());
    if let (Some(next), Some(last)) = (next, recovered.last_mut()) {
        let overlap = boundary_overlap(&norm_words(&last.text), &norm_words(&next.text));
        if overlap > 0 {
            last.text = strip_trailing_words(&last.text, overlap);
        }
    }
    recovered.retain(|segment| !segment.text.trim().is_empty());
}

/// Choose spans to retry: dense speech only, densest first, within the budget.
fn select_spans<'a>(spans: &'a [UncoveredSpan], limits: &RedecodeLimits) -> Vec<&'a UncoveredSpan> {
    let mut candidates: Vec<&UncoveredSpan> = spans
        .iter()
        .filter(|span| span.speech_ratio >= limits.min_speech_ratio)
        .collect();
    candidates.sort_by(|a, b| {
        b.speech_ratio
            .total_cmp(&a.speech_ratio)
            .then(b.speech_seconds.total_cmp(&a.speech_seconds))
    });
    let mut budget = limits.max_total_seconds;
    let mut selected = Vec::new();
    for span in candidates {
        if span.duration <= budget {
            budget -= span.duration;
            selected.push(span);
        }
    }
    selected
}

/// Re-decode material uncovered spans that contain dense speech, once each.
///
/// `decode` receives the audio slice of a span and returns segments with
/// timestamps relative to the slice start. A failing or empty re-decode leaves
/// the gap in place so the caller's coverage warning still fires.
pub fn redecode_uncovered_speech<F>(
    audio: &[f32],
    segments: Vec<TranscriptSegment>,
    limits: &RedecodeLimits,
    mut decode: F,
) -> Vec<TranscriptSegment>
where
    F: FnMut(&[f32]) -> Result<Vec<TranscriptSegment>, DictationError>,
{
    let coverage = analyze_coverage(audio, &segments);
    if coverage.uncovered_spans.is_empty() {
        return segments;
    }

    let selected = select_spans(&coverage.uncovered_spans, limits);
    if selected.len() < coverage.uncovered_spans.len() {
        info!(
            "Re-decoding {} of {} uncovered spans (budget {:.0}s, min speech ratio {:.2})",
            selected.len(),
            coverage.uncovered_spans.len(),
            limits.max_total_seconds,
            limits.min_speech_ratio
        );
    }

    let mut additions = Vec::new();
    for span in selected {
        let from = ((span.start * SAMPLE_RATE_HZ) as usize).min(audio.len());
        let to = ((span.end * SAMPLE_RATE_HZ).ceil() as usize).min(audio.len());
        if to <= from {
            continue;
        }
        info!(
            "Re-decoding a {:.1}s span ({:.2}s-{:.2}s) that Whisper skipped",
            span.duration, span.start, span.end
        );
        let recovered = match decode(&audio[from..to]) {
            Ok(recovered) => recovered,
            Err(error) => {
                warn!(
                    "Re-decoding uncovered span {:.2}s-{:.2}s failed: {error}",
                    span.start, span.end
                );
                continue;
            }
        };
        let mut recovered: Vec<TranscriptSegment> = recovered
            .into_iter()
            .filter(|segment| segment_is_trustworthy(segment, limits))
            .map(|mut segment| {
                segment.start = (segment.start + span.start).clamp(span.start, span.end);
                segment.end = (segment.end + span.start).clamp(segment.start, span.end);
                segment
            })
            .collect();
        recovered.sort_by(|a, b| a.start.total_cmp(&b.start));
        let previous = segments
            .iter()
            .filter(|s| s.end <= span.start + 0.01)
            .max_by(|a, b| a.end.total_cmp(&b.end));
        let next = segments
            .iter()
            .filter(|s| s.start >= span.end - 0.01)
            .min_by(|a, b| a.start.total_cmp(&b.start));
        dedupe_boundaries(&mut recovered, previous, next);
        additions.extend(recovered);
    }

    let mut merged = segments;
    merged.extend(additions);
    merged.sort_by(|a, b| a.start.total_cmp(&b.start));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.to_string(),
            avg_logprob: Some(-0.2),
            no_speech_prob: 0.0,
        }
    }

    /// 16 kHz audio: loud tone where `speech` ranges say, silence elsewhere.
    fn audio(total_seconds: usize, speech: &[(usize, usize)]) -> Vec<f32> {
        let mut samples = vec![0.0f32; total_seconds * 16_000];
        for &(from, to) in speech {
            for (i, sample) in samples[from * 16_000..to * 16_000].iter_mut().enumerate() {
                *sample = 0.3 * (i as f32 * 0.05).sin();
            }
        }
        samples
    }

    fn texts(segments: &[TranscriptSegment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    fn limits(audio_len: usize) -> RedecodeLimits {
        RedecodeLimits::for_audio(audio_len as f64 / 16_000.0, 0.3)
    }

    #[test]
    fn redecodes_uncovered_speech_and_merges_in_order() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let mut calls = Vec::new();
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |slice| {
            calls.push(slice.len());
            Ok(vec![seg(0.0, 8.0, "middle one"), seg(8.0, 40.0, "middle two")])
        });
        // Gap is 10s..50s (40s, 50% speech); decoded exactly once over that span.
        assert_eq!(calls, vec![40 * 16_000]);
        assert_eq!(texts(&merged), ["first", "middle one", "middle two", "last"]);
        assert_eq!(merged[1].start, 10.0);
        assert_eq!(merged[2].end, 50.0);
        assert!(analyze_coverage(&samples, &merged).warnings.is_empty());
    }

    #[test]
    fn silence_only_gaps_are_not_redecoded() {
        let samples = audio(60, &[(0, 10), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |_| {
            panic!("decoder must not run for a silent gap")
        });
        assert_eq!(texts(&merged), ["first", "last"]);
    }

    #[test]
    fn empty_or_failed_redecode_keeps_the_warning() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let l = limits(samples.len());

        let empty = redecode_uncovered_speech(&samples, initial.clone(), &l, |_| Ok(vec![]));
        assert_eq!(texts(&empty), ["first", "last"]);
        assert_eq!(analyze_coverage(&samples, &empty).warnings[0].code, "uncovered_speech");

        let failed = redecode_uncovered_speech(&samples, initial, &l, |_| {
            Err(DictationError::TranscriptionFailed("boom".into()))
        });
        assert_eq!(texts(&failed), ["first", "last"]);
        assert_eq!(analyze_coverage(&samples, &failed).warnings.len(), 1);
    }

    #[test]
    fn span_at_end_of_file_clamps_to_audio_length() {
        let samples = audio(60, &[(0, 10), (20, 60)]);
        let initial = vec![seg(0.0, 10.0, "first")];
        let mut slice_len = 0;
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |slice| {
            slice_len = slice.len();
            // Timestamps run past the slice end; they must clamp to the file.
            Ok(vec![seg(0.0, 70.0, "tail")])
        });
        assert_eq!(slice_len, 50 * 16_000);
        assert_eq!(texts(&merged), ["first", "tail"]);
        assert_eq!(merged[1].start, 10.0);
        assert_eq!(merged[1].end, 60.0);
    }

    #[test]
    fn span_longer_than_one_whisper_window_is_decoded_whole() {
        let samples = audio(1000, &[(0, 10), (20, 190), (195, 1000)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(195.0, 1000.0, "last")];
        let mut calls = Vec::new();
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |slice| {
            calls.push(slice.len());
            Ok(vec![seg(0.0, 60.0, "long one"), seg(60.0, 185.0, "long two")])
        });
        // 10s..195s is 185 s; one call, Whisper windows it internally.
        assert_eq!(calls, vec![185 * 16_000]);
        assert_eq!(texts(&merged), ["first", "long one", "long two", "last"]);
    }

    #[test]
    fn boundary_duplicates_are_trimmed_against_neighbours() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![
            seg(0.0, 10.0, " we went to the market"),
            seg(50.0, 60.0, " and then we left"),
        ];
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |_| {
            Ok(vec![
                seg(0.0, 20.0, " To the market, and bought bread"),
                seg(20.0, 40.0, " it was late and then we left"),
            ])
        });
        assert_eq!(
            texts(&merged),
            [
                " we went to the market",
                " and bought bread",
                " it was late",
                " and then we left"
            ]
        );
    }

    #[test]
    fn single_word_overlap_is_not_treated_as_duplicate() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, " one two"), seg(50.0, 60.0, " end")];
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |_| {
            Ok(vec![seg(0.0, 40.0, " two three end")])
        });
        assert_eq!(texts(&merged), [" one two", " two three end", " end"]);
    }

    #[test]
    fn no_speech_marked_and_low_confidence_segments_are_dropped() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |_| {
            let mut high_no_speech = seg(10.0, 15.0, "hallucinated");
            high_no_speech.no_speech_prob = 0.8;
            let mut low_logprob = seg(15.0, 20.0, "garbled");
            low_logprob.avg_logprob = Some(-1.6);
            Ok(vec![
                seg(0.0, 5.0, "<|nospeech|> Thanks for watching"),
                high_no_speech,
                low_logprob,
                seg(20.0, 40.0, "real speech"),
            ])
        });
        assert_eq!(texts(&merged), ["first", "real speech", "last"]);
    }

    #[test]
    fn low_speech_density_spans_are_skipped() {
        // 100 s gap with only 20 s (20%) of loud audio: likely noise/music.
        let samples = audio(120, &[(0, 10), (30, 50), (110, 120)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(110.0, 120.0, "last")];
        let merged = redecode_uncovered_speech(&samples, initial, &limits(samples.len()), |_| {
            panic!("decoder must not run for a sparse span")
        });
        assert_eq!(texts(&merged), ["first", "last"]);
    }

    #[test]
    fn total_redecoded_audio_is_capped_and_densest_span_goes_first() {
        // 1000 s file -> cap is 200 s. Two gaps: A = 100 s at ~50% density,
        // B = 100 s at ~100% density; C = 100 s at ~50% would exceed the cap.
        let speech = [
            (0, 10),    // covered
            (60, 110),  // gap A: 10..110 (50 s of 100)
            (110, 120), // covered
            (120, 220), // gap B: 120..220 fully speech
            (220, 230), // covered
            (280, 330), // gap C: 230..330 (50 s of 100)
            (330, 1000), // covered
        ];
        let samples = audio(1000, &speech);
        let initial = vec![
            seg(0.0, 10.0, "c1"),
            seg(110.0, 120.0, "c2"),
            seg(220.0, 230.0, "c3"),
            seg(330.0, 1000.0, "c4"),
        ];
        let l = limits(samples.len());
        assert_eq!(l.max_total_seconds, 200.0);
        let mut decoded = Vec::new();
        let merged = redecode_uncovered_speech(&samples, initial, &l, |slice| {
            decoded.push(slice.len() as f64 / 16_000.0);
            Ok(vec![seg(0.0, 1.0, "recovered")])
        });
        // B (densest) first, then one of the 50% spans; the third is over budget.
        assert_eq!(decoded, vec![100.0, 100.0]);
        assert_eq!(merged.iter().filter(|s| s.text == "recovered").count(), 2);
        assert!(merged.iter().any(|s| s.text == "recovered" && s.start == 120.0));
    }

    #[test]
    fn budget_scales_with_duration_and_is_capped() {
        assert_eq!(RedecodeLimits::for_audio(1000.0, 0.3).max_total_seconds, 200.0);
        assert_eq!(RedecodeLimits::for_audio(36_000.0, 0.3).max_total_seconds, 600.0);
        assert_eq!(RedecodeLimits::for_audio(120.0, 0.3).max_total_seconds, 60.0);
        assert_eq!(RedecodeLimits::for_audio(30.0, 0.3).max_total_seconds, 30.0);
    }
}
