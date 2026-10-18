use super::*;
use crate::meeting::media::MediaError;

/// Every variant of the five meeting enums, with the code it gets and the
/// text it keeps. The conversions are exhaustive matches, so a new variant
/// fails to compile before it can be missing here.
#[test]
fn every_meeting_error_keeps_its_text_and_gets_a_code() {
    let rows: Vec<(DictationError, &str, &str)> = vec![
        (
            MeetingError::InvalidField("duration").into(),
            "meeting_invalid",
            "invalid meeting duration",
        ),
        (
            MeetingError::DuplicateId("segment").into(),
            "meeting_invalid",
            "duplicate meeting segment id",
        ),
        (
            MeetingError::UnknownSpeaker.into(),
            "meeting_invalid",
            "meeting segment references an unknown speaker",
        ),
        (
            MeetingError::UnsupportedSchema(9).into(),
            "meeting_invalid",
            "unsupported meeting schema version 9",
        ),
        (
            MeetingError::Serialization.into(),
            "meeting_invalid",
            "meeting serialization failed",
        ),
        (
            MeetingReviewError::Transcript(MeetingError::Serialization).into(),
            "meeting_invalid",
            "invalid meeting transcript: meeting serialization failed",
        ),
        (
            MeetingReviewError::SourceMismatch.into(),
            "meeting_stale",
            "correction source does not match the original audio source",
        ),
        (
            MeetingReviewError::OriginalRevisionMismatch.into(),
            "meeting_stale",
            "correction original revision does not match this review",
        ),
        (
            MeetingReviewError::ExpectedRevisionMismatch.into(),
            "meeting_stale",
            "correction expected revision does not match this review",
        ),
        (
            MeetingReviewError::InvalidRevision.into(),
            "meeting_invalid",
            "review revision is invalid",
        ),
        (
            MeetingReviewError::NothingToUndo.into(),
            "meeting_invalid",
            "cannot undo a review with no correction batches",
        ),
        (
            MeetingReviewError::OperationLimit.into(),
            "meeting_invalid",
            "meeting review exceeds its operation limit",
        ),
        (
            ReprocessingPlanError::StaleContext.into(),
            "meeting_stale",
            "reprocessing plan context is stale",
        ),
        (
            ReprocessingPlanError::RevisionMismatch.into(),
            "meeting_invalid",
            "reprocessing plan revision does not match its contents",
        ),
        (
            ProposalError::RevisionMismatch.into(),
            "meeting_stale",
            "meeting reprocessing proposal revision does not match",
        ),
        (
            ProposalError::PreviousRevisionMismatch.into(),
            "meeting_stale",
            "previous meeting review revision does not match",
        ),
        (
            ProposalError::UnresolvedCorrections.into(),
            "meeting_invalid",
            "meeting reprocessing proposal has unresolved corrections",
        ),
        (
            ProposalError::Review(MeetingReviewError::ExpectedRevisionMismatch).into(),
            "meeting_stale",
            "invalid meeting review: correction expected revision does not match this review",
        ),
        (
            ProposalError::Review(MeetingReviewError::UnknownSegment).into(),
            "meeting_invalid",
            "invalid meeting review: unknown meeting segment",
        ),
        (
            MediaError::SourceMismatch.into(),
            "meeting_stale",
            "The selected audio does not match this review.",
        ),
        (
            MediaError::Changed.into(),
            "meeting_stale",
            "The recording changed; attach it again before playback.",
        ),
        (
            MediaError::InvalidFile.into(),
            "meeting_invalid",
            "Select a nonempty regular audio file no larger than 2 GiB.",
        ),
        (
            MediaError::Unsupported.into(),
            "meeting_invalid",
            "The selected audio container is unsupported.",
        ),
        (
            MediaError::InvalidRange.into(),
            "meeting_invalid",
            "The requested audio range is invalid or too large.",
        ),
        (
            MediaError::Io(std::io::Error::other("disk gone")).into(),
            "meeting_invalid",
            "Audio file I/O failed: disk gone",
        ),
    ];
    for (error, code, text) in rows {
        assert_eq!(error.code(), code, "{error:?}");
        assert_eq!(error.to_string(), text, "{error:?}");
        assert!(!error.retryable(), "{error:?}");
    }
}
