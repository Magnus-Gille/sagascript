//! Versioned, validated documents for a live meeting transcript.
//!
//! A live document deliberately has its own contract.  It is not an alternate
//! representation of [`crate::meeting::MeetingTranscript`], whose v1/v2 wire
//! formats remain unchanged.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::meeting::MeetingExportFormat;

const LIVE_SCHEMA_VERSION: u32 = 1;
const MAX_DURATION_MS: u64 = 14_400_000;
const MAX_ITEMS: usize = 100_000;
const MAX_METADATA_BYTES: usize = 512;
const MAX_ID_CHARS: usize = 128;
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TEXT_BYTES_PER_SEGMENT: usize = 1024 * 1024;
const MAX_REASON_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Microphone,
    System,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Microphone => "Jag · mikrofon",
            Self::System => "Mötet · systemljud",
        }
    }

    fn order(self) -> u8 {
        match self {
            Self::Microphone => 0,
            Self::System => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentStatus {
    Preview,
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSegment {
    pub id: String,
    pub source: Source,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub status: SegmentStatus,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub source: Source,
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveDocument {
    pub schema_version: u32,
    pub session_id: String,
    pub language: String,
    pub model: String,
    pub duration_ms: u64,
    pub segments: Vec<LiveSegment>,
    pub gaps: Vec<Gap>,
    pub complete: bool,
    pub audio_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveDocumentWire {
    schema_version: u32,
    session_id: String,
    language: String,
    model: String,
    duration_ms: u64,
    segments: Vec<LiveSegment>,
    gaps: Vec<Gap>,
    complete: bool,
    #[serde(default)]
    audio_sha256: Option<String>,
}

impl<'de> Deserialize<'de> for LiveDocument {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = LiveDocumentWire::deserialize(deserializer)?;
        let document = Self {
            schema_version: wire.schema_version,
            session_id: wire.session_id,
            language: wire.language,
            model: wire.model,
            duration_ms: wire.duration_ms,
            segments: wire.segments,
            gaps: wire.gaps,
            complete: wire.complete,
            audio_sha256: wire.audio_sha256,
        };
        document.validate().map_err(serde::de::Error::custom)?;
        Ok(document)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveDocumentError {
    UnsupportedSchema,
    InvalidMetadata,
    InvalidId,
    DuplicateId,
    InvalidRange,
    InvalidOrdering,
    PreviewSegment,
    TooManyItems,
    TextTooLarge,
    InvalidAudioHash,
    Serialization,
}

impl LiveDocumentError {
    /// Stable machine-readable error code for callers that need to branch.
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "live_document.unsupported_schema",
            Self::InvalidMetadata => "live_document.invalid_metadata",
            Self::InvalidId => "live_document.invalid_id",
            Self::DuplicateId => "live_document.duplicate_id",
            Self::InvalidRange => "live_document.invalid_range",
            Self::InvalidOrdering => "live_document.invalid_ordering",
            Self::PreviewSegment => "live_document.preview_segment",
            Self::TooManyItems => "live_document.too_many_items",
            Self::TextTooLarge => "live_document.text_too_large",
            Self::InvalidAudioHash => "live_document.invalid_audio_hash",
            Self::Serialization => "live_document.serialization",
        }
    }
}

impl fmt::Display for LiveDocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for LiveDocumentError {}

impl LiveDocument {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_id: impl Into<String>,
        language: impl Into<String>,
        model: impl Into<String>,
        duration_ms: u64,
        segments: Vec<LiveSegment>,
        gaps: Vec<Gap>,
        complete: bool,
        audio_sha256: Option<String>,
    ) -> Result<Self, LiveDocumentError> {
        let mut document = Self {
            schema_version: LIVE_SCHEMA_VERSION,
            session_id: session_id.into(),
            language: language.into(),
            model: model.into(),
            duration_ms,
            segments,
            gaps,
            complete,
            audio_sha256,
        };
        document.sort_for_stable_output();
        document.validate()?;
        Ok(document)
    }

    pub fn from_json(json: &str) -> Result<Self, LiveDocumentError> {
        serde_json::from_str(json).map_err(|_| LiveDocumentError::Serialization)
    }

    pub fn validate(&self) -> Result<(), LiveDocumentError> {
        if self.schema_version != LIVE_SCHEMA_VERSION {
            return Err(LiveDocumentError::UnsupportedSchema);
        }
        if !valid_metadata(&self.session_id)
            || !valid_metadata(&self.language)
            || !valid_metadata(&self.model)
        {
            return Err(LiveDocumentError::InvalidMetadata);
        }
        if self.duration_ms > MAX_DURATION_MS {
            return Err(LiveDocumentError::InvalidRange);
        }
        if self.segments.len() > MAX_ITEMS || self.gaps.len() > MAX_ITEMS {
            return Err(LiveDocumentError::TooManyItems);
        }
        if let Some(hash) = &self.audio_sha256 {
            if !is_sha256(hash) {
                return Err(LiveDocumentError::InvalidAudioHash);
            }
        }

        let mut ids = BTreeSet::new();
        let mut text_bytes = 0usize;
        for (index, segment) in self.segments.iter().enumerate() {
            if !valid_id(&segment.id) {
                return Err(LiveDocumentError::InvalidId);
            }
            if !ids.insert(&segment.id) {
                return Err(LiveDocumentError::DuplicateId);
            }
            if segment.status != SegmentStatus::Final {
                return Err(LiveDocumentError::PreviewSegment);
            }
            if segment.end_ms < segment.start_ms || segment.end_ms > self.duration_ms {
                return Err(LiveDocumentError::InvalidRange);
            }
            if segment.text.len() > MAX_TEXT_BYTES_PER_SEGMENT {
                return Err(LiveDocumentError::TextTooLarge);
            }
            text_bytes = text_bytes
                .checked_add(segment.text.len())
                .ok_or(LiveDocumentError::TextTooLarge)?;
            if text_bytes > MAX_TEXT_BYTES {
                return Err(LiveDocumentError::TextTooLarge);
            }
            if index > 0
                && compare_segments(&self.segments[index - 1], segment) == Ordering::Greater
            {
                return Err(LiveDocumentError::InvalidOrdering);
            }
        }

        for (index, gap) in self.gaps.iter().enumerate() {
            if gap.end_ms < gap.start_ms || gap.end_ms > self.duration_ms {
                return Err(LiveDocumentError::InvalidRange);
            }
            if gap.reason.len() > MAX_REASON_BYTES {
                return Err(LiveDocumentError::TextTooLarge);
            }
            if index > 0 && compare_gaps(&self.gaps[index - 1], gap) == Ordering::Greater {
                return Err(LiveDocumentError::InvalidOrdering);
            }
        }
        Ok(())
    }

    pub fn export(&self, format: MeetingExportFormat) -> Result<String, LiveDocumentError> {
        self.validate()?;
        match format {
            MeetingExportFormat::Plain => Ok(self.render_plain()),
            MeetingExportFormat::Markdown => Ok(self.render_markdown()),
            MeetingExportFormat::Json => {
                serde_json::to_string(self).map_err(|_| LiveDocumentError::Serialization)
            }
            MeetingExportFormat::Srt => Ok(self.render_subtitles(false)),
            MeetingExportFormat::Vtt => Ok(self.render_subtitles(true)),
        }
    }

    fn sort_for_stable_output(&mut self) {
        self.segments.sort_by(compare_segments);
        self.gaps.sort_by(compare_gaps);
    }

    fn render_plain(&self) -> String {
        let mut lines = vec![format!(
            "Status: {}",
            if self.complete {
                "complete"
            } else {
                "INCOMPLETE"
            }
        )];
        lines.extend(self.event_lines(false));
        lines.join("\n")
    }

    fn render_markdown(&self) -> String {
        let mut lines = vec![
            "# Live meeting transcript".to_string(),
            format!(
                "> Status: {}",
                if self.complete {
                    "complete"
                } else {
                    "INCOMPLETE"
                }
            ),
        ];
        lines.extend(self.event_lines(true));
        lines.join("\n")
    }

    fn event_lines(&self, markdown: bool) -> Vec<String> {
        let mut events = Vec::with_capacity(self.segments.len() + self.gaps.len());
        for segment in &self.segments {
            events.push((
                segment.start_ms,
                segment.end_ms,
                1u8,
                segment.source.order(),
                format_segment_line(segment, markdown),
            ));
        }
        for gap in &self.gaps {
            events.push((
                gap.start_ms,
                gap.end_ms,
                0u8,
                gap.source.order(),
                format_gap_line(gap, markdown),
            ));
        }
        events.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
                .then_with(|| left.3.cmp(&right.3))
                .then_with(|| left.4.cmp(&right.4))
        });
        events.into_iter().map(|event| event.4).collect()
    }

    fn render_subtitles(&self, vtt: bool) -> String {
        let mut output = if vtt {
            "WEBVTT\n\n".to_string()
        } else {
            String::new()
        };
        let mut cues = Vec::with_capacity(self.segments.len() + self.gaps.len() + 1);
        if !self.complete {
            let end_ms = self.duration_ms.clamp(1, 1_000);
            cues.push(SubtitleCue {
                start_ms: 0,
                end_ms,
                source_order: 0,
                kind: SubtitleCueKind::Incomplete,
                text: "[INCOMPLETE TRANSCRIPT]".to_string(),
            });
        }
        for segment in &self.segments {
            cues.push(SubtitleCue {
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                source_order: segment.source.order(),
                kind: SubtitleCueKind::Segment,
                text: format!(
                    "[{}] {}",
                    segment.source.label(),
                    subtitle_text(&segment.text)
                ),
            });
        }
        for gap in &self.gaps {
            cues.push(SubtitleCue {
                start_ms: gap.start_ms,
                end_ms: gap.end_ms,
                source_order: gap.source.order(),
                kind: SubtitleCueKind::Gap,
                text: format!(
                    "[{}] [AUDIO GAP] {}",
                    gap.source.label(),
                    subtitle_text(&gap.reason)
                ),
            });
        }
        cues.sort_by(|left, right| {
            let left_status = left.kind == SubtitleCueKind::Incomplete;
            let right_status = right.kind == SubtitleCueKind::Incomplete;
            right_status
                .cmp(&left_status)
                .then_with(|| left.start_ms.cmp(&right.start_ms))
                .then_with(|| left.end_ms.cmp(&right.end_ms))
                .then_with(|| left.source_order.cmp(&right.source_order))
                .then_with(|| left.kind.priority().cmp(&right.kind.priority()))
                .then_with(|| left.text.cmp(&right.text))
        });
        for (index, cue) in cues.iter().enumerate() {
            output.push_str(&(index + 1).to_string());
            output.push('\n');
            output.push_str(&timestamp(cue.start_ms, !vtt));
            output.push_str(" --> ");
            output.push_str(&timestamp(cue.end_ms, !vtt));
            output.push('\n');
            output.push_str(&cue.text);
            output.push_str("\n\n");
        }
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubtitleCueKind {
    Incomplete,
    Gap,
    Segment,
}

impl SubtitleCueKind {
    const fn priority(self) -> u8 {
        match self {
            Self::Incomplete => 0,
            Self::Gap => 1,
            Self::Segment => 2,
        }
    }
}

struct SubtitleCue {
    start_ms: u64,
    end_ms: u64,
    source_order: u8,
    kind: SubtitleCueKind,
    text: String,
}

fn valid_metadata(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= MAX_METADATA_BYTES
        && !value.chars().any(char::is_control)
}

fn valid_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_ID_CHARS
        && !value.chars().any(char::is_control)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn compare_segments(left: &LiveSegment, right: &LiveSegment) -> Ordering {
    left.start_ms
        .cmp(&right.start_ms)
        .then_with(|| left.end_ms.cmp(&right.end_ms))
        .then_with(|| left.source.order().cmp(&right.source.order()))
        .then_with(|| compare_ids(&left.id, &right.id))
        .then_with(|| left.revision.cmp(&right.revision))
}

fn compare_gaps(left: &Gap, right: &Gap) -> Ordering {
    left.start_ms
        .cmp(&right.start_ms)
        .then_with(|| left.end_ms.cmp(&right.end_ms))
        .then_with(|| left.source.order().cmp(&right.source.order()))
        .then_with(|| left.reason.cmp(&right.reason))
}

fn compare_ids(left: &str, right: &str) -> Ordering {
    match (trailing_number(left), trailing_number(right)) {
        (Some(left_number), Some(right_number)) => {
            left_number.cmp(&right_number).then_with(|| left.cmp(right))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left.cmp(right),
    }
}

fn trailing_number(value: &str) -> Option<u64> {
    let digits = value
        .bytes()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .collect::<Vec<_>>();
    if digits.is_empty() {
        return None;
    }
    let digits = digits.into_iter().rev().collect::<Vec<_>>();
    std::str::from_utf8(&digits).ok()?.parse().ok()
}

fn format_segment_line(segment: &LiveSegment, markdown: bool) -> String {
    let start = timestamp(segment.start_ms, false);
    let end = timestamp(segment.end_ms, false);
    let label = segment.source.label();
    let text = if markdown {
        escape_markdown(&segment.text)
    } else {
        segment.text.clone()
    };
    if markdown {
        format!("- **{start} --> {end} — {label}:** {text}")
    } else {
        format!("{start} --> {end} [{label}] {text}")
    }
}

fn format_gap_line(gap: &Gap, markdown: bool) -> String {
    let start = timestamp(gap.start_ms, false);
    let end = timestamp(gap.end_ms, false);
    let label = gap.source.label();
    if markdown {
        format!(
            "- **GAP {start} --> {end} — {label}:** {}",
            escape_markdown(&gap.reason)
        )
    } else {
        format!("[GAP {start} --> {end} — {label}: {}]", gap.reason)
    }
}

fn timestamp(milliseconds: u64, srt: bool) -> String {
    let hours = milliseconds / 3_600_000;
    let minutes = (milliseconds / 60_000) % 60;
    let seconds = (milliseconds / 1_000) % 60;
    let remainder = milliseconds % 1_000;
    let separator = if srt { ',' } else { '.' };
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{remainder:03}")
}

fn subtitle_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .collect()
}

fn escape_markdown(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '*' | '_' | '`' | '[' | ']' => {
                escaped.push('\\');
                escaped.push(character);
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod document_tests;
