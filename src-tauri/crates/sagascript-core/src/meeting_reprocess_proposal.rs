//! Saveable, revision-checked proposals for selective meeting reprocessing.
//!
//! A proposal keeps the immutable previous review and a newly inferred
//! transcript separate from the active review.  Each previous correction
//! operation has one disposition in `resolutions`; accepting a proposal is the
//! only operation that creates a new [`MeetingReview`].

use std::collections::BTreeMap;

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::meeting::MeetingTranscript;
use crate::meeting_reprocess::{preview_migration, MigrationPreview, MigrationStatus};
use crate::meeting_review::{
    CorrectionFile, CorrectionOperation, MeetingReview, MeetingReviewError,
    MAX_CORRECTION_OPERATIONS, MAX_REVIEW_GENERATION, REVIEW_SCHEMA_VERSION,
};

pub const REPROCESS_PROPOSAL_SCHEMA_VERSION: u32 = 1;
pub const MAX_SERIALIZED_PROPOSAL_BYTES: usize = 72 * 1024 * 1024;

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Errors returned while creating, validating, resolving, or accepting a
/// selective reprocessing proposal.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProposalError {
    #[error("invalid meeting review: {0}")]
    Review(#[from] MeetingReviewError),
    #[error("unsupported meeting reprocessing proposal schema version {0}")]
    UnsupportedSchema(u32),
    #[error("invalid meeting reprocessing proposal field {0}")]
    InvalidField(&'static str),
    #[error("meeting reprocessing proposal revision does not match")]
    RevisionMismatch,
    #[error("previous meeting review revision does not match")]
    PreviousRevisionMismatch,
    #[error("meeting reprocessing proposal generation overflowed")]
    GenerationOverflow,
    #[error("meeting reprocessing proposal exceeds its serialized size limit")]
    SerializedSizeLimit,
    #[error("meeting reprocessing proposal serialization failed")]
    Serialization,
    #[error("meeting reprocessing proposal has unresolved corrections")]
    UnresolvedCorrections,
}

/// A version-1, non-published selective reprocessing proposal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingReprocessingProposal {
    pub schema_version: u32,
    pub previous: MeetingReview,
    pub proposed: MeetingTranscript,
    pub generation: u64,
    pub resolutions: Vec<Option<Vec<CorrectionOperation>>>,
    pub revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeetingReprocessingProposalWire {
    schema_version: u32,
    previous: MeetingReview,
    proposed: MeetingTranscript,
    generation: u64,
    resolutions: Vec<Option<Vec<CorrectionOperation>>>,
    revision: String,
}

impl<'de> Deserialize<'de> for MeetingReprocessingProposal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MeetingReprocessingProposalWire::deserialize(deserializer)?;
        let proposal = Self {
            schema_version: wire.schema_version,
            previous: wire.previous,
            proposed: wire.proposed,
            generation: wire.generation,
            resolutions: wire.resolutions,
            revision: wire.revision,
        };
        proposal.validate().map_err(D::Error::custom)?;
        Ok(proposal)
    }
}

#[derive(Serialize)]
struct RevisionPayload<'a> {
    schema_version: u32,
    previous: &'a MeetingReview,
    proposed: &'a MeetingTranscript,
    generation: u64,
    resolutions: &'a [Option<Vec<CorrectionOperation>>],
}

impl MeetingReprocessingProposal {
    /// Create an unresolved proposal with one empty disposition per prior
    /// correction operation.
    pub fn new(
        previous: MeetingReview,
        proposed: MeetingTranscript,
    ) -> Result<Self, ProposalError> {
        previous.validate()?;
        proposed
            .validate()
            .map_err(|error| ProposalError::Review(MeetingReviewError::Transcript(error)))?;

        let resolutions = vec![None; operation_count(&previous)?];
        let mut proposal = Self {
            schema_version: REPROCESS_PROPOSAL_SCHEMA_VERSION,
            previous,
            proposed,
            generation: 0,
            resolutions,
            revision: String::new(),
        };
        proposal.revision = proposal.compute_revision()?;
        proposal.validate()?;
        Ok(proposal)
    }

    /// Validate all public fields, including the revision over every field
    /// except `revision` itself.
    pub fn validate(&self) -> Result<(), ProposalError> {
        if self.schema_version != REPROCESS_PROPOSAL_SCHEMA_VERSION {
            return Err(ProposalError::UnsupportedSchema(self.schema_version));
        }
        self.previous.validate()?;
        self.proposed
            .validate()
            .map_err(|error| ProposalError::Review(MeetingReviewError::Transcript(error)))?;
        if self.generation > MAX_REVIEW_GENERATION {
            return Err(ProposalError::GenerationOverflow);
        }

        let expected_count = operation_count(&self.previous)?;
        if self.resolutions.len() != expected_count {
            return Err(ProposalError::InvalidField("resolutions"));
        }

        let mut effective_operation_count = 0usize;
        for resolution in &self.resolutions {
            let count = match resolution {
                Some(operations) => {
                    validate_resolution(operations)?;
                    operations.len()
                }
                None => 1,
            };
            effective_operation_count = effective_operation_count
                .checked_add(count)
                .ok_or(ProposalError::InvalidField("resolutions"))?;
        }
        if effective_operation_count > MAX_CORRECTION_OPERATIONS {
            return Err(ProposalError::Review(MeetingReviewError::OperationLimit));
        }

        if !is_sha256(&self.revision) {
            return Err(ProposalError::InvalidField("revision"));
        }
        if self.compute_revision()? != self.revision {
            return Err(ProposalError::RevisionMismatch);
        }
        if serialized_size(self)? > MAX_SERIALIZED_PROPOSAL_BYTES {
            return Err(ProposalError::SerializedSizeLimit);
        }
        Ok(())
    }

    /// Return the current automatic/explicit migration preview.
    pub fn preview(&self) -> Result<MigrationPreview, ProposalError> {
        self.validate()?;
        let overrides = self
            .resolutions
            .iter()
            .enumerate()
            .filter_map(|(index, operations)| {
                operations
                    .as_ref()
                    .map(|operations| (index, operations.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        preview_migration(&self.previous, &self.proposed, &overrides).map_err(ProposalError::Review)
    }

    /// Replace one disposition, returning a new revision without mutating the
    /// original proposal.
    pub fn resolve(
        &self,
        expected_revision: &str,
        index: usize,
        operations: Vec<CorrectionOperation>,
    ) -> Result<Self, ProposalError> {
        self.validate()?;
        if expected_revision != self.revision {
            return Err(ProposalError::RevisionMismatch);
        }
        if index >= self.resolutions.len() {
            return Err(ProposalError::InvalidField("resolution index"));
        }
        validate_resolution(&operations)?;

        let generation = next_generation(self.generation)?;
        let mut next = self.clone();
        next.generation = generation;
        next.resolutions[index] = Some(operations);
        next.revision = next.compute_revision()?;
        next.validate()?;
        Ok(next)
    }

    /// Publish a fully resolved proposal as a new ordinary meeting review.
    pub fn accept(&self, expected_previous_revision: &str) -> Result<MeetingReview, ProposalError> {
        self.validate()?;
        if expected_previous_revision != self.previous.revision {
            return Err(ProposalError::PreviousRevisionMismatch);
        }

        let preview = self.preview()?;
        if preview
            .steps
            .iter()
            .any(|step| step.status != MigrationStatus::Applied)
        {
            return Err(ProposalError::UnresolvedCorrections);
        }

        Ok(preview.candidate)
    }

    fn compute_revision(&self) -> Result<String, ProposalError> {
        hash_json(&RevisionPayload {
            schema_version: self.schema_version,
            previous: &self.previous,
            proposed: &self.proposed,
            generation: self.generation,
            resolutions: &self.resolutions,
        })
    }
}

fn operation_count(review: &MeetingReview) -> Result<usize, ProposalError> {
    review.batches.iter().try_fold(0usize, |count, batch| {
        count
            .checked_add(batch.operations.len())
            .ok_or(ProposalError::Review(MeetingReviewError::OperationLimit))
    })
}

fn validate_resolution(operations: &[CorrectionOperation]) -> Result<(), ProposalError> {
    if operations.is_empty() {
        return Err(ProposalError::InvalidField("resolutions"));
    }
    let file = CorrectionFile {
        schema_version: REVIEW_SCHEMA_VERSION,
        source_sha256: ZERO_SHA256.to_owned(),
        original_revision: ZERO_SHA256.to_owned(),
        expected_revision: ZERO_SHA256.to_owned(),
        operations: operations.to_vec(),
    };
    file.validate().map_err(ProposalError::Review)
}

fn next_generation(generation: u64) -> Result<u64, ProposalError> {
    if generation >= MAX_REVIEW_GENERATION {
        return Err(ProposalError::GenerationOverflow);
    }
    generation
        .checked_add(1)
        .ok_or(ProposalError::GenerationOverflow)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, ProposalError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| ProposalError::Serialization)
}

fn hash_json<T: Serialize>(value: &T) -> Result<String, ProposalError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ProposalError::Serialization)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{MeetingSegmentInput, MeetingSpeaker};

    fn transcript(source: &str, text: &str) -> MeetingTranscript {
        MeetingTranscript::new(
            source,
            "en",
            "tiny",
            4.0,
            vec![
                MeetingSegmentInput {
                    start: 0.0,
                    end: 1.0,
                    text: text.to_owned(),
                    speaker: "speaker-a".into(),
                },
                MeetingSegmentInput {
                    start: 1.0,
                    end: 2.0,
                    text: "world".into(),
                    speaker: "speaker-a".into(),
                },
            ],
            vec![MeetingSpeaker {
                id: "speaker-a".into(),
                label: "Alice".into(),
            }],
        )
        .expect("valid transcript")
    }

    fn edit(segment_id: &str, text: &str) -> CorrectionOperation {
        CorrectionOperation::EditSegment {
            segment_id: segment_id.to_owned(),
            text: Some(text.to_owned()),
            speaker_id: None,
        }
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

    fn previous_review() -> MeetingReview {
        let review = MeetingReview::new(transcript(&"a".repeat(64), "hello")).expect("review");
        review
            .apply_corrections(&file(&review, vec![edit("seg-000001", "prior edit")]))
            .expect("previous correction")
    }

    fn previous_review_with_two_edits() -> MeetingReview {
        let review = previous_review();
        review
            .apply_corrections(&file(
                &review,
                vec![edit("seg-000002", "prior second edit")],
            ))
            .expect("second previous correction")
    }

    #[test]
    fn json_roundtrip_preserves_unresolved_proposal() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review(),
            transcript(&"b".repeat(64), "hello"),
        )
        .expect("proposal");
        let json = serde_json::to_string(&proposal).expect("proposal json");
        let decoded: MeetingReprocessingProposal =
            serde_json::from_str(&json).expect("proposal roundtrip");
        assert_eq!(decoded, proposal);
        assert_eq!(decoded.resolutions, vec![None]);
    }

    #[test]
    fn tampered_schema_revision_count_and_unknown_field_are_rejected() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review(),
            transcript(&"b".repeat(64), "hello"),
        )
        .expect("proposal");
        let mut schema = serde_json::to_value(&proposal).expect("value");
        schema["schema_version"] = serde_json::json!(2);
        assert!(serde_json::from_value::<MeetingReprocessingProposal>(schema).is_err());

        let mut revision = serde_json::to_value(&proposal).expect("value");
        revision["revision"] = serde_json::json!("f".repeat(64));
        assert!(serde_json::from_value::<MeetingReprocessingProposal>(revision).is_err());

        let mut count = serde_json::to_value(&proposal).expect("value");
        count["resolutions"] = serde_json::json!([]);
        assert!(serde_json::from_value::<MeetingReprocessingProposal>(count).is_err());

        let mut unknown = serde_json::to_value(&proposal).expect("value");
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<MeetingReprocessingProposal>(unknown).is_err());
    }

    #[test]
    fn generation_limit_and_stale_resolution_are_rejected() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review(),
            transcript(&"b".repeat(64), "hello"),
        )
        .expect("proposal");
        let mut over_limit = proposal.clone();
        over_limit.generation = MAX_REVIEW_GENERATION + 1;
        assert_eq!(
            over_limit.validate(),
            Err(ProposalError::GenerationOverflow)
        );

        assert_eq!(
            proposal.resolve(&"0".repeat(64), 0, vec![edit("seg-000001", "target")]),
            Err(ProposalError::RevisionMismatch)
        );
    }

    #[test]
    fn stale_previous_revision_and_unresolved_conflict_prevent_accept() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review(),
            transcript(&"b".repeat(64), "changed source"),
        )
        .expect("proposal");
        assert_eq!(
            proposal.accept(&"0".repeat(64)),
            Err(ProposalError::PreviousRevisionMismatch)
        );
        assert_eq!(
            proposal.accept(&proposal.previous.revision),
            Err(ProposalError::UnresolvedCorrections)
        );
    }

    #[test]
    fn explicit_resolution_accepts_and_replays_each_operation_once() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review(),
            transcript(&"b".repeat(64), "changed source"),
        )
        .expect("proposal");
        let resolved = proposal
            .resolve(
                &proposal.revision,
                0,
                vec![edit("seg-000001", "target edit")],
            )
            .expect("resolution");
        assert_ne!(resolved.revision, proposal.revision);
        assert_eq!(proposal.resolutions, vec![None]);

        let accepted = resolved
            .accept(&resolved.previous.revision)
            .expect("accepted proposal");
        assert_eq!(accepted.batches.len(), 1);
        assert_eq!(accepted.batches[0].operations.len(), 1);
        assert_eq!(
            accepted.materialize().expect("materialized").segments[0].text,
            "target edit"
        );
    }

    #[test]
    fn acceptance_retains_automatic_and_explicit_migrations() {
        let proposal = MeetingReprocessingProposal::new(
            previous_review_with_two_edits(),
            transcript(&"a".repeat(64), "hello"),
        )
        .expect("proposal");
        let resolved = proposal
            .resolve(
                &proposal.revision,
                1,
                vec![edit("seg-000002", "explicit second edit")],
            )
            .expect("explicit resolution");

        let accepted = resolved
            .accept(&resolved.previous.revision)
            .expect("accepted proposal");
        let materialized = accepted.materialize().expect("materialized");
        assert_eq!(materialized.segments[0].text, "prior edit");
        assert_eq!(materialized.segments[1].text, "explicit second edit");
        assert_eq!(accepted.batches.len(), 2);
    }
}
