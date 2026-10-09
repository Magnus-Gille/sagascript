/// Merge diarization speaker segments with Whisper transcript segments.
///
/// Assigns a speaker label to each transcript segment based on time overlap
/// with the diarization output.
use crate::diarization::{DiarizedSegment, SpeakerSegment, TimestampedSegment};
use crate::diarization_report::{AttributionEvidence, AttributionReason, SpeakerSupport};
use std::collections::BTreeMap;

/// Assign speakers to transcript segments by maximum time overlap.
///
/// For each transcript segment, finds the speaker with the largest overlap
/// among all `SpeakerSegment`s. If no overlap exists, assigns the nearest
/// speaker by time gap.
pub fn merge_with_transcript(
    speakers: &[SpeakerSegment],
    transcript: &[TimestampedSegment],
) -> Vec<DiarizedSegment> {
    merge_with_diagnostics(speakers, transcript).0
}

/// Assign text once, preserving diagnostic support for competing speakers.
/// Multiple regions for one identity count through their union, not their
/// largest single region or the sum of duplicate activity.
pub fn merge_with_diagnostics(
    speakers: &[SpeakerSegment],
    transcript: &[TimestampedSegment],
) -> (Vec<DiarizedSegment>, Vec<AttributionEvidence>) {
    let mut diagnostics = Vec::with_capacity(transcript.len());
    let merged = transcript
        .iter()
        .enumerate()
        .map(|(index, seg)| {
            let evidence = attribution(speakers, index, seg.start, seg.end);
            let speaker = evidence.speaker.clone();
            diagnostics.push(evidence);
            DiarizedSegment {
                start: seg.start,
                end: seg.end,
                speaker,
                text: seg.text.clone(),
            }
        })
        .collect();
    (merged, diagnostics)
}

/// Find the speaker label with maximum overlap for a time window `[start, end]`.
/// Falls back to nearest speaker if no overlap exists.
fn attribution(
    speakers: &[SpeakerSegment],
    index: usize,
    start: f64,
    end: f64,
) -> AttributionEvidence {
    let mut support_regions = BTreeMap::<&str, Vec<(f64, f64)>>::new();
    let mut first_seen = BTreeMap::new();
    for seg in speakers {
        let overlap_start = start.max(seg.start);
        let overlap_end = end.min(seg.end);
        if overlap_end > overlap_start {
            let ordinal = first_seen.len();
            first_seen.entry(seg.speaker.as_str()).or_insert(ordinal);
            support_regions
                .entry(&seg.speaker)
                .or_default()
                .push((overlap_start, overlap_end));
        }
    }
    let mut support = support_regions
        .into_iter()
        .map(|(speaker, mut regions)| {
            regions.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
            let mut total = 0.0;
            let mut last = regions[0];
            for region in regions.into_iter().skip(1) {
                if region.0 <= last.1 {
                    last.1 = last.1.max(region.1);
                } else {
                    total += last.1 - last.0;
                    last = region;
                }
            }
            total += last.1 - last.0;
            SpeakerSupport {
                speaker: speaker.into(),
                overlap_seconds: total,
            }
        })
        .collect::<Vec<_>>();
    support.sort_by(|a, b| {
        b.overlap_seconds
            .total_cmp(&a.overlap_seconds)
            .then_with(|| first_seen[a.speaker.as_str()].cmp(&first_seen[b.speaker.as_str()]))
    });
    let mut evidence = AttributionEvidence {
        index,
        start,
        end,
        speaker: "SPEAKER_0".into(),
        reason: AttributionReason::NoSpeakerEvidence,
        support,
        margin_seconds: None,
        gap_seconds: None,
    };
    if let Some(best) = evidence.support.first() {
        evidence.speaker = best.speaker.clone();
        let runner_up = evidence.support.get(1).map_or(0.0, |s| s.overlap_seconds);
        let margin = (best.overlap_seconds - runner_up).max(0.0);
        evidence.margin_seconds = Some(margin);
        evidence.reason = if evidence.support.len() > 1 && margin <= 1e-9 {
            AttributionReason::TiedOverlap
        } else {
            AttributionReason::TemporalOverlap
        };
    } else if let Some(nearest) = speakers
        .iter()
        .min_by(|a, b| gap_to_segment(a, start, end).total_cmp(&gap_to_segment(b, start, end)))
    {
        evidence.speaker = nearest.speaker.clone();
        evidence.reason = AttributionReason::NearestGap;
        evidence.gap_seconds = Some(gap_to_segment(nearest, start, end));
    }
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        evidence.reason = AttributionReason::InvalidTimestamp;
    }
    evidence
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
    fn attribution_uses_total_union_support_per_speaker() {
        let speakers = vec![spk(0.0, 0.7, "a"), spk(1.3, 2.0, "a"), spk(0.5, 1.5, "b")];
        let output = merge_with_transcript(&speakers, &[seg(0.0, 2.0, "word")]);
        assert_eq!(output[0].speaker, "a");
    }

    #[test]
    fn duplicate_same_speaker_activity_does_not_inflate_support() {
        let speakers = vec![spk(0.0, 0.8, "a"), spk(0.0, 0.8, "a"), spk(0.0, 1.2, "b")];
        let output = merge_with_transcript(&speakers, &[seg(0.0, 2.0, "word")]);
        assert_eq!(output[0].speaker, "b");
    }

    #[test]
    fn perfect_alignment() {
        let speakers = vec![spk(0.0, 2.0, "SPEAKER_0"), spk(2.0, 4.0, "SPEAKER_1")];
        let transcript = vec![seg(0.0, 2.0, "Hello"), seg(2.0, 4.0, "World")];
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
        let speakers = vec![spk(0.0, 2.0, "SPEAKER_0"), spk(2.0, 4.0, "SPEAKER_1")];
        let transcript = vec![seg(0.5, 2.5, "test")];
        let result = merge_with_transcript(&speakers, &transcript);
        assert_eq!(result[0].speaker, "SPEAKER_0", "SPEAKER_0 has more overlap");
    }

    #[test]
    fn word_granularity_preserves_turns_that_coarse_segment_collapses() {
        let speakers = vec![spk(0.0, 6.0, "SPEAKER_0"), spk(6.0, 10.0, "SPEAKER_1")];

        // The GUI's former segment-level path attributed this entire Whisper
        // segment to SPEAKER_0 because it has the larger total overlap.
        let coarse = merge_with_transcript(&speakers, &[seg(0.0, 10.0, "Hello there Yes")]);
        assert_eq!(coarse.len(), 1);
        assert_eq!(coarse[0].speaker, "SPEAKER_0");

        // Word-sized timestamps retain the turn boundary for consolidation.
        let words = merge_with_transcript(
            &speakers,
            &[
                seg(0.0, 2.5, "Hello"),
                seg(2.5, 5.5, "there"),
                seg(6.0, 9.0, "Yes"),
            ],
        );
        let consolidated = consolidate(&words);
        assert_eq!(consolidated.len(), 2);
        assert_eq!(consolidated[0].speaker, "SPEAKER_0");
        assert_eq!(consolidated[1].speaker, "SPEAKER_1");
    }

    #[test]
    fn no_overlap_uses_nearest() {
        // Transcript segment [5.0, 6.0], speakers end at 4.0
        let speakers = vec![spk(0.0, 2.0, "SPEAKER_0"), spk(2.0, 4.0, "SPEAKER_1")];
        let transcript = vec![seg(5.0, 6.0, "later")];
        let result = merge_with_transcript(&speakers, &transcript);
        // Nearest is SPEAKER_1 (gap = 1.0 vs SPEAKER_0 gap = 3.0)
        assert_eq!(result[0].speaker, "SPEAKER_1");
    }

    #[test]
    fn diagnostics_mark_invalid_timestamps_and_nearest_fallback_in_order() {
        let speakers = vec![spk(0.0, 2.0, "SPEAKER_0"), spk(2.0, 4.0, "SPEAKER_1")];
        let transcript = vec![
            seg(3.0, 2.0, "invalid"),
            seg(5.0, 6.0, "later"),
            seg(1.0, 3.0, "tie"),
        ];
        let (merged, diagnostics) = merge_with_diagnostics(&speakers, &transcript);

        assert_eq!(
            merged
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["invalid", "later", "tie"]
        );
        assert_eq!(
            diagnostics
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(diagnostics[0].reason, AttributionReason::InvalidTimestamp);
        assert_eq!(diagnostics[1].reason, AttributionReason::NearestGap);
        assert_eq!(diagnostics[1].speaker, "SPEAKER_1");
        assert_eq!(diagnostics[2].reason, AttributionReason::TiedOverlap);
        assert_eq!(diagnostics[2].speaker, "SPEAKER_0");
    }

    #[test]
    fn short_backchannel_turn_gets_its_own_assignment() {
        let speakers = vec![
            spk(0.0, 1.2, "SPEAKER_0"),
            spk(1.2, 1.35, "SPEAKER_1"),
            spk(1.35, 3.0, "SPEAKER_0"),
        ];
        let transcript = vec![
            seg(0.4, 0.8, "main"),
            seg(1.2, 1.35, "mm"),
            seg(1.5, 2.0, "continues"),
        ];
        let (merged, diagnostics) = merge_with_diagnostics(&speakers, &transcript);

        assert_eq!(merged.len(), transcript.len());
        assert_eq!(
            merged
                .iter()
                .map(|item| item.speaker.as_str())
                .collect::<Vec<_>>(),
            ["SPEAKER_0", "SPEAKER_1", "SPEAKER_0"]
        );
        assert!(diagnostics
            .iter()
            .all(|item| item.reason == AttributionReason::TemporalOverlap));
        assert_eq!(
            merged
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["main", "mm", "continues"]
        );
    }

    #[test]
    fn rapid_switches_assign_each_token_once_without_reordering() {
        let speakers = vec![
            spk(0.0, 0.5, "SPEAKER_0"),
            spk(0.5, 0.55, "SPEAKER_1"),
            spk(0.55, 1.0, "SPEAKER_0"),
            spk(1.0, 1.1, "SPEAKER_2"),
        ];
        let transcript = vec![
            seg(0.0, 0.5, "one"),
            seg(0.5, 0.55, "two"),
            seg(0.55, 1.0, "three"),
            seg(1.0, 1.1, "four"),
        ];
        let (merged, diagnostics) = merge_with_diagnostics(&speakers, &transcript);

        assert_eq!(merged.len(), transcript.len());
        assert_eq!(
            diagnostics
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(
            merged
                .iter()
                .map(|item| item.speaker.as_str())
                .collect::<Vec<_>>(),
            ["SPEAKER_0", "SPEAKER_1", "SPEAKER_0", "SPEAKER_2"]
        );
        assert_eq!(
            merged
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "three", "four"]
        );
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
