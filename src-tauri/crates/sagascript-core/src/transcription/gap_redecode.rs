//! Recovery for speech that Whisper's long-audio windowing skipped (#273).
//!
//! whisper.cpp advances a full 30 s window when a decode ends in a lone
//! timestamp token ("single timestamp ending - skip entire chunk"). If the
//! decoder stopped early inside a window, everything after that point in the
//! window is never transcribed. The coverage check detects such gaps; this
//! module re-decodes just those spans once and merges the result in time order.

use tracing::warn;

use super::diagnostics::analyze_coverage;
use super::whisper_backend::contains_no_speech_marker;
use super::TranscriptSegment;
use crate::error::DictationError;

const SAMPLE_RATE_HZ: f64 = 16_000.0;

/// Re-decode every material uncovered span that contains speech, once.
///
/// `decode` receives the audio slice of a span and returns segments with
/// timestamps relative to the slice start. A failing or empty re-decode leaves
/// the gap in place so the caller's coverage warning still fires.
pub fn redecode_uncovered_speech<F>(
    audio: &[f32],
    segments: Vec<TranscriptSegment>,
    mut decode: F,
) -> Vec<TranscriptSegment>
where
    F: FnMut(&[f32]) -> Result<Vec<TranscriptSegment>, DictationError>,
{
    let coverage = analyze_coverage(audio, &segments);
    if coverage.uncovered_spans.is_empty() {
        return segments;
    }

    let mut merged = segments;
    for span in &coverage.uncovered_spans {
        let from = ((span.start * SAMPLE_RATE_HZ) as usize).min(audio.len());
        let to = ((span.end * SAMPLE_RATE_HZ).ceil() as usize).min(audio.len());
        if to <= from {
            continue;
        }
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
        merged.extend(recovered.into_iter().filter_map(|mut segment| {
            if segment.text.trim().is_empty() || contains_no_speech_marker(&segment.text) {
                return None;
            }
            segment.start = (segment.start + span.start).clamp(span.start, span.end);
            segment.end = (segment.end + span.start).clamp(segment.start, span.end);
            Some(segment)
        }));
    }
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

    #[test]
    fn redecodes_uncovered_speech_and_merges_in_order() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];
        let mut calls = Vec::new();
        let merged = redecode_uncovered_speech(&samples, initial, |slice| {
            calls.push(slice.len());
            Ok(vec![seg(0.0, 8.0, "middle one"), seg(8.0, 40.0, "middle two")])
        });
        // Gap is 10s..50s (40s); decoded exactly once over that span.
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
        let merged = redecode_uncovered_speech(&samples, initial.clone(), |_| {
            panic!("decoder must not run for a silent gap")
        });
        assert_eq!(texts(&merged), ["first", "last"]);
    }

    #[test]
    fn empty_or_failed_redecode_keeps_the_warning() {
        let samples = audio(60, &[(0, 10), (20, 40), (50, 60)]);
        let initial = vec![seg(0.0, 10.0, "first"), seg(50.0, 60.0, "last")];

        let empty = redecode_uncovered_speech(&samples, initial.clone(), |_| Ok(vec![]));
        assert_eq!(texts(&empty), ["first", "last"]);
        assert_eq!(analyze_coverage(&samples, &empty).warnings[0].code, "uncovered_speech");

        let failed = redecode_uncovered_speech(&samples, initial.clone(), |_| {
            Err(DictationError::TranscriptionFailed("boom".into()))
        });
        assert_eq!(texts(&failed), ["first", "last"]);
        assert_eq!(analyze_coverage(&samples, &failed).warnings.len(), 1);
    }
}
