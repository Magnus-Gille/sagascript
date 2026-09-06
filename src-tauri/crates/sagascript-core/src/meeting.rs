//! Validated meeting transcripts and deterministic exports.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

const SCHEMA_VERSION: u32 = 1;
const MAX_DURATION_SECONDS: f64 = 14_400.0;
const MAX_SEGMENTS: usize = 100_000;
const MAX_SPEAKERS: usize = 64;
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_LABEL_CHARS: usize = 128;
const MAX_ID_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingError {
    InvalidField(&'static str),
    DuplicateId(&'static str),
    UnknownSpeaker,
    UnsupportedSchema(u32),
    Serialization,
}

impl fmt::Display for MeetingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(field) => write!(f, "invalid meeting {field}"),
            Self::DuplicateId(kind) => write!(f, "duplicate meeting {kind} id"),
            Self::UnknownSpeaker => f.write_str("meeting segment references an unknown speaker"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported meeting schema version {version}")
            }
            Self::Serialization => f.write_str("meeting serialization failed"),
        }
    }
}

impl std::error::Error for MeetingError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingSpeaker {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingSegment {
    pub id: String,
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeetingSegmentInput {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub speaker: String,
}

impl From<MeetingSegmentInput> for MeetingSegment {
    fn from(input: MeetingSegmentInput) -> Self {
        Self {
            id: String::new(),
            start: input.start,
            end: input.end,
            text: input.text,
            speaker: input.speaker,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingTranscript {
    pub schema_version: u32,
    pub source_sha256: String,
    pub language: String,
    pub model: String,
    pub duration_seconds: f64,
    pub segments: Vec<MeetingSegment>,
    pub speakers: Vec<MeetingSpeaker>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeetingTranscriptWire {
    schema_version: u32,
    source_sha256: String,
    language: String,
    model: String,
    duration_seconds: f64,
    segments: Vec<MeetingSegment>,
    speakers: Vec<MeetingSpeaker>,
}

impl<'de> Deserialize<'de> for MeetingTranscript {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MeetingTranscriptWire::deserialize(deserializer)?;
        let document = Self {
            schema_version: wire.schema_version,
            source_sha256: wire.source_sha256,
            language: wire.language,
            model: wire.model,
            duration_seconds: wire.duration_seconds,
            segments: wire.segments,
            speakers: wire.speakers,
        };
        document.validate().map_err(serde::de::Error::custom)?;
        Ok(document)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingExportFormat {
    Plain,
    Markdown,
    Json,
    Srt,
    Vtt,
}

impl MeetingTranscript {
    pub fn new(
        source_sha256: impl Into<String>,
        language: impl Into<String>,
        model: impl Into<String>,
        duration_seconds: f64,
        segments: Vec<MeetingSegmentInput>,
        speakers: Vec<MeetingSpeaker>,
    ) -> Result<Self, MeetingError> {
        let mut ordered: Vec<(usize, MeetingSegment)> = Vec::new();
        for (ordinal, segment) in segments.into_iter().enumerate() {
            if ordered.len() == MAX_SEGMENTS {
                return Err(MeetingError::InvalidField("segments"));
            }
            ordered.push((ordinal, segment.into()));
        }
        ordered.sort_by(|(left_ordinal, left), (right_ordinal, right)| {
            left.start
                .total_cmp(&right.start)
                .then_with(|| left.end.total_cmp(&right.end))
                .then_with(|| left_ordinal.cmp(right_ordinal))
        });
        let segments = ordered
            .into_iter()
            .enumerate()
            .map(|(index, (_, mut segment))| {
                segment.id = format!("seg-{:06}", index + 1);
                segment
            })
            .collect();
        let document = Self {
            schema_version: SCHEMA_VERSION,
            source_sha256: source_sha256.into(),
            language: language.into(),
            model: model.into(),
            duration_seconds,
            segments,
            speakers,
        };
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), MeetingError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(MeetingError::UnsupportedSchema(self.schema_version));
        }
        if !is_sha256(&self.source_sha256) {
            return Err(MeetingError::InvalidField("source_sha256"));
        }
        if !valid_metadata(&self.language) {
            return Err(MeetingError::InvalidField("language"));
        }
        if !valid_metadata(&self.model) {
            return Err(MeetingError::InvalidField("model"));
        }
        if !self.duration_seconds.is_finite()
            || !(0.0..=MAX_DURATION_SECONDS).contains(&self.duration_seconds)
        {
            return Err(MeetingError::InvalidField("duration_seconds"));
        }
        if self.segments.len() > MAX_SEGMENTS {
            return Err(MeetingError::InvalidField("segments"));
        }
        if self.speakers.len() > MAX_SPEAKERS {
            return Err(MeetingError::InvalidField("speakers"));
        }

        let mut speaker_ids = BTreeSet::new();
        for speaker in &self.speakers {
            validate_id(&speaker.id, "speaker id")?;
            if !speaker_ids.insert(&speaker.id) {
                return Err(MeetingError::DuplicateId("speaker"));
            }
            if speaker.label.trim().is_empty()
                || speaker.label.chars().count() > MAX_LABEL_CHARS
                || speaker.label.chars().any(char::is_control)
            {
                return Err(MeetingError::InvalidField("speaker label"));
            }
        }

        let mut segment_ids = BTreeSet::new();
        let mut text_bytes = 0usize;
        let mut previous_position = None;
        let mut previous_numeric_id = None;
        for segment in &self.segments {
            validate_id(&segment.id, "segment id")?;
            if !segment_ids.insert(&segment.id) {
                return Err(MeetingError::DuplicateId("segment"));
            }
            if !speaker_ids.contains(&segment.speaker) {
                return Err(MeetingError::UnknownSpeaker);
            }
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < 0.0
                || segment.end < segment.start
                || segment.end > self.duration_seconds
            {
                return Err(MeetingError::InvalidField("segment bounds"));
            }
            if let Some((previous_start, previous_end)) = previous_position {
                if (segment.start, segment.end).lt(&(previous_start, previous_end)) {
                    return Err(MeetingError::InvalidField("segment order"));
                }
            }
            if let Some(current_numeric_id) = stable_segment_number(&segment.id) {
                if let Some(previous_numeric_id) = previous_numeric_id {
                    if current_numeric_id <= previous_numeric_id {
                        return Err(MeetingError::InvalidField("segment order"));
                    }
                }
                previous_numeric_id = Some(current_numeric_id);
            }
            previous_position = Some((segment.start, segment.end));
            text_bytes = text_bytes
                .checked_add(segment.text.len())
                .ok_or(MeetingError::InvalidField("text"))?;
            if text_bytes > MAX_TEXT_BYTES {
                return Err(MeetingError::InvalidField("text"));
            }
        }
        Ok(())
    }

    pub fn rename_speaker(
        &self,
        speaker_id: &str,
        label: impl Into<String>,
    ) -> Result<Self, MeetingError> {
        self.validate()?;
        let mut next = self.clone();
        let speaker = next
            .speakers
            .iter_mut()
            .find(|speaker| speaker.id == speaker_id)
            .ok_or(MeetingError::UnknownSpeaker)?;
        speaker.label = label.into();
        next.validate()?;
        Ok(next)
    }

    pub fn merge_speakers(&self, from_id: &str, to_id: &str) -> Result<Self, MeetingError> {
        self.validate()?;
        if from_id == to_id {
            return Err(MeetingError::InvalidField("speaker merge"));
        }
        if !self.speakers.iter().any(|speaker| speaker.id == from_id)
            || !self.speakers.iter().any(|speaker| speaker.id == to_id)
        {
            return Err(MeetingError::UnknownSpeaker);
        }
        let mut next = self.clone();
        for segment in &mut next.segments {
            if segment.speaker == from_id {
                segment.speaker = to_id.to_string();
            }
        }
        next.speakers.retain(|speaker| speaker.id != from_id);
        next.validate()?;
        Ok(next)
    }

    pub fn export(&self, format: MeetingExportFormat) -> Result<String, MeetingError> {
        self.validate()?;
        match format {
            MeetingExportFormat::Plain => Ok(self.render_plain()),
            MeetingExportFormat::Markdown => Ok(self.render_markdown()),
            MeetingExportFormat::Json => {
                serde_json::to_string(self).map_err(|_| MeetingError::Serialization)
            }
            MeetingExportFormat::Srt => Ok(self.to_subtitles(false)),
            MeetingExportFormat::Vtt => Ok(self.to_subtitles(true)),
        }
    }

    pub fn to_plain(&self) -> Result<String, MeetingError> {
        self.export(MeetingExportFormat::Plain)
    }

    fn render_plain(&self) -> String {
        self.segments
            .iter()
            .map(|segment| {
                format!(
                    "{} --> {} [{}] {}",
                    format_timestamp(segment.start, false),
                    format_timestamp(segment.end, false),
                    self.label_for(&segment.speaker),
                    segment.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn to_markdown(&self) -> Result<String, MeetingError> {
        self.export(MeetingExportFormat::Markdown)
    }

    fn render_markdown(&self) -> String {
        self.segments
            .iter()
            .map(|segment| {
                format!(
                    "- **{} --> {} — {}:** {}",
                    format_timestamp(segment.start, false),
                    format_timestamp(segment.end, false),
                    escape_markdown(&self.label_for(&segment.speaker)),
                    escape_markdown(&segment.text)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn to_json(&self) -> Result<String, MeetingError> {
        self.export(MeetingExportFormat::Json)
    }

    pub fn to_srt(&self) -> Result<String, MeetingError> {
        self.export(MeetingExportFormat::Srt)
    }

    pub fn to_vtt(&self) -> Result<String, MeetingError> {
        self.export(MeetingExportFormat::Vtt)
    }

    fn label_for(&self, id: &str) -> String {
        self.speakers
            .iter()
            .find(|speaker| speaker.id == id)
            .map(|speaker| speaker.label.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn to_subtitles(&self, vtt: bool) -> String {
        let mut output = if vtt {
            "WEBVTT\n\n".to_string()
        } else {
            String::new()
        };
        for (index, segment) in self.segments.iter().enumerate() {
            output.push_str(&(index + 1).to_string());
            output.push('\n');
            output.push_str(&format_timestamp(segment.start, !vtt));
            output.push_str(" --> ");
            output.push_str(&format_timestamp(segment.end, !vtt));
            output.push('\n');
            output.push('[');
            output.push_str(&escape_subtitle(&self.label_for(&segment.speaker)));
            output.push_str("] ");
            output.push_str(&escape_subtitle(&segment.text));
            output.push_str("\n\n");
        }
        output
    }
}

fn valid_metadata(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_ID_CHARS * 4 && !value.chars().any(char::is_control)
}

fn validate_id(value: &str, field: &'static str) -> Result<(), MeetingError> {
    if value.is_empty()
        || value.chars().count() > MAX_ID_CHARS
        || value.chars().any(char::is_control)
    {
        Err(MeetingError::InvalidField(field))
    } else {
        Ok(())
    }
}

fn stable_segment_number(value: &str) -> Option<u64> {
    value
        .strip_prefix("seg-")
        .filter(|suffix| suffix.len() == 6 && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|suffix| suffix.parse().ok())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn timestamp_millis(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}

fn format_timestamp(seconds: f64, srt: bool) -> String {
    let millis = timestamp_millis(seconds);
    let hours = millis / 3_600_000;
    let minutes = (millis / 60_000) % 60;
    let seconds = (millis / 1_000) % 60;
    let remainder = millis % 1_000;
    let separator = if srt { ',' } else { '.' };
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{remainder:03}")
}

fn escape_subtitle(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    for line in normalized.split('\n') {
        let mut escaped = String::with_capacity(line.len());
        for character in line.chars() {
            if character.is_control() {
                continue;
            }
            match character {
                '&' => escaped.push_str("&amp;"),
                '<' => escaped.push_str("&lt;"),
                '>' => escaped.push_str("&gt;"),
                character => escaped.push(character),
            }
        }
        if !escaped.trim().is_empty() {
            lines.push(escaped);
        }
    }
    output.push_str(&lines.join("\n"));
    output
}

fn escape_markdown(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '\u{60}'
            || matches!(
                character,
                '\\' | '*'
                    | '_'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '#'
                    | '+'
                    | '-'
                    | '.'
                    | '!'
                    | '<'
                    | '>'
                    | '|'
            )
        {
            output.push('\\');
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speakers() -> Vec<MeetingSpeaker> {
        vec![
            MeetingSpeaker {
                id: "a".into(),
                label: "Alice".into(),
            },
            MeetingSpeaker {
                id: "b".into(),
                label: "Bob".into(),
            },
        ]
    }

    fn document() -> MeetingTranscript {
        MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            10.0,
            vec![
                MeetingSegmentInput {
                    start: 2.0,
                    end: 3.0005,
                    text: "second".into(),
                    speaker: "b".into(),
                },
                MeetingSegmentInput {
                    start: 0.0,
                    end: 1.0,
                    text: "first".into(),
                    speaker: "a".into(),
                },
            ],
            speakers(),
        )
        .expect("fixture is valid")
    }

    #[test]
    fn constructor_sorts_and_assigns_stable_ids() {
        let doc = document();
        assert_eq!(doc.segments[0].id, "seg-000001");
        assert_eq!(doc.segments[0].text, "first");
        assert_eq!(doc.segments[1].id, "seg-000002");
    }

    #[test]
    fn constructor_rejects_invalid_hash() {
        let bad = MeetingTranscript::new(
            "bad",
            "en",
            "model",
            1.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "x".into(),
                speaker: "a".into(),
            }],
            speakers(),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn constructor_rejects_out_of_bounds_segment() {
        let outside = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            1.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 2.0,
                text: "x".into(),
                speaker: "a".into(),
            }],
            speakers(),
        );
        assert!(outside.is_err());
    }

    #[test]
    fn constructor_rejects_unknown_speaker() {
        let bad = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            1.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "x".into(),
                speaker: "missing".into(),
            }],
            speakers(),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn rename_and_merge_are_immutable_and_preserve_segment_identity() {
        let doc = document();
        let renamed = doc.rename_speaker("a", "Chair").expect("known speaker");
        assert_eq!(doc.speakers[0].label, "Alice");
        assert_eq!(renamed.speakers[0].label, "Chair");
        let merged = renamed
            .merge_speakers("b", "a")
            .expect("different known speakers");
        assert_eq!(merged.speakers.len(), 1);
        assert_eq!(merged.segments[1].speaker, "a");
        assert_eq!(merged.segments[1].id, "seg-000002");
        assert!(renamed.merge_speakers("a", "a").is_err());
        assert_eq!(renamed.speakers.len(), 2);
    }

    #[test]
    fn failed_rename_does_not_mutate_original() {
        let doc = document();
        assert!(doc.rename_speaker("missing", "Chair").is_err());
        assert_eq!(doc.speakers[0].label, "Alice");
        assert!(doc.rename_speaker("a", "").is_err());
        assert_eq!(doc.speakers[0].label, "Alice");
    }

    #[test]
    fn unicode_text_is_preserved_and_invalid_mutations_fail_closed() {
        let doc = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "sv",
            "modell",
            1.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "räksmörgås 🎙️".into(),
                speaker: "a".into(),
            }],
            speakers(),
        )
        .expect("unicode is valid");
        assert!(doc.to_json().expect("json").contains("räksmörgås"));
        let mut invalid = doc.clone();
        invalid.segments[0].speaker = "unknown".into();
        assert!(invalid.export(MeetingExportFormat::Plain).is_err());
    }

    #[test]
    fn all_exports_are_deterministic_and_keep_attribution() {
        let doc = document();
        for format in [
            MeetingExportFormat::Plain,
            MeetingExportFormat::Markdown,
            MeetingExportFormat::Json,
            MeetingExportFormat::Srt,
            MeetingExportFormat::Vtt,
        ] {
            let first = doc.export(format).expect("valid export");
            assert_eq!(first, doc.export(format).expect("same export"));
            assert!(matches!(format, MeetingExportFormat::Json) || first.contains("Alice"));
        }
        assert!(doc
            .to_plain()
            .expect("plain")
            .contains("00:00:00.000 --> 00:00:01.000 [Alice] first"));
        assert!(doc.to_markdown().expect("markdown").contains("Alice"));
        assert!(doc
            .to_srt()
            .expect("srt")
            .contains("00:00:00,000 --> 00:00:01,000"));
        let vtt = doc.to_vtt().expect("vtt");
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:00.000 --> 00:00:01.000"));
    }

    #[test]
    fn subtitle_escaping_and_timestamp_rounding_are_safe() {
        let doc = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            2.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.2346,
                text: "<&>\n\n# not a cue".into(),
                speaker: "a".into(),
            }],
            speakers(),
        )
        .expect("valid");
        let srt = doc.to_srt().expect("srt");
        assert!(srt.contains("&lt;&amp;&gt;\n# not a cue"));
        assert!(srt.contains("00:00:01,235"));
        assert!(!srt.contains("\n\n# not a cue"));
        assert!(doc
            .to_markdown()
            .expect("markdown")
            .contains("\\# not a cue"));
        assert!(doc.to_markdown().expect("markdown").contains("\\<"));
    }

    #[test]
    fn subtitle_normalizes_crlf_and_removes_blank_or_control_lines() {
        let doc = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            2.0,
            vec![MeetingSegmentInput {
                start: 0.0,
                end: 1.0,
                text: "first\r\n \t\r\nsecond\rlast\u{0007}".into(),
                speaker: "a".into(),
            }],
            speakers(),
        )
        .expect("valid");
        let srt = doc.to_srt().expect("srt");
        assert!(srt.contains("first\nsecond\nlast"));
        assert!(!srt.contains("\n \t\n"));
        assert!(!srt.contains('\u{0007}'));
    }

    #[test]
    fn timestamp_format_carries_across_hours() {
        let doc = MeetingTranscript::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "en",
            "model",
            3_700.0,
            vec![MeetingSegmentInput {
                start: 3_599.999_6,
                end: 3_660.001,
                text: "hour carry".into(),
                speaker: "a".into(),
            }],
            speakers(),
        )
        .expect("valid");
        let plain = doc.to_plain().expect("plain");
        assert!(plain.contains("01:00:00.000 --> 01:01:00.001"));
    }

    #[test]
    fn validation_rejects_whitespace_labels_and_noncanonical_order() {
        let mut whitespace = document();
        whitespace.speakers[0].label = " \t\n".into();
        assert!(whitespace.validate().is_err());

        let mut unsorted = document();
        unsorted.segments.swap(0, 1);
        assert!(unsorted.validate().is_err());

        let mut ids = document();
        ids.segments[0].id = "seg-000002".into();
        ids.segments[1].id = "seg-000001".into();
        assert!(ids.validate().is_err());
    }

    #[test]
    fn merge_rejects_empty_or_unknown_speaker_ids() {
        let doc = document();
        assert!(doc.merge_speakers("", "a").is_err());
        assert!(doc.merge_speakers("missing", "a").is_err());
        assert!(doc.merge_speakers("a", "missing").is_err());
    }

    #[test]
    fn serde_validates_and_rejects_unknown_fields_or_schema() {
        let json = serde_json::to_string(&document()).expect("serialize");
        let roundtrip: MeetingTranscript = serde_json::from_str(&json).expect("valid json");
        assert_eq!(roundtrip, document());
        let unknown = format!("{}{},\"extra\":1}}", &json[..json.len() - 1], "");
        assert!(serde_json::from_str::<MeetingTranscript>(&unknown).is_err());
        let unsupported = json.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        assert!(serde_json::from_str::<MeetingTranscript>(&unsupported).is_err());
    }

    #[test]
    fn serde_rejects_duplicate_ids_speakers_bounds_and_unsorted_segments() {
        let json = serde_json::to_value(document()).expect("serialize");

        let mut duplicate_segment = json.clone();
        let first_id = duplicate_segment["segments"][0]["id"].clone();
        duplicate_segment["segments"][1]["id"] = first_id;
        assert!(serde_json::from_value::<MeetingTranscript>(duplicate_segment).is_err());

        let mut duplicate_speaker = json.clone();
        duplicate_speaker["speakers"][1]["id"] = serde_json::json!("a");
        assert!(serde_json::from_value::<MeetingTranscript>(duplicate_speaker).is_err());

        let mut invalid_bounds = json.clone();
        invalid_bounds["segments"][0]["end"] = serde_json::json!(11.0);
        assert!(serde_json::from_value::<MeetingTranscript>(invalid_bounds).is_err());

        let mut unsorted = json.clone();
        let first = unsorted["segments"][0].clone();
        let second = unsorted["segments"][1].clone();
        unsorted["segments"][0] = second;
        unsorted["segments"][1] = first;
        assert!(serde_json::from_value::<MeetingTranscript>(unsorted).is_err());

        let nonfinite = serde_json::from_str::<MeetingTranscript>(
            &serde_json::to_string(&json).expect("serialize"),
        );
        assert!(nonfinite.is_ok());
        let nonfinite_json = serde_json::to_string(&json).expect("serialize").replacen(
            "\"start\":0.0",
            "\"start\":1e999",
            1,
        );
        assert!(serde_json::from_str::<MeetingTranscript>(&nonfinite_json).is_err());
    }
}
