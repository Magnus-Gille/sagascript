//! Pure, versioned meeting-transcript corrections.
//!
//! A [`MeetingReview`] keeps the machine-produced [`MeetingTranscript`] immutable
//! and stores user changes as ordered, replayable correction batches.  The
//! review is deliberately independent from the transcript schema so existing
//! transcript files remain compatible.

use std::fmt;

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::meeting::{MeetingError, MeetingExportFormat, MeetingTranscript};

pub const REVIEW_SCHEMA_VERSION: u32 = 1;
pub const MAX_CORRECTION_OPERATIONS: usize = 1024;
pub const MAX_SERIALIZED_REVIEW_BYTES: usize = 24 * 1024 * 1024;
/// Highest generation representable exactly by the JavaScript `number` type.
pub const MAX_REVIEW_GENERATION: u64 = 9_007_199_254_740_991;

const MAX_ID_CHARS: usize = 128;
const MAX_LABEL_CHARS: usize = 128;

/// Errors returned by review construction, validation, replay, and export.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MeetingReviewError {
    #[error("invalid meeting transcript: {0}")]
    Transcript(#[from] MeetingError),
    #[error("unsupported meeting review schema version {0}")]
    UnsupportedSchema(u32),
    #[error("invalid meeting review field {0}")]
    InvalidField(&'static str),
    #[error("invalid correction operation: {0}")]
    InvalidOperation(&'static str),
    #[error("unknown meeting segment")]
    UnknownSegment,
    #[error("unknown meeting speaker")]
    UnknownSpeaker,
    #[error("correction source does not match the original audio source")]
    SourceMismatch,
    #[error("correction original revision does not match this review")]
    OriginalRevisionMismatch,
    #[error("correction expected revision does not match this review")]
    ExpectedRevisionMismatch,
    #[error("review revision is invalid")]
    InvalidRevision,
    #[error("cannot undo a review with no correction batches")]
    NothingToUndo,
    #[error("meeting review generation overflowed")]
    GenerationOverflow,
    #[error("meeting review exceeds its operation limit")]
    OperationLimit,
    #[error("meeting review exceeds its serialized size limit")]
    SerializedSizeLimit,
    #[error("meeting review serialization failed")]
    Serialization,
}

/// A version-1 correction request.  Applying one file creates one undo batch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionFile {
    pub schema_version: u32,
    pub source_sha256: String,
    pub original_revision: String,
    pub expected_revision: String,
    pub operations: Vec<CorrectionOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectionFileWire {
    schema_version: u32,
    source_sha256: String,
    original_revision: String,
    expected_revision: String,
    operations: Vec<CorrectionOperation>,
}

impl<'de> Deserialize<'de> for CorrectionFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CorrectionFileWire::deserialize(deserializer)?;
        let file = Self {
            schema_version: wire.schema_version,
            source_sha256: wire.source_sha256,
            original_revision: wire.original_revision,
            expected_revision: wire.expected_revision,
            operations: wire.operations,
        };
        file.validate().map_err(D::Error::custom)?;
        Ok(file)
    }
}

impl CorrectionFile {
    pub fn validate(&self) -> Result<(), MeetingReviewError> {
        if self.schema_version != REVIEW_SCHEMA_VERSION {
            return Err(MeetingReviewError::UnsupportedSchema(self.schema_version));
        }
        if !is_sha256(&self.source_sha256) {
            return Err(MeetingReviewError::InvalidField("source_sha256"));
        }
        if !is_sha256(&self.original_revision) {
            return Err(MeetingReviewError::InvalidField("original_revision"));
        }
        if !is_sha256(&self.expected_revision) {
            return Err(MeetingReviewError::InvalidField("expected_revision"));
        }
        if self.operations.is_empty() {
            return Err(MeetingReviewError::InvalidField("operations"));
        }
        if self.operations.len() > MAX_CORRECTION_OPERATIONS {
            return Err(MeetingReviewError::OperationLimit);
        }
        for operation in &self.operations {
            operation.validate_shape()?;
        }
        serialized_size(self).map(|size| {
            if size > MAX_SERIALIZED_REVIEW_BYTES {
                Err(MeetingReviewError::SerializedSizeLimit)
            } else {
                Ok(())
            }
        })?
    }
}

/// A tagged, explicit correction operation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CorrectionOperation {
    EditSegment {
        segment_id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        speaker_id: Option<String>,
    },
    RenameSpeaker {
        speaker_id: String,
        label: String,
    },
    MergeSpeakers {
        from_id: String,
        into_id: String,
    },
}

impl<'de> Deserialize<'de> for CorrectionOperation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let mut fields = match value {
            serde_json::Value::Object(fields) => fields,
            _ => return Err(D::Error::custom("correction operation must be an object")),
        };
        let kind = fields
            .remove("kind")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| D::Error::custom("correction operation kind is required"))?;
        let payload = serde_json::Value::Object(fields);
        let operation = match kind.as_str() {
            "edit_segment" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct EditWire {
                    segment_id: String,
                    #[serde(default)]
                    text: Option<String>,
                    #[serde(default)]
                    speaker_id: Option<String>,
                }
                let operation: EditWire =
                    serde_json::from_value(payload).map_err(D::Error::custom)?;
                Self::EditSegment {
                    segment_id: operation.segment_id,
                    text: operation.text,
                    speaker_id: operation.speaker_id,
                }
            }
            "rename_speaker" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct RenameWire {
                    speaker_id: String,
                    label: String,
                }
                let operation: RenameWire =
                    serde_json::from_value(payload).map_err(D::Error::custom)?;
                Self::RenameSpeaker {
                    speaker_id: operation.speaker_id,
                    label: operation.label,
                }
            }
            "merge_speakers" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct MergeWire {
                    from_id: String,
                    into_id: String,
                }
                let operation: MergeWire =
                    serde_json::from_value(payload).map_err(D::Error::custom)?;
                Self::MergeSpeakers {
                    from_id: operation.from_id,
                    into_id: operation.into_id,
                }
            }
            _ => return Err(D::Error::custom("unsupported correction operation kind")),
        };
        operation.validate_shape().map_err(D::Error::custom)?;
        Ok(operation)
    }
}

impl CorrectionOperation {
    fn validate_shape(&self) -> Result<(), MeetingReviewError> {
        match self {
            Self::EditSegment {
                segment_id,
                text,
                speaker_id,
            } => {
                validate_id(segment_id, "segment_id")?;
                if text.is_none() && speaker_id.is_none() {
                    return Err(MeetingReviewError::InvalidOperation(
                        "edit_segment needs text or speaker_id",
                    ));
                }
                if let Some(speaker_id) = speaker_id {
                    validate_id(speaker_id, "speaker_id")?;
                }
            }
            Self::RenameSpeaker { speaker_id, label } => {
                validate_id(speaker_id, "speaker_id")?;
                validate_label(label)?;
            }
            Self::MergeSpeakers { from_id, into_id } => {
                validate_id(from_id, "from_id")?;
                validate_id(into_id, "into_id")?;
                if from_id == into_id {
                    return Err(MeetingReviewError::InvalidOperation(
                        "cannot merge a speaker into itself",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// One atomic request retained as an undoable provenance batch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionBatch {
    pub operations: Vec<CorrectionOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorrectionBatchWire {
    operations: Vec<CorrectionOperation>,
}

impl<'de> Deserialize<'de> for CorrectionBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CorrectionBatchWire::deserialize(deserializer)?;
        let batch = Self {
            operations: wire.operations,
        };
        batch.validate().map_err(D::Error::custom)?;
        Ok(batch)
    }
}

impl CorrectionBatch {
    pub fn validate(&self) -> Result<(), MeetingReviewError> {
        if self.operations.is_empty() {
            return Err(MeetingReviewError::InvalidField("batches.operations"));
        }
        if self.operations.len() > MAX_CORRECTION_OPERATIONS {
            return Err(MeetingReviewError::OperationLimit);
        }
        for operation in &self.operations {
            operation.validate_shape()?;
        }
        Ok(())
    }
}

/// The immutable original transcript plus validated correction history.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingReview {
    pub schema_version: u32,
    pub original: MeetingTranscript,
    pub original_revision: String,
    pub generation: u64,
    pub batches: Vec<CorrectionBatch>,
    pub revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeetingReviewWire {
    schema_version: u32,
    original: MeetingTranscript,
    original_revision: String,
    generation: u64,
    batches: Vec<CorrectionBatch>,
    revision: String,
}

impl<'de> Deserialize<'de> for MeetingReview {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MeetingReviewWire::deserialize(deserializer)?;
        let review = Self {
            schema_version: wire.schema_version,
            original: wire.original,
            original_revision: wire.original_revision,
            generation: wire.generation,
            batches: wire.batches,
            revision: wire.revision,
        };
        review.validate().map_err(D::Error::custom)?;
        Ok(review)
    }
}

#[derive(Serialize)]
struct RevisionPayload<'a> {
    schema_version: u32,
    original_revision: &'a str,
    generation: u64,
    batches: &'a [CorrectionBatch],
}

impl MeetingReview {
    /// Create a review with no corrections and generation zero.
    pub fn new(original: MeetingTranscript) -> Result<Self, MeetingReviewError> {
        original.validate()?;
        let original_revision = hash_json(&original)?;
        let mut review = Self {
            schema_version: REVIEW_SCHEMA_VERSION,
            original,
            original_revision,
            generation: 0,
            batches: Vec::new(),
            revision: String::new(),
        };
        review.revision = review.compute_revision()?;
        review.validate()?;
        Ok(review)
    }

    pub fn validate(&self) -> Result<(), MeetingReviewError> {
        self.validate_and_materialize().map(|_| ())
    }

    fn validate_and_materialize(&self) -> Result<MeetingTranscript, MeetingReviewError> {
        if self.schema_version != REVIEW_SCHEMA_VERSION {
            return Err(MeetingReviewError::UnsupportedSchema(self.schema_version));
        }
        if self.generation > MAX_REVIEW_GENERATION {
            return Err(MeetingReviewError::GenerationOverflow);
        }
        self.original.validate()?;
        if !is_sha256(&self.original_revision) || !is_sha256(&self.revision) {
            return Err(MeetingReviewError::InvalidRevision);
        }
        if hash_json(&self.original)? != self.original_revision {
            return Err(MeetingReviewError::InvalidRevision);
        }
        let operation_count = self.batches.iter().try_fold(0usize, |count, batch| {
            batch.validate()?;
            count
                .checked_add(batch.operations.len())
                .ok_or(MeetingReviewError::OperationLimit)
        })?;
        if operation_count > MAX_CORRECTION_OPERATIONS {
            return Err(MeetingReviewError::OperationLimit);
        }
        let materialized = self.replay_batches()?;
        if self.compute_revision()? != self.revision {
            return Err(MeetingReviewError::InvalidRevision);
        }
        if serialized_size(self)? > MAX_SERIALIZED_REVIEW_BYTES {
            return Err(MeetingReviewError::SerializedSizeLimit);
        }
        Ok(materialized)
    }

    /// Return the transcript obtained by replaying all retained batches.
    pub fn materialize(&self) -> Result<MeetingTranscript, MeetingReviewError> {
        self.validate_and_materialize()
    }

    /// Apply one validated correction file as one atomic undo batch.
    pub fn apply_corrections(&self, file: &CorrectionFile) -> Result<Self, MeetingReviewError> {
        let mut materialized = self.validate_and_materialize()?;
        file.validate()?;
        if file.source_sha256 != self.original.source_sha256 {
            return Err(MeetingReviewError::SourceMismatch);
        }
        if file.original_revision != self.original_revision {
            return Err(MeetingReviewError::OriginalRevisionMismatch);
        }
        if file.expected_revision != self.revision {
            return Err(MeetingReviewError::ExpectedRevisionMismatch);
        }
        let next_generation = next_generation(self.generation)?;
        let new_count = self.operation_count()? + file.operations.len();
        if new_count > MAX_CORRECTION_OPERATIONS {
            return Err(MeetingReviewError::OperationLimit);
        }

        for operation in &file.operations {
            materialized = apply_operation(&materialized, operation)?;
        }
        let mut next = self.clone();
        next.generation = next_generation;
        next.batches.push(CorrectionBatch {
            operations: file.operations.clone(),
        });
        next.revision = next.compute_revision()?;
        next.validate()?;
        Ok(next)
    }

    /// Remove the latest correction batch, retaining the immutable original.
    pub fn undo(&self, expected_revision: &str) -> Result<Self, MeetingReviewError> {
        self.validate()?;
        self.check_expected_revision(expected_revision)?;
        if self.batches.is_empty() {
            return Err(MeetingReviewError::NothingToUndo);
        }
        let generation = next_generation(self.generation)?;
        let mut next = self.clone();
        next.batches.pop();
        next.generation = generation;
        next.revision = next.compute_revision()?;
        next.validate()?;
        Ok(next)
    }

    /// Remove all correction batches and restore the original machine output.
    pub fn reset(&self, expected_revision: &str) -> Result<Self, MeetingReviewError> {
        self.validate()?;
        self.check_expected_revision(expected_revision)?;
        let generation = next_generation(self.generation)?;
        let mut next = self.clone();
        next.batches.clear();
        next.generation = generation;
        next.revision = next.compute_revision()?;
        next.validate()?;
        Ok(next)
    }

    /// Export reviewed presentation formats, or the full review envelope as JSON.
    pub fn export(&self, format: MeetingExportFormat) -> Result<String, MeetingReviewError> {
        let materialized = self.validate_and_materialize()?;
        if format == MeetingExportFormat::Json {
            return serde_json::to_string(self).map_err(|_| MeetingReviewError::Serialization);
        }
        materialized
            .export(format)
            .map_err(MeetingReviewError::Transcript)
    }

    fn operation_count(&self) -> Result<usize, MeetingReviewError> {
        self.batches.iter().try_fold(0usize, |count, batch| {
            count
                .checked_add(batch.operations.len())
                .ok_or(MeetingReviewError::OperationLimit)
        })
    }

    fn check_expected_revision(&self, expected_revision: &str) -> Result<(), MeetingReviewError> {
        if !is_sha256(expected_revision) {
            return Err(MeetingReviewError::InvalidRevision);
        }
        if expected_revision != self.revision {
            return Err(MeetingReviewError::ExpectedRevisionMismatch);
        }
        Ok(())
    }

    fn compute_revision(&self) -> Result<String, MeetingReviewError> {
        hash_json(&RevisionPayload {
            schema_version: self.schema_version,
            original_revision: &self.original_revision,
            generation: self.generation,
            batches: &self.batches,
        })
    }

    fn replay_batches(&self) -> Result<MeetingTranscript, MeetingReviewError> {
        let mut materialized = self.original.clone();
        for batch in &self.batches {
            for operation in &batch.operations {
                materialized = apply_operation(&materialized, operation)?;
            }
        }
        Ok(materialized)
    }
}

fn apply_operation(
    transcript: &MeetingTranscript,
    operation: &CorrectionOperation,
) -> Result<MeetingTranscript, MeetingReviewError> {
    operation.validate_shape()?;
    match operation {
        CorrectionOperation::EditSegment {
            segment_id,
            text,
            speaker_id,
        } => {
            let mut next = transcript.clone();
            if let Some(speaker_id) = speaker_id {
                if !next
                    .speakers
                    .iter()
                    .any(|speaker| speaker.id == *speaker_id)
                {
                    return Err(MeetingReviewError::UnknownSpeaker);
                }
            }
            let segment = next
                .segments
                .iter_mut()
                .find(|segment| segment.id == *segment_id)
                .ok_or(MeetingReviewError::UnknownSegment)?;
            if let Some(text) = text {
                segment.text = text.clone();
            }
            if let Some(speaker_id) = speaker_id {
                segment.speaker = speaker_id.clone();
            }
            next.validate().map_err(MeetingReviewError::Transcript)?;
            Ok(next)
        }
        CorrectionOperation::RenameSpeaker { speaker_id, label } => transcript
            .rename_speaker(speaker_id, label.clone())
            .map_err(MeetingReviewError::Transcript),
        CorrectionOperation::MergeSpeakers { from_id, into_id } => transcript
            .merge_speakers(from_id, into_id)
            .map_err(MeetingReviewError::Transcript),
    }
}

fn validate_id(value: &str, field: &'static str) -> Result<(), MeetingReviewError> {
    if value.is_empty()
        || value.chars().count() > MAX_ID_CHARS
        || value.chars().any(char::is_control)
    {
        Err(MeetingReviewError::InvalidField(field))
    } else {
        Ok(())
    }
}

fn validate_label(value: &str) -> Result<(), MeetingReviewError> {
    if value.trim().is_empty()
        || value.chars().count() > MAX_LABEL_CHARS
        || value.chars().any(char::is_control)
    {
        Err(MeetingReviewError::InvalidField("label"))
    } else {
        Ok(())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn next_generation(generation: u64) -> Result<u64, MeetingReviewError> {
    if generation >= MAX_REVIEW_GENERATION {
        return Err(MeetingReviewError::GenerationOverflow);
    }
    generation
        .checked_add(1)
        .ok_or(MeetingReviewError::GenerationOverflow)
}

fn serialized_size<T: Serialize>(value: &T) -> Result<usize, MeetingReviewError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| MeetingReviewError::Serialization)
}

fn hash_json<T: Serialize>(value: &T) -> Result<String, MeetingReviewError> {
    let bytes = serde_json::to_vec(value).map_err(|_| MeetingReviewError::Serialization)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl fmt::Display for CorrectionOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EditSegment { segment_id, .. } => write!(f, "edit_segment:{segment_id}"),
            Self::RenameSpeaker { speaker_id, .. } => write!(f, "rename_speaker:{speaker_id}"),
            Self::MergeSpeakers { from_id, into_id } => {
                write!(f, "merge_speakers:{from_id}->{into_id}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::{MeetingSegmentInput, MeetingSpeaker};

    fn transcript() -> MeetingTranscript {
        MeetingTranscript::new(
            "a".repeat(64),
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

    #[test]
    fn new_materialize_and_hashes_are_deterministic() {
        let first = MeetingReview::new(transcript()).expect("review");
        let second = MeetingReview::new(transcript()).expect("review");
        assert_eq!(first, second);
        assert_eq!(first.materialize().expect("materialize"), first.original);
        assert_eq!(first.original_revision.len(), 64);
        assert_eq!(first.revision.len(), 64);
    }

    #[test]
    fn unicode_empty_text_and_multiple_operations_replay() {
        let review = MeetingReview::new(transcript()).expect("review");
        let next = review
            .apply_corrections(&file(
                &review,
                vec![
                    CorrectionOperation::EditSegment {
                        segment_id: "seg-000001".into(),
                        text: Some("".into()),
                        speaker_id: Some("b".into()),
                    },
                    CorrectionOperation::RenameSpeaker {
                        speaker_id: "b".into(),
                        label: "Béatrice".into(),
                    },
                ],
            ))
            .expect("corrections");
        let materialized = next.materialize().expect("materialize");
        assert_eq!(materialized.segments[0].text, "");
        assert_eq!(materialized.segments[0].speaker, "b");
        assert_eq!(materialized.speakers[1].label, "Béatrice");
        assert_eq!(next.batches.len(), 1);
    }

    #[test]
    fn merge_undo_and_reset_advance_generation_and_prevent_aba() {
        let review = MeetingReview::new(transcript()).expect("review");
        let merged = review
            .apply_corrections(&file(
                &review,
                vec![CorrectionOperation::MergeSpeakers {
                    from_id: "b".into(),
                    into_id: "a".into(),
                }],
            ))
            .expect("merge");
        let undone = merged.undo(&merged.revision).expect("undo");
        assert_eq!(undone.materialize().expect("materialize"), review.original);
        assert_eq!(undone.batches.len(), 0);
        assert_ne!(undone.revision, review.revision);
        assert_eq!(undone.generation, 2);
        let reset = merged.reset(&merged.revision).expect("reset");
        assert_eq!(reset.materialize().expect("materialize"), review.original);
        assert_ne!(reset.revision, review.revision);
        assert_eq!(reset.generation, 2);
        assert!(reset.batches.is_empty());
    }

    #[test]
    fn stale_bindings_and_invalid_operations_are_rejected() {
        let review = MeetingReview::new(transcript()).expect("review");
        let mut stale = file(
            &review,
            vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("changed".into()),
                speaker_id: None,
            }],
        );
        stale.expected_revision = "b".repeat(64);
        assert_eq!(
            review.apply_corrections(&stale),
            Err(MeetingReviewError::ExpectedRevisionMismatch)
        );

        let mut wrong_source = file(
            &review,
            vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("changed".into()),
                speaker_id: None,
            }],
        );
        wrong_source.source_sha256 = "c".repeat(64);
        assert_eq!(
            review.apply_corrections(&wrong_source),
            Err(MeetingReviewError::SourceMismatch)
        );

        let mut wrong_original = file(
            &review,
            vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("changed".into()),
                speaker_id: None,
            }],
        );
        wrong_original.original_revision = "d".repeat(64);
        assert_eq!(
            review.apply_corrections(&wrong_original),
            Err(MeetingReviewError::OriginalRevisionMismatch)
        );

        let invalid = CorrectionFile {
            operations: vec![CorrectionOperation::EditSegment {
                segment_id: "".into(),
                text: None,
                speaker_id: None,
            }],
            ..file(&review, Vec::new())
        };
        assert!(invalid.validate().is_err());
        assert_eq!(
            review.undo(&review.revision),
            Err(MeetingReviewError::NothingToUndo)
        );
    }

    #[test]
    fn strict_deserialization_detects_unknown_fields_and_tampering() {
        let review = MeetingReview::new(transcript()).expect("review");
        let json = serde_json::to_string(&review).expect("json");
        let decoded: MeetingReview = serde_json::from_str(&json).expect("round trip");
        assert_eq!(decoded, review);

        let unknown = format!(
            r#"{{"schema_version":1,"source_sha256":"{}","original_revision":"{}","expected_revision":"{}","operations":[{{"kind":"edit_segment","segment_id":"seg-000001","text":"x","unexpected":true}}]}}"#,
            review.original.source_sha256, review.original_revision, review.revision
        );
        assert!(serde_json::from_str::<CorrectionFile>(&unknown).is_err());

        let tampered = json.replace(&review.revision, &"0".repeat(64));
        assert!(serde_json::from_str::<MeetingReview>(&tampered).is_err());
    }

    #[test]
    fn correction_operations_and_nonempty_review_round_trip_through_serde() {
        let review = MeetingReview::new(transcript()).expect("review");
        let operations = vec![
            CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("edited".into()),
                speaker_id: Some("b".into()),
            },
            CorrectionOperation::RenameSpeaker {
                speaker_id: "b".into(),
                label: "Béatrice".into(),
            },
            CorrectionOperation::MergeSpeakers {
                from_id: "b".into(),
                into_id: "a".into(),
            },
        ];

        for operation in &operations {
            let json = serde_json::to_string(operation).expect("operation json");
            let decoded: CorrectionOperation =
                serde_json::from_str(&json).expect("operation round trip");
            assert_eq!(&decoded, operation);
        }

        let applied = review
            .apply_corrections(&file(&review, operations))
            .expect("corrections");
        let json = serde_json::to_string(&applied).expect("review json");
        let decoded: MeetingReview = serde_json::from_str(&json).expect("review round trip");
        assert_eq!(decoded, applied);
    }

    #[test]
    fn every_operation_rejects_unknown_fields_and_schema_versions_are_checked() {
        let review = MeetingReview::new(transcript()).expect("review");
        let operations = vec![
            CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("edited".into()),
                speaker_id: None,
            },
            CorrectionOperation::RenameSpeaker {
                speaker_id: "a".into(),
                label: "Alicia".into(),
            },
            CorrectionOperation::MergeSpeakers {
                from_id: "b".into(),
                into_id: "a".into(),
            },
        ];
        for operation in operations {
            let mut value = serde_json::to_value(operation).expect("operation value");
            value
                .as_object_mut()
                .expect("operation object")
                .insert("unexpected".into(), serde_json::json!(true));
            assert!(serde_json::from_value::<CorrectionOperation>(value).is_err());
        }

        let mut file_value = serde_json::to_value(file(
            &review,
            vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("edited".into()),
                speaker_id: None,
            }],
        ))
        .expect("file value");
        file_value["schema_version"] = serde_json::json!(2);
        assert!(serde_json::from_value::<CorrectionFile>(file_value).is_err());

        let mut review_value = serde_json::to_value(&review).expect("review value");
        review_value["schema_version"] = serde_json::json!(2);
        assert!(serde_json::from_value::<MeetingReview>(review_value).is_err());
    }

    #[test]
    fn failed_multistep_correction_is_atomic_and_unknown_speaker_is_rejected() {
        let review = MeetingReview::new(transcript()).expect("review");
        let before = serde_json::to_vec(&review).expect("review bytes");
        let result = review.apply_corrections(&file(
            &review,
            vec![
                CorrectionOperation::EditSegment {
                    segment_id: "seg-000001".into(),
                    text: Some("this must not persist".into()),
                    speaker_id: None,
                },
                CorrectionOperation::EditSegment {
                    segment_id: "seg-000002".into(),
                    text: Some("also rejected".into()),
                    speaker_id: Some("missing-speaker".into()),
                },
            ],
        ));
        assert_eq!(result, Err(MeetingReviewError::UnknownSpeaker));
        assert_eq!(serde_json::to_vec(&review).expect("review bytes"), before);
        assert_eq!(review.materialize().expect("materialize"), review.original);
    }

    #[test]
    fn exact_operation_limit_and_serialized_review_limit_are_enforced() {
        let review = MeetingReview::new(transcript()).expect("review");
        let exact_operations = (0..MAX_CORRECTION_OPERATIONS)
            .map(|index| CorrectionOperation::RenameSpeaker {
                speaker_id: "a".into(),
                label: format!("Alice {index}"),
            })
            .collect();
        let exact_file = file(&review, exact_operations);
        exact_file.validate().expect("exact operation limit");
        let exact_review = review
            .apply_corrections(&exact_file)
            .expect("exact operation limit applies");
        assert_eq!(
            exact_review.operation_count().expect("operation count"),
            MAX_CORRECTION_OPERATIONS
        );

        let mut large_original = transcript();
        let max_text_bytes = 16 * 1024 * 1024;
        let second_text_bytes = large_original.segments[1].text.len();
        large_original.segments[0].text = "x".repeat(max_text_bytes - second_text_bytes);
        let large_review = MeetingReview::new(large_original).expect("large review");
        let large_file = file(
            &large_review,
            vec![CorrectionOperation::EditSegment {
                segment_id: "seg-000001".into(),
                text: Some("y".repeat(max_text_bytes - second_text_bytes)),
                speaker_id: None,
            }],
        );
        large_file.validate().expect("large correction file");
        assert_eq!(
            large_review.apply_corrections(&large_file),
            Err(MeetingReviewError::SerializedSizeLimit)
        );
    }

    #[test]
    fn corrected_history_replays_exports_and_rejects_tampering_and_stale_undo_reset() {
        let review = MeetingReview::new(transcript()).expect("review");
        let renamed = review
            .apply_corrections(&file(
                &review,
                vec![
                    CorrectionOperation::EditSegment {
                        segment_id: "seg-000001".into(),
                        text: Some("Hej 👋".into()),
                        speaker_id: Some("b".into()),
                    },
                    CorrectionOperation::RenameSpeaker {
                        speaker_id: "b".into(),
                        label: "Béatrice".into(),
                    },
                ],
            ))
            .expect("corrections");

        let plain = renamed
            .export(MeetingExportFormat::Plain)
            .expect("plain export");
        let markdown = renamed
            .export(MeetingExportFormat::Markdown)
            .expect("markdown export");
        let json = renamed
            .export(MeetingExportFormat::Json)
            .expect("json export");
        let srt = renamed
            .export(MeetingExportFormat::Srt)
            .expect("srt export");
        let vtt = renamed
            .export(MeetingExportFormat::Vtt)
            .expect("vtt export");
        assert!(plain.contains("Hej 👋") && plain.contains("Béatrice"));
        assert!(markdown.contains("Hej 👋") && markdown.contains("Béatrice"));
        assert!(json.contains("Béatrice") && json.contains("batches"));
        assert!(srt.contains("Hej 👋") && srt.contains("Béatrice"));
        assert!(vtt.starts_with("WEBVTT") && vtt.contains("Béatrice"));

        let encoded = serde_json::to_string(&renamed).expect("review json");
        let decoded: MeetingReview = serde_json::from_str(&encoded).expect("review decode");
        assert_eq!(
            decoded.materialize().expect("decoded replay"),
            renamed.materialize().expect("replay")
        );

        let mut tampered = serde_json::to_value(&renamed).expect("review value");
        tampered["batches"][0]["operations"][0]["text"] = serde_json::json!("tampered");
        assert!(serde_json::from_value::<MeetingReview>(tampered).is_err());

        let stale_revision = "f".repeat(64);
        assert_eq!(
            renamed.undo(&stale_revision),
            Err(MeetingReviewError::ExpectedRevisionMismatch)
        );
        assert_eq!(
            renamed.reset(&stale_revision),
            Err(MeetingReviewError::ExpectedRevisionMismatch)
        );

        let merged = renamed
            .apply_corrections(&file(
                &renamed,
                vec![CorrectionOperation::MergeSpeakers {
                    from_id: "b".into(),
                    into_id: "a".into(),
                }],
            ))
            .expect("merge");
        let merged_materialized = merged.materialize().expect("merged replay");
        assert_eq!(merged_materialized.segments[0].speaker, "a");
        assert!(!merged_materialized
            .speakers
            .iter()
            .any(|speaker| speaker.id == "b"));
    }

    #[test]
    fn limits_and_exports_are_enforced() {
        let review = MeetingReview::new(transcript()).expect("review");
        let too_many = CorrectionFile {
            operations: (0..=MAX_CORRECTION_OPERATIONS)
                .map(|_| CorrectionOperation::RenameSpeaker {
                    speaker_id: "a".into(),
                    label: "Alice".into(),
                })
                .collect(),
            ..file(&review, Vec::new())
        };
        assert_eq!(too_many.validate(), Err(MeetingReviewError::OperationLimit));

        let applied = review
            .apply_corrections(&file(
                &review,
                vec![CorrectionOperation::EditSegment {
                    segment_id: "seg-000001".into(),
                    text: Some("reviewed".into()),
                    speaker_id: None,
                }],
            ))
            .expect("correction");
        let plain = applied.export(MeetingExportFormat::Plain).expect("plain");
        assert!(plain.contains("reviewed"));
        let review_json = applied
            .export(MeetingExportFormat::Json)
            .expect("review json");
        assert!(review_json.contains("original_revision"));
        assert!(review_json.contains("edit_segment"));
    }

    #[test]
    fn generation_stops_at_javascript_safe_integer() {
        let review = MeetingReview::new(transcript()).expect("review");
        let mut maxed = review.clone();
        maxed.generation = MAX_REVIEW_GENERATION;
        maxed.revision = maxed.compute_revision().expect("revision");
        maxed.validate().expect("maximum generation is valid");
        assert_eq!(
            maxed.apply_corrections(&file(
                &maxed,
                vec![CorrectionOperation::EditSegment {
                    segment_id: "seg-000001".into(),
                    text: Some("blocked".into()),
                    speaker_id: None,
                }],
            )),
            Err(MeetingReviewError::GenerationOverflow)
        );

        let mut overflow = review;
        overflow.generation = MAX_REVIEW_GENERATION + 1;
        overflow.revision = overflow.compute_revision().expect("revision");
        assert_eq!(
            overflow.validate(),
            Err(MeetingReviewError::GenerationOverflow)
        );
    }
}
