//! Pure migration previews for selective meeting reprocessing.
//!
//! A preview starts from a newly inferred transcript and replays the previous
//! review's correction operations in order.  It never mutates the previous
//! review and never publishes the candidate; callers must resolve every
//! conflict before accepting the result.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::meeting::{MeetingError, MeetingTranscript};
use crate::meeting_reprocess_matching::{match_segments, match_speakers};
use crate::meeting_review::{
    CorrectionFile, CorrectionOperation, MeetingReview, MeetingReviewError,
    MAX_CORRECTION_OPERATIONS, REVIEW_SCHEMA_VERSION,
};

/// The disposition of one previous correction during migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    Applied,
    Conflict,
    Blocked,
}

/// One previous correction and its proposed translation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MigrationStep {
    pub original: CorrectionOperation,
    pub mapped: Option<Vec<CorrectionOperation>>,
    pub status: MigrationStatus,
}

/// A non-published candidate review plus the ordered migration decisions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MigrationPreview {
    pub candidate: MeetingReview,
    pub steps: Vec<MigrationStep>,
}

/// Preview replaying `previous` corrections onto `proposed`.
///
/// Automatic mapping uses only unique original segment signatures and complete
/// speaker membership mappings.  Explicit overrides are target operations and
/// therefore remain available when source identity changed or automatic
/// mapping is impossible.
pub fn preview_migration(
    previous: &MeetingReview,
    proposed: &MeetingTranscript,
    overrides: &BTreeMap<usize, Vec<CorrectionOperation>>,
) -> Result<MigrationPreview, MeetingReviewError> {
    previous.validate()?;
    proposed.validate()?;

    let operations = previous
        .batches
        .iter()
        .flat_map(|batch| batch.operations.iter().cloned())
        .collect::<Vec<_>>();

    for (&index, replacement) in overrides {
        if index >= operations.len() {
            return Err(MeetingReviewError::InvalidField("overrides"));
        }
        if replacement.is_empty() {
            return Err(MeetingReviewError::InvalidField("overrides"));
        }
    }

    let mut candidate = MeetingReview::new(proposed.clone())?;
    let source_matches = previous.original.source_sha256 == proposed.source_sha256;
    let segment_matches =
        source_matches.then(|| match_segments(&previous.original.segments, &proposed.segments));
    let speaker_matches = source_matches.then(|| match_speakers(&previous.original, proposed));

    for replacement in overrides.values() {
        CorrectionFile {
            schema_version: REVIEW_SCHEMA_VERSION,
            source_sha256: candidate.original.source_sha256.clone(),
            original_revision: candidate.original_revision.clone(),
            expected_revision: candidate.revision.clone(),
            operations: replacement.clone(),
        }
        .validate()?;
    }

    let mut effective_operation_count = 0usize;
    for index in 0..operations.len() {
        let operation_count = overrides.get(&index).map_or(1, Vec::len);
        effective_operation_count = effective_operation_count
            .checked_add(operation_count)
            .ok_or(MeetingReviewError::OperationLimit)?;
    }
    if effective_operation_count > MAX_CORRECTION_OPERATIONS {
        return Err(MeetingReviewError::OperationLimit);
    }

    let mut blocked = false;
    let mut steps = Vec::with_capacity(operations.len());

    for (index, original) in operations.into_iter().enumerate() {
        let mapped = overrides.get(&index).cloned().or_else(|| {
            if !source_matches {
                return None;
            }
            map_operation(
                &original,
                &previous.original,
                proposed,
                segment_matches
                    .as_deref()
                    .expect("source matches have segment map"),
                speaker_matches
                    .as_deref()
                    .expect("source matches have speaker map"),
            )
        });

        let status = if blocked {
            MigrationStatus::Blocked
        } else {
            match mapped.as_ref() {
                None => {
                    blocked = true;
                    MigrationStatus::Conflict
                }
                Some(operations) => {
                    let file = CorrectionFile {
                        schema_version: REVIEW_SCHEMA_VERSION,
                        source_sha256: candidate.original.source_sha256.clone(),
                        original_revision: candidate.original_revision.clone(),
                        expected_revision: candidate.revision.clone(),
                        operations: operations.clone(),
                    };
                    match candidate.apply_corrections(&file) {
                        Ok(next) => {
                            candidate = next;
                            MigrationStatus::Applied
                        }
                        Err(error) if is_semantic_conflict(&error) => {
                            blocked = true;
                            MigrationStatus::Conflict
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        };

        steps.push(MigrationStep {
            original,
            mapped,
            status,
        });
    }

    Ok(MigrationPreview { candidate, steps })
}

fn map_operation(
    operation: &CorrectionOperation,
    source: &MeetingTranscript,
    target: &MeetingTranscript,
    segment_matches: &[Option<usize>],
    speaker_matches: &[Option<usize>],
) -> Option<Vec<CorrectionOperation>> {
    let mapped = match operation {
        CorrectionOperation::EditSegment {
            segment_id,
            text,
            speaker_id,
        } => CorrectionOperation::EditSegment {
            segment_id: map_segment_id(segment_id, source, target, segment_matches)?,
            text: text.clone(),
            speaker_id: match speaker_id.as_deref() {
                Some(id) => Some(map_speaker_id(id, source, target, speaker_matches)?),
                None => None,
            },
        },
        CorrectionOperation::RenameSpeaker { speaker_id, label } => {
            CorrectionOperation::RenameSpeaker {
                speaker_id: map_speaker_id(speaker_id, source, target, speaker_matches)?,
                label: label.clone(),
            }
        }
        CorrectionOperation::MergeSpeakers { from_id, into_id } => {
            CorrectionOperation::MergeSpeakers {
                from_id: map_speaker_id(from_id, source, target, speaker_matches)?,
                into_id: map_speaker_id(into_id, source, target, speaker_matches)?,
            }
        }
    };
    Some(vec![mapped])
}

fn map_segment_id(
    source_id: &str,
    source: &MeetingTranscript,
    target: &MeetingTranscript,
    matches: &[Option<usize>],
) -> Option<String> {
    let source_index = source
        .segments
        .iter()
        .position(|segment| segment.id == source_id)?;
    let target_index = matches.get(source_index).copied().flatten()?;
    target
        .segments
        .get(target_index)
        .map(|segment| segment.id.clone())
}

fn map_speaker_id(
    source_id: &str,
    source: &MeetingTranscript,
    target: &MeetingTranscript,
    matches: &[Option<usize>],
) -> Option<String> {
    let source_index = source
        .speakers
        .iter()
        .position(|speaker| speaker.id == source_id)?;
    let target_index = matches.get(source_index).copied().flatten()?;
    target
        .speakers
        .get(target_index)
        .map(|speaker| speaker.id.clone())
}

fn is_semantic_conflict(error: &MeetingReviewError) -> bool {
    matches!(
        error,
        MeetingReviewError::InvalidField(_)
            | MeetingReviewError::InvalidOperation(_)
            | MeetingReviewError::UnknownSegment
            | MeetingReviewError::UnknownSpeaker
            | MeetingReviewError::Transcript(
                MeetingError::InvalidField(_)
                    | MeetingError::DuplicateId(_)
                    | MeetingError::UnknownSpeaker,
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{MeetingSegmentInput, MeetingSpeaker};

    fn transcript(source: &str) -> MeetingTranscript {
        MeetingTranscript::new(
            source,
            "en",
            "tiny",
            4.0,
            vec![
                MeetingSegmentInput {
                    start: 0.0,
                    end: 1.0,
                    text: "hello".into(),
                    speaker: "a".into(),
                },
                MeetingSegmentInput {
                    start: 1.0,
                    end: 2.0,
                    text: "world".into(),
                    speaker: "b".into(),
                },
            ],
            vec![
                MeetingSpeaker {
                    id: "a".into(),
                    label: "Alice".into(),
                },
                MeetingSpeaker {
                    id: "b".into(),
                    label: "Bob".into(),
                },
            ],
        )
        .expect("valid transcript")
    }

    fn file(review: &MeetingReview, operations: Vec<CorrectionOperation>) -> CorrectionFile {
        CorrectionFile {
            schema_version: REVIEW_SCHEMA_VERSION,
            source_sha256: review.original.source_sha256.clone(),
            original_revision: review.original_revision.clone(),
            expected_revision: review.revision.clone(),
            operations,
        }
    }

    fn edit(segment_id: &str, text: &str) -> CorrectionOperation {
        CorrectionOperation::EditSegment {
            segment_id: segment_id.into(),
            text: Some(text.into()),
            speaker_id: None,
        }
    }

    fn retargeted_transcript() -> MeetingTranscript {
        let mut target = transcript(&"a".repeat(64));
        target.segments[0].id = "seg-000010".into();
        target.segments[0].speaker = "speaker-x".into();
        target.segments[1].id = "seg-000020".into();
        target.segments[1].speaker = "speaker-y".into();
        target.speakers[0].id = "speaker-x".into();
        target.speakers[1].id = "speaker-y".into();
        target.speakers[0].label = "Renamed Alice".into();
        target.speakers[1].label = "Renamed Bob".into();
        target.validate().expect("retargeted transcript");
        target
    }

    #[test]
    fn maps_text_speaker_and_speaker_operations_by_original_signatures() {
        let old = transcript(&"a".repeat(64));
        let previous = MeetingReview::new(old).expect("review");
        let first = previous
            .apply_corrections(&file(
                &previous,
                vec![CorrectionOperation::EditSegment {
                    segment_id: "seg-000001".into(),
                    text: Some("edited".into()),
                    speaker_id: Some("b".into()),
                }],
            ))
            .expect("first correction");
        let previous = first
            .apply_corrections(&file(
                &first,
                vec![
                    CorrectionOperation::RenameSpeaker {
                        speaker_id: "b".into(),
                        label: "Béatrice".into(),
                    },
                    CorrectionOperation::MergeSpeakers {
                        from_id: "b".into(),
                        into_id: "a".into(),
                    },
                ],
            ))
            .expect("speaker corrections");

        let result = preview_migration(&previous, &retargeted_transcript(), &BTreeMap::new())
            .expect("preview");
        assert!(result
            .steps
            .iter()
            .all(|step| step.status == MigrationStatus::Applied));
        assert_eq!(result.steps.len(), 3);
        assert_eq!(
            result.steps[0].mapped,
            Some(vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000010".into(),
                text: Some("edited".into()),
                speaker_id: Some("speaker-y".into()),
            }])
        );
        let materialized = result.candidate.materialize().expect("candidate");
        assert_eq!(materialized.segments[0].text, "edited");
        assert_eq!(materialized.segments[0].speaker, "speaker-x");
        assert_eq!(materialized.speakers.len(), 1);
        assert_eq!(materialized.speakers[0].id, "speaker-x");
        assert_eq!(materialized.speakers[0].label, "Renamed Alice");
    }

    #[test]
    fn replays_multiple_edits_for_one_original_segment_in_order() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(
                &previous,
                vec![edit("seg-000001", "first"), edit("seg-000001", "final")],
            ))
            .expect("corrections");

        let result = preview_migration(&previous, &transcript(&"a".repeat(64)), &BTreeMap::new())
            .expect("preview");
        assert_eq!(
            result
                .steps
                .iter()
                .map(|step| step.status)
                .collect::<Vec<_>>(),
            vec![MigrationStatus::Applied, MigrationStatus::Applied]
        );
        assert_eq!(
            result.candidate.materialize().expect("candidate").segments[0].text,
            "final"
        );
    }

    #[test]
    fn shifted_split_and_joined_signatures_do_not_fuzzy_map() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(
                &previous,
                vec![edit("seg-000001", "first"), edit("seg-000002", "second")],
            ))
            .expect("corrections");

        let mut shifted = transcript(&"a".repeat(64));
        shifted.segments[0].start = 0.25;
        shifted.validate().expect("shifted transcript");
        let split = MeetingTranscript::new(
            "a".repeat(64),
            "en",
            "tiny",
            4.0,
            vec![
                MeetingSegmentInput {
                    start: 0.0,
                    end: 0.5,
                    text: "hel".into(),
                    speaker: "a".into(),
                },
                MeetingSegmentInput {
                    start: 0.5,
                    end: 1.0,
                    text: "lo".into(),
                    speaker: "a".into(),
                },
                MeetingSegmentInput {
                    start: 1.0,
                    end: 2.0,
                    text: "world".into(),
                    speaker: "b".into(),
                },
            ],
            vec![
                MeetingSpeaker {
                    id: "a".into(),
                    label: "Alice".into(),
                },
                MeetingSpeaker {
                    id: "b".into(),
                    label: "Bob".into(),
                },
            ],
        )
        .expect("split transcript");
        let joined = MeetingTranscript::new(
            "a".repeat(64),
            "en",
            "tiny",
            4.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 2.0,
                text: "hello world".into(),
                speaker: "a".into(),
            }],
            vec![MeetingSpeaker {
                id: "a".into(),
                label: "Alice".into(),
            }],
        )
        .expect("joined transcript");

        for proposed in [shifted, split, joined] {
            let result =
                preview_migration(&previous, &proposed, &BTreeMap::new()).expect("preview");
            assert_eq!(result.steps[0].status, MigrationStatus::Conflict);
            assert_eq!(result.steps[1].status, MigrationStatus::Blocked);
            assert!(result.candidate.batches.is_empty());
        }
    }

    #[test]
    fn changed_source_disables_automatic_mapping_but_override_recovers_prefix() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(
                &previous,
                vec![edit("seg-000001", "first"), edit("seg-000002", "second")],
            ))
            .expect("corrections");
        let proposed = transcript(&"b".repeat(64));
        let override_operation = edit("seg-000001", "explicit");
        let overrides = BTreeMap::from([(0usize, vec![override_operation])]);

        let result = preview_migration(&previous, &proposed, &overrides).expect("preview");
        assert_eq!(result.steps[0].status, MigrationStatus::Applied);
        assert_eq!(result.steps[1].status, MigrationStatus::Conflict);
        assert_eq!(result.steps[1].mapped, None);
        assert_eq!(
            result.candidate.materialize().expect("candidate").segments[0].text,
            "explicit"
        );
    }

    #[test]
    fn first_unmapped_or_invalid_group_blocks_every_later_group() {
        let old = transcript(&"a".repeat(64));
        let previous = MeetingReview::new(old).expect("review");
        let previous = previous
            .apply_corrections(&file(
                &previous,
                vec![edit("seg-000001", "first"), edit("seg-000002", "second")],
            ))
            .expect("corrections");
        let mut proposed = transcript(&"a".repeat(64));
        proposed.segments[0].text = "changed source text".into();
        proposed.validate().expect("proposed transcript");

        let result = preview_migration(&previous, &proposed, &BTreeMap::new()).expect("preview");
        assert_eq!(result.steps[0].status, MigrationStatus::Conflict);
        assert_eq!(result.steps[1].status, MigrationStatus::Blocked);
        assert!(result.candidate.batches.is_empty());

        let invalid_override =
            BTreeMap::from([(0usize, vec![edit("does-not-exist", "invalid target")])]);
        let result = preview_migration(&previous, &transcript(&"a".repeat(64)), &invalid_override)
            .expect("preview");
        assert_eq!(result.steps[0].status, MigrationStatus::Conflict);
        assert_eq!(result.steps[1].status, MigrationStatus::Blocked);
    }

    #[test]
    fn validates_pending_override_groups_before_blocked_replay() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(
                &previous,
                vec![edit("seg-000001", "first"), edit("seg-000002", "second")],
            ))
            .expect("corrections");
        let proposed = transcript(&"b".repeat(64));

        let malformed = BTreeMap::from([(1usize, vec![edit("", "invalid target")])]);
        assert_eq!(
            preview_migration(&previous, &proposed, &malformed),
            Err(MeetingReviewError::InvalidField("segment_id"))
        );

        let at_limit_override = BTreeMap::from([(
            1usize,
            vec![edit("seg-000001", "at group limit"); MAX_CORRECTION_OPERATIONS],
        )]);
        assert_eq!(
            preview_migration(&previous, &proposed, &at_limit_override),
            Err(MeetingReviewError::OperationLimit)
        );
    }

    #[test]
    fn override_indices_and_empty_resolutions_are_rejected() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(&previous, vec![edit("seg-000001", "changed")]))
            .expect("correction");

        assert_eq!(
            preview_migration(
                &previous,
                &transcript(&"a".repeat(64)),
                &BTreeMap::from([(1usize, vec![edit("seg-000001", "out of range")])]),
            ),
            Err(MeetingReviewError::InvalidField("overrides"))
        );
        assert_eq!(
            preview_migration(
                &previous,
                &transcript(&"a".repeat(64)),
                &BTreeMap::from([(0usize, Vec::new())]),
            ),
            Err(MeetingReviewError::InvalidField("overrides"))
        );
    }

    #[test]
    fn repeated_preview_is_deterministic_and_does_not_change_previous() {
        let previous = MeetingReview::new(transcript(&"a".repeat(64))).expect("review");
        let previous = previous
            .apply_corrections(&file(&previous, vec![edit("seg-000001", "changed")]))
            .expect("correction");
        let original_previous = previous.clone();
        let proposed = transcript(&"a".repeat(64));

        let first = preview_migration(&previous, &proposed, &BTreeMap::new()).expect("preview");
        let second = preview_migration(&previous, &proposed, &BTreeMap::new()).expect("preview");
        assert_eq!(first, second);
        assert_eq!(previous, original_previous);
        assert_eq!(first.candidate.batches.len(), 1);
    }
}
