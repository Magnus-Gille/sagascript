//! Test fixtures made of core types, shared by the workspace's tests (#363).
//!
//! Compiled for this crate's unit tests (`cfg(test)`) and, through the
//! `test-support` feature that `sagascript-cli` and the app enable as a
//! dev-dependency, for theirs; release builds never contain it. Fixtures that
//! need no core type (temporary directories, SHA-256, the isolated CLI
//! environment) are in the `sagascript-testsupport` crate, which core's unit
//! tests can use because it does not depend on core.

use crate::meeting::{MeetingSegmentInput, MeetingSpeaker, MeetingTranscript};

/// Starts a synthetic [`MeetingTranscript`]: source hash `"a" * 64`, language
/// `"en"`, model `"tiny"`, 4 seconds, no segments and no speakers. Set what
/// the test relies on, then [`build`](MeetingTranscriptBuilder::build) it.
pub fn meeting_transcript() -> MeetingTranscriptBuilder {
    MeetingTranscriptBuilder {
        source_sha256: "a".repeat(64),
        language: "en".to_string(),
        model: "tiny".to_string(),
        duration_seconds: 4.0,
        segments: Vec::new(),
        speakers: Vec::new(),
    }
}

/// Builder returned by [`meeting_transcript`].
#[derive(Debug, Clone)]
#[must_use]
pub struct MeetingTranscriptBuilder {
    source_sha256: String,
    language: String,
    model: String,
    duration_seconds: f64,
    segments: Vec<MeetingSegmentInput>,
    speakers: Vec<MeetingSpeaker>,
}

impl MeetingTranscriptBuilder {
    pub fn source_sha256(mut self, source_sha256: impl Into<String>) -> Self {
        self.source_sha256 = source_sha256.into();
        self
    }

    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration_seconds = seconds;
        self
    }

    /// Appends a segment; [`MeetingTranscript::new`] sorts them and assigns ids.
    pub fn segment(
        mut self,
        start: f64,
        end: f64,
        text: impl Into<String>,
        speaker: impl Into<String>,
    ) -> Self {
        self.segments.push(MeetingSegmentInput {
            start,
            end,
            text: text.into(),
            speaker: speaker.into(),
        });
        self
    }

    pub fn speaker(mut self, id: impl Into<String>, label: impl Into<String>) -> Self {
        self.speakers.push(MeetingSpeaker {
            id: id.into(),
            label: label.into(),
        });
        self
    }

    /// Builds through [`MeetingTranscript::new`], so the fixture passes the
    /// same validation as real documents; panics if it does not.
    pub fn build(self) -> MeetingTranscript {
        MeetingTranscript::new(
            self.source_sha256,
            self.language,
            self.model,
            self.duration_seconds,
            self.segments,
            self.speakers,
        )
        .unwrap_or_else(|error| panic!("synthetic meeting transcript is invalid: {error}"))
    }
}
