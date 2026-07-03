/// Merge diarization speaker segments with Whisper transcript segments.
///
/// Assigns a speaker label to each transcript segment based on time overlap
/// with the diarization output.
use crate::diarization::{DiarizedSegment, SpeakerSegment, TimestampedSegment};

/// Assign speakers to transcript segments by maximum time overlap.
///
/// For each transcript segment, finds the speaker with the largest overlap
/// among all `SpeakerSegment`s. If no overlap exists, assigns the nearest
/// speaker by time gap.
pub fn merge_with_transcript(
    speakers: &[SpeakerSegment],
    transcript: &[TimestampedSegment],
) -> Vec<DiarizedSegment> {
    transcript
        .iter()
        .map(|seg| {
            let speaker = assign_speaker(speakers, seg.start, seg.end);
            DiarizedSegment {
                start: seg.start,
                end: seg.end,
                speaker,
                text: seg.text.clone(),
            }
        })
        .collect()
}

/// Find the speaker label with maximum overlap for a time window `[start, end]`.
/// Falls back to nearest speaker if no overlap exists.
fn assign_speaker(speakers: &[SpeakerSegment], start: f64, end: f64) -> String {
    if speakers.is_empty() {
        return "SPEAKER_0".to_string();
    }

    // Accumulate overlap per speaker label
    let mut best_speaker: Option<&str> = None;
    let mut best_overlap = 0.0f64;

    for seg in speakers {
        let overlap_start = start.max(seg.start);
        let overlap_end = end.min(seg.end);
        let overlap = (overlap_end - overlap_start).max(0.0);

        if overlap > best_overlap {
            best_overlap = overlap;
            best_speaker = Some(&seg.speaker);
        }
    }

    if let Some(spk) = best_speaker {
        return spk.to_string();
    }

    // No overlap — find nearest speaker by minimum gap
    let nearest = speakers
        .iter()
        .min_by(|a, b| {
            let gap_a = gap_to_segment(a, start, end);
            let gap_b = gap_to_segment(b, start, end);
            gap_a
                .partial_cmp(&gap_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

    nearest
        .map(|s| s.speaker.clone())
        .unwrap_or_else(|| "SPEAKER_0".to_string())
}

/// Compute the time gap between a speaker segment and the query interval `[start, end]`.
fn gap_to_segment(seg: &SpeakerSegment, start: f64, end: f64) -> f64 {
    if seg.end < start {
        start - seg.end
    } else if seg.start > end {
        seg.start - end
    } else {
        0.0 // overlapping
    }
}

/// Merge consecutive `DiarizedSegment`s from the same speaker into one.
///
/// Adjacent segments with the same speaker label are merged; their texts
/// are concatenated with a space separator.
pub fn consolidate(segments: &[DiarizedSegment]) -> Vec<DiarizedSegment> {
    let mut result: Vec<DiarizedSegment> = Vec::new();

    for seg in segments {
        if let Some(last) = result.last_mut() {
            if last.speaker == seg.speaker {
                last.end = seg.end;
                if !seg.text.is_empty() {
                    if !last.text.is_empty() {
                        last.text.push(' ');
                    }
                    last.text.push_str(&seg.text);
                }
                continue;
            }
        }
        result.push(seg.clone());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spk(start: f64, end: f64, label: &str) -> SpeakerSegment {
        SpeakerSegment {
            start,
            end,
            speaker: label.to_string(),
        }
    }

    fn seg(start: f64, end: f64, text: &str) -> TimestampedSegment {
        TimestampedSegment {
            start,
            end,
            text: text.to_string(),
        }
    }

    fn dseg(start: f64, end: f64, speaker: &str, text: &str) -> DiarizedSegment {
        DiarizedSegment {
            start,
            end,
            speaker: speaker.to_string(),
            text: text.to_string(),
        }
    }

    // -- merge_with_transcript --

    #[test]
    fn perfect_alignment() {
        let speakers = vec![
            spk(0.0, 2.0, "SPEAKER_0"),
            spk(2.0, 4.0, "SPEAKER_1"),
        ];
        let transcript = vec![
            seg(0.0, 2.0, "Hello"),
            seg(2.0, 4.0, "World"),
        ];
        let result = merge_with_transcript(&speakers, &transcript);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].speaker, "SPEAKER_0");
        assert_eq!(result[1].speaker, "SPEAKER_1");
        assert_eq!(result[0].text, "Hello");
        assert_eq!(result[1].text, "World");
    }

    #[test]
    fn majority_overlap_wins() {
        // Transcript segment [1.0, 3.0] overlaps SPEAKER_0 by 1s, SPEAKER_1 by 1s — tie, first wins
        // But [0.5, 2.5]: overlaps SPEAKER_0 by 1.5s and SPEAKER_1 by 0.5s → SPEAKER_0 wins
        let speakers = vec![
            spk(0.0, 2.0, "SPEAKER_0"),
            spk(2.0, 4.0, "SPEAKER_1"),
        ];
        let transcript = vec![seg(0.5, 2.5, "test")];
        let result = merge_with_transcript(&speakers, &transcript);
        assert_eq!(result[0].speaker, "SPEAKER_0", "SPEAKER_0 has more overlap");
    }

    #[test]
    fn no_overlap_uses_nearest() {
        // Transcript segment [5.0, 6.0], speakers end at 4.0
        let speakers = vec![
            spk(0.0, 2.0, "SPEAKER_0"),
            spk(2.0, 4.0, "SPEAKER_1"),
        ];
        let transcript = vec![seg(5.0, 6.0, "later")];
        let result = merge_with_transcript(&speakers, &transcript);
        // Nearest is SPEAKER_1 (gap = 1.0 vs SPEAKER_0 gap = 3.0)
        assert_eq!(result[0].speaker, "SPEAKER_1");
    }

    #[test]
    fn empty_transcript_returns_empty() {
        let speakers = vec![spk(0.0, 2.0, "SPEAKER_0")];
        let result = merge_with_transcript(&speakers, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn empty_speakers_defaults_to_speaker_zero() {
        let transcript = vec![seg(0.0, 1.0, "test")];
        let result = merge_with_transcript(&[], &transcript);
        assert_eq!(result[0].speaker, "SPEAKER_0");
    }

    #[test]
    fn timestamps_preserved() {
        let speakers = vec![spk(0.0, 5.0, "SPEAKER_0")];
        let transcript = vec![seg(1.5, 3.5, "text")];
        let result = merge_with_transcript(&speakers, &transcript);
        assert!((result[0].start - 1.5).abs() < 1e-9);
        assert!((result[0].end - 3.5).abs() < 1e-9);
    }

    // -- consolidate --

    #[test]
    fn consolidate_merges_consecutive_same_speaker() {
        let segments = vec![
            dseg(0.0, 1.0, "SPEAKER_0", "Hello"),
            dseg(1.0, 2.0, "SPEAKER_0", "world"),
            dseg(2.0, 3.0, "SPEAKER_0", "foo"),
        ];
        let result = consolidate(&segments);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].speaker, "SPEAKER_0");
        assert_eq!(result[0].text, "Hello world foo");
        assert!((result[0].start - 0.0).abs() < 1e-9);
        assert!((result[0].end - 3.0).abs() < 1e-9);
    }

    #[test]
    fn consolidate_keeps_different_speakers_separate() {
        let segments = vec![
            dseg(0.0, 1.0, "SPEAKER_0", "A"),
            dseg(1.0, 2.0, "SPEAKER_1", "B"),
            dseg(2.0, 3.0, "SPEAKER_0", "C"),
        ];
        let result = consolidate(&segments);
        assert_eq!(result.len(), 3, "alternating speakers should not merge");
    }

    #[test]
    fn consolidate_empty_returns_empty() {
        assert!(consolidate(&[]).is_empty());
    }

    #[test]
    fn consolidate_single_segment_unchanged() {
        let segments = vec![dseg(0.0, 1.0, "SPEAKER_0", "text")];
        let result = consolidate(&segments);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "text");
    }

    #[test]
    fn consolidate_handles_empty_text() {
        let segments = vec![
            dseg(0.0, 1.0, "SPEAKER_0", "Hello"),
            dseg(1.0, 2.0, "SPEAKER_0", ""),
            dseg(2.0, 3.0, "SPEAKER_0", "world"),
        ];
        let result = consolidate(&segments);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "Hello world");
    }

    // -- gap_to_segment --

    #[test]
    fn gap_before_segment() {
        let seg = spk(5.0, 8.0, "X");
        assert!((gap_to_segment(&seg, 1.0, 3.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn gap_after_segment() {
        let seg = spk(1.0, 3.0, "X");
        assert!((gap_to_segment(&seg, 5.0, 8.0) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn gap_overlapping_is_zero() {
        let seg = spk(0.0, 5.0, "X");
        assert!((gap_to_segment(&seg, 2.0, 4.0)).abs() < 1e-9);
    }
}
