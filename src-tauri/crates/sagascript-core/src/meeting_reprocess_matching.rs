use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use crate::meeting::{MeetingSegment, MeetingTranscript};

#[derive(Debug, Eq, Hash, PartialEq)]
struct SegmentSignature {
    start: u64,
    end: u64,
    text: String,
}

fn finite_bits(value: f64) -> Option<u64> {
    if !value.is_finite() {
        None
    } else if value == 0.0 {
        Some(0)
    } else {
        Some(value.to_bits())
    }
}

fn signature(segment: &MeetingSegment) -> Option<SegmentSignature> {
    Some(SegmentSignature {
        start: finite_bits(segment.start)?,
        end: finite_bits(segment.end)?,
        text: segment.text.clone(),
    })
}

pub(crate) fn match_segments(
    before: &[MeetingSegment],
    after: &[MeetingSegment],
) -> Vec<Option<usize>> {
    let mut source_counts = HashMap::with_capacity(before.len());
    for segment in before {
        if let Some(key) = signature(segment) {
            *source_counts.entry(key).or_insert(0usize) += 1;
        }
    }

    let mut target_indices = HashMap::with_capacity(after.len());
    for (index, segment) in after.iter().enumerate() {
        if let Some(key) = signature(segment) {
            match target_indices.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(Some(index));
                }
                Entry::Occupied(mut entry) => {
                    entry.insert(None);
                }
            }
        }
    }

    before
        .iter()
        .map(|segment| {
            let key = signature(segment)?;
            if source_counts.get(&key) != Some(&1) {
                return None;
            }
            target_indices.get(&key).copied().flatten()
        })
        .collect()
}

fn unique_speaker_indices(
    speakers: &[crate::meeting::MeetingSpeaker],
) -> HashMap<&str, Option<usize>> {
    let mut indices = HashMap::with_capacity(speakers.len());
    for (index, speaker) in speakers.iter().enumerate() {
        match indices.entry(speaker.id.as_str()) {
            Entry::Vacant(entry) => {
                entry.insert(Some(index));
            }
            Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    indices
}

fn speaker_memberships(
    segments: &[MeetingSegment],
    speaker_indices: &HashMap<&str, Option<usize>>,
    speaker_count: usize,
) -> Vec<HashSet<usize>> {
    let mut memberships = (0..speaker_count)
        .map(|_| HashSet::new())
        .collect::<Vec<HashSet<usize>>>();
    for (segment_index, segment) in segments.iter().enumerate() {
        if let Some(Some(speaker_index)) = speaker_indices.get(segment.speaker.as_str()) {
            memberships[*speaker_index].insert(segment_index);
        }
    }
    memberships
}

pub(crate) fn match_speakers(
    before: &MeetingTranscript,
    after: &MeetingTranscript,
) -> Vec<Option<usize>> {
    let segment_matches = match_segments(&before.segments, &after.segments);
    let before_speaker_indices = unique_speaker_indices(&before.speakers);
    let after_speaker_indices = unique_speaker_indices(&after.speakers);
    let before_memberships = speaker_memberships(
        &before.segments,
        &before_speaker_indices,
        before.speakers.len(),
    );
    let after_memberships = speaker_memberships(
        &after.segments,
        &after_speaker_indices,
        after.speakers.len(),
    );

    before_memberships
        .iter()
        .map(|source_membership| {
            if source_membership.is_empty() {
                return None;
            }

            let mut target_speaker = None;
            let mut matched_targets = HashSet::with_capacity(source_membership.len());
            for &source_segment_index in source_membership {
                let target_segment_index = segment_matches[source_segment_index]?;
                let target_segment = after.segments.get(target_segment_index)?;
                let target_speaker_index = after_speaker_indices
                    .get(target_segment.speaker.as_str())
                    .copied()
                    .flatten()?;

                if let Some(expected_speaker_index) = target_speaker {
                    if expected_speaker_index != target_speaker_index {
                        return None;
                    }
                } else {
                    target_speaker = Some(target_speaker_index);
                }
                matched_targets.insert(target_segment_index);
            }

            let target_speaker = target_speaker?;
            (after_memberships[target_speaker] == matched_targets).then_some(target_speaker)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{match_segments, match_speakers};
    use crate::meeting::{MeetingSegment, MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct Fixture {
        name: String,
        before: Vec<(f64, f64, String)>,
        after: Vec<(f64, f64, String)>,
        expected: Vec<Option<usize>>,
    }

    fn segment(id: &str, start: f64, end: f64, text: &str, speaker: &str) -> MeetingSegment {
        MeetingSegment {
            id: id.to_owned(),
            start,
            end,
            text: text.to_owned(),
            speaker: speaker.to_owned(),
        }
    }

    fn fixture_segments(items: &[(f64, f64, String)], prefix: &str) -> Vec<MeetingSegment> {
        items
            .iter()
            .enumerate()
            .map(|(index, (start, end, text))| {
                segment(
                    &format!("{prefix}-{index}"),
                    *start,
                    *end,
                    text,
                    &format!("speaker-{index}"),
                )
            })
            .collect()
    }

    fn transcript(segments: &[(&str, f64, f64, &str)], speaker_ids: &[&str]) -> MeetingTranscript {
        MeetingTranscript::new(
            "0000000000000000000000000000000000000000000000000000000000000000",
            "en",
            "model",
            10.0,
            segments
                .iter()
                .map(|(speaker, start, end, text)| MeetingSegmentInput {
                    start: *start,
                    end: *end,
                    text: (*text).to_owned(),
                    speaker: (*speaker).to_owned(),
                })
                .collect(),
            speaker_ids
                .iter()
                .map(|id| MeetingSpeaker {
                    id: (*id).to_owned(),
                    label: format!("label-{id}"),
                })
                .collect(),
        )
        .expect("test transcript should validate")
    }

    #[test]
    fn matches_root_reviewed_fixtures() {
        let fixtures: Vec<Fixture> = serde_json::from_str(include_str!(
            "../../../../scripts/fixtures/meeting-reprocess-matching.json"
        ))
        .expect("root-reviewed matching fixtures should parse");

        for fixture in fixtures {
            let before = fixture_segments(&fixture.before, "before");
            let after = fixture_segments(&fixture.after, "after");
            assert_eq!(
                match_segments(&before, &after),
                fixture.expected,
                "{}",
                fixture.name
            );
        }
    }

    #[test]
    fn ignores_ids_and_speakers_but_requires_exact_signature() {
        let before = vec![segment("old-id", 1.0, 2.0, "same text", "old-speaker")];
        let after = vec![segment("new-id", 1.0, 2.0, "same text", "new-speaker")];

        assert_eq!(match_segments(&before, &after), vec![Some(0)]);
    }

    #[test]
    fn treats_negative_and_positive_zero_as_equal() {
        let before = vec![segment("before", -0.0, -0.0, "zero", "speaker")];
        let after = vec![segment("after", 0.0, 0.0, "zero", "other")];

        assert_eq!(match_segments(&before, &after), vec![Some(0)]);
    }

    #[test]
    fn rejects_nonfinite_signatures() {
        let before = vec![
            segment("nan", f64::NAN, 1.0, "nan", "speaker"),
            segment("infinity", 2.0, f64::INFINITY, "infinity", "speaker"),
            segment("finite", 3.0, 4.0, "finite", "speaker"),
        ];
        let after = vec![
            segment("nan-target", f64::NAN, 1.0, "nan", "other"),
            segment("infinity-target", 2.0, f64::INFINITY, "infinity", "other"),
            segment("finite-target", 3.0, 4.0, "finite", "other"),
        ];

        assert_eq!(match_segments(&before, &after), vec![None, None, Some(2)]);
    }

    #[test]
    fn matches_speakers_after_id_relabeling() {
        let before = transcript(
            &[
                ("before-a", 0.0, 1.0, "hello"),
                ("before-b", 2.0, 3.0, "goodbye"),
            ],
            &["before-a", "before-b"],
        );
        let after = transcript(
            &[
                ("after-y", 0.0, 1.0, "hello"),
                ("after-x", 2.0, 3.0, "goodbye"),
            ],
            &["after-x", "after-y"],
        );

        assert_eq!(match_speakers(&before, &after), vec![Some(1), Some(0)]);
    }

    #[test]
    fn rejects_split_source_segment() {
        let before = transcript(&[("before", 0.0, 10.0, "whole")], &["before"]);
        let after = transcript(
            &[
                ("after", 0.0, 5.0, "half one"),
                ("after", 5.0, 10.0, "half two"),
            ],
            &["after"],
        );

        assert_eq!(match_speakers(&before, &after), vec![None]);
    }

    #[test]
    fn rejects_merged_source_speakers() {
        let before = transcript(
            &[("before-a", 0.0, 1.0, "one"), ("before-b", 2.0, 3.0, "two")],
            &["before-a", "before-b"],
        );
        let after = transcript(
            &[("after", 0.0, 1.0, "one"), ("after", 2.0, 3.0, "two")],
            &["after"],
        );

        assert_eq!(match_speakers(&before, &after), vec![None, None]);
    }

    #[test]
    fn rejects_unmapped_source_segment() {
        let before = transcript(
            &[
                ("before", 0.0, 1.0, "keep"),
                ("before", 2.0, 3.0, "changed"),
            ],
            &["before"],
        );
        let after = transcript(&[("after", 0.0, 1.0, "keep")], &["after"]);

        assert_eq!(match_speakers(&before, &after), vec![None]);
    }

    #[test]
    fn rejects_extra_target_membership() {
        let before = transcript(&[("before", 0.0, 1.0, "keep")], &["before"]);
        let after = transcript(
            &[("after", 0.0, 1.0, "keep"), ("after", 2.0, 3.0, "new")],
            &["after"],
        );

        assert_eq!(match_speakers(&before, &after), vec![None]);
    }

    #[test]
    fn never_matches_empty_source_speaker() {
        let before = transcript(&[("active", 0.0, 1.0, "keep")], &["empty", "active"]);
        let after = transcript(&[("target", 0.0, 1.0, "keep")], &["target"]);

        assert_eq!(match_speakers(&before, &after), vec![None, Some(0)]);
    }
}
