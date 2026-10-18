//! The meeting error enums as [`DictationError`]s (ADR 0006, #360 part 2), so a
//! caller can write `?` and keep a code.
//!
//! Each enum is matched exhaustively: a new variant does not compile until it
//! is classified. The text is the enum's own `Display`, unchanged; only the
//! code is new:
//!
//! - [`DictationError::MeetingStale`]: the document or recording no longer
//!   matches what the caller holds (a revision moved on, another source). Load
//!   or attach it again.
//! - [`DictationError::MeetingInvalid`]: every other refusal.
//!
//! The CLI's `meeting` commands replace the text with fixed, content-free
//! diagnostics on purpose (a transcript must not reach a log) and keep
//! `transcription_failed`; they do not use these conversions.

use crate::error::DictationError;
use crate::meeting::media::MediaError;
use crate::meeting::reprocess::plan::ReprocessingPlanError;
use crate::meeting::reprocess::proposal::ProposalError;
use crate::meeting::review::MeetingReviewError;
use crate::meeting::MeetingError;

fn invalid(error: &impl std::fmt::Display) -> DictationError {
    DictationError::MeetingInvalid(error.to_string())
}

fn stale(error: &impl std::fmt::Display) -> DictationError {
    DictationError::MeetingStale(error.to_string())
}

impl From<MeetingError> for DictationError {
    fn from(error: MeetingError) -> Self {
        match &error {
            MeetingError::InvalidField(_)
            | MeetingError::DuplicateId(_)
            | MeetingError::UnknownSpeaker
            | MeetingError::UnsupportedSchema(_)
            | MeetingError::Serialization => invalid(&error),
        }
    }
}

impl From<MeetingReviewError> for DictationError {
    fn from(error: MeetingReviewError) -> Self {
        match &error {
            MeetingReviewError::SourceMismatch
            | MeetingReviewError::OriginalRevisionMismatch
            | MeetingReviewError::ExpectedRevisionMismatch => stale(&error),
            MeetingReviewError::Transcript(_)
            | MeetingReviewError::UnsupportedSchema(_)
            | MeetingReviewError::InvalidField(_)
            | MeetingReviewError::InvalidOperation(_)
            | MeetingReviewError::UnknownSegment
            | MeetingReviewError::UnknownSpeaker
            | MeetingReviewError::InvalidRevision
            | MeetingReviewError::NothingToUndo
            | MeetingReviewError::GenerationOverflow
            | MeetingReviewError::OperationLimit
            | MeetingReviewError::SerializedSizeLimit
            | MeetingReviewError::Serialization => invalid(&error),
        }
    }
}

impl From<ReprocessingPlanError> for DictationError {
    fn from(error: ReprocessingPlanError) -> Self {
        match &error {
            ReprocessingPlanError::StaleContext => stale(&error),
            ReprocessingPlanError::UnsupportedSchema(_)
            | ReprocessingPlanError::InvalidField(_)
            | ReprocessingPlanError::RevisionMismatch
            | ReprocessingPlanError::Serialization => invalid(&error),
        }
    }
}

impl From<ProposalError> for DictationError {
    fn from(error: ProposalError) -> Self {
        match &error {
            ProposalError::RevisionMismatch | ProposalError::PreviousRevisionMismatch => {
                stale(&error)
            }
            // The proposal wraps the review it was made from: that review's own
            // classification decides, with the proposal's wording.
            ProposalError::Review(review) => match DictationError::from(review.clone()) {
                DictationError::MeetingStale(_) => stale(&error),
                _ => invalid(&error),
            },
            ProposalError::UnsupportedSchema(_)
            | ProposalError::InvalidField(_)
            | ProposalError::GenerationOverflow
            | ProposalError::SerializedSizeLimit
            | ProposalError::Serialization
            | ProposalError::UnresolvedCorrections => invalid(&error),
        }
    }
}

impl From<MediaError> for DictationError {
    fn from(error: MediaError) -> Self {
        match &error {
            MediaError::SourceMismatch | MediaError::Changed => stale(&error),
            MediaError::InvalidFile
            | MediaError::Unsupported
            | MediaError::InvalidRange
            | MediaError::Io(_) => invalid(&error),
        }
    }
}

#[cfg(test)]
#[path = "dictation_error_tests.rs"]
mod tests;
