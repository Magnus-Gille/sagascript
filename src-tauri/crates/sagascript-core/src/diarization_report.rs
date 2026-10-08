//! Versioned, local diarization evidence. Distances and temporal support are
//! diagnostic measurements, never calibrated speaker probabilities.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ITEMS: usize = 500_000;
const MAX_DURATION: f64 = 14_400.0;

/// Floor-clipped 16kHz sample counts can exceed floating-point interval length
/// by less than one sample. This bound applies only to discrete activity support.
pub const ACTIVITY_DURATION_TOLERANCE: f64 = 1.0 / 16_000.0 + 1e-9;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReportError {
    #[error("invalid diarization report {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivitySpan {
    pub start: f64,
    pub end: f64,
    /// Simultaneous acoustic activity, independent of transcript attribution.
    pub speakers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingStatus {
    Usable,
    Missing,
    Degenerate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionEvidence {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub track: usize,
    pub speaker: String,
    pub embedding_status: EmbeddingStatus,
    /// Cosine distance to the assigned output cluster's centroid.
    pub assigned_centroid_distance: Option<f64>,
    pub nearest_other_centroid_distance: Option<f64>,
    /// Missing embeddings use stitched-track fallback, not an independent voice match.
    pub used_track_fallback: bool,
    #[serde(default)]
    pub active_speech_seconds: Option<f64>,
    #[serde(default)]
    pub overlapping_speech_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoderEvidence {
    pub strategy: String,
    pub beam_size: u32,
    pub temperature_fallback: bool,
    pub vad_enabled: bool,
    pub vad_threshold: f32,
    pub vad_min_silence_duration_ms: i32,
    pub vad_speech_pad_ms: i32,
    pub vad_samples_overlap: f32,
    /// VAD uses source-mapped segments; compacted DTW is not source word timing.
    pub timestamp_method: String,
}

impl From<crate::transcription::whisper_backend::DiarizationDecoderConfig> for DecoderEvidence {
    fn from(value: crate::transcription::whisper_backend::DiarizationDecoderConfig) -> Self {
        Self {
            strategy: value.strategy.into(),
            beam_size: value.beam_size,
            temperature_fallback: value.temperature_fallback,
            vad_enabled: value.vad_enabled,
            vad_threshold: value.vad_threshold,
            vad_min_silence_duration_ms: value.vad_min_silence_duration_ms,
            vad_speech_pad_ms: value.vad_speech_pad_ms,
            vad_samples_overlap: value.vad_samples_overlap,
            timestamp_method: if value.vad_enabled {
                "source_mapped_segments"
            } else {
                "dtw_words_with_segment_fallback"
            }
            .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AsrSegmentEvidence {
    pub start: f64,
    pub end: f64,
    /// Values supplied by Whisper, unrelated to speaker identity confidence.
    pub avg_logprob: Option<f32>,
    pub no_speech_prob: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributionReason {
    TemporalOverlap,
    TiedOverlap,
    NearestGap,
    NoSpeakerEvidence,
    InvalidTimestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeakerSupport {
    pub speaker: String,
    /// Union of this speaker's activity within the word interval.
    pub overlap_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionEvidence {
    pub index: usize,
    pub start: f64,
    pub end: f64,
    pub speaker: String,
    pub reason: AttributionReason,
    pub support: Vec<SpeakerSupport>,
    /// Unnormalised duration difference between strongest and runner-up support.
    pub margin_seconds: Option<f64>,
    pub gap_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiarizationParameters {
    pub threshold: f32,
    pub min_segment_seconds: f64,
    pub min_gap_seconds: f64,
    pub min_speaker_seconds: f64,
    pub absorb_max_distance: f32,
    pub hint_merge_max_distance: f32,
    #[serde(default)]
    pub exclusive_speech_embeddings: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiarizationReport {
    pub schema_version: u32,
    pub source_sha256: String,
    pub duration_seconds: f64,
    pub build_revision: String,
    pub build_version: String,
    #[serde(default)]
    pub diagnostics_included: bool,
    #[serde(default)]
    pub speaker_hint: Option<crate::speaker_hint::SpeakerCountHint>,
    #[serde(default)]
    pub speaker_hint_outcome: Option<crate::speaker_hint::SpeakerHintOutcome>,
    #[serde(default)]
    pub segmentation_model_sha256: Option<String>,
    #[serde(default)]
    pub embedding_model_sha256: Option<String>,
    #[serde(default)]
    pub decoder: Option<DecoderEvidence>,
    #[serde(default)]
    pub asr_segments: Vec<AsrSegmentEvidence>,
    /// The acoustic timeline remains original when users edit/merge transcript labels.
    #[serde(default)]
    pub transcript_modified: bool,
    pub parameters: DiarizationParameters,
    pub activity: Vec<ActivitySpan>,
    pub regions: Vec<RegionEvidence>,
    /// Contains no duplicated transcript text or raw voice vectors.
    pub attributions: Vec<AttributionEvidence>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}

fn bounds(start: f64, end: f64, duration: f64) -> bool {
    start.is_finite() && end.is_finite() && start >= 0.0 && start <= end && end <= duration
}

impl DiarizationReport {
    pub fn validate(&self) -> Result<(), ReportError> {
        if self.schema_version != 1 {
            return Err(ReportError::Invalid("schema_version"));
        }
        if self.source_sha256.len() != 64
            || !self
                .source_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ReportError::Invalid("source_sha256"));
        }
        if !self.duration_seconds.is_finite()
            || !(0.0..=MAX_DURATION).contains(&self.duration_seconds)
        {
            return Err(ReportError::Invalid("duration_seconds"));
        }
        if !valid_id(&self.build_revision) || !valid_id(&self.build_version) {
            return Err(ReportError::Invalid("build identity"));
        }
        if let Some(hint) = self.speaker_hint {
            hint.validate()
                .map_err(|_| ReportError::Invalid("speaker hint"))?;
            if hint.is_empty() {
                return Err(ReportError::Invalid("speaker hint"));
            }
        }
        if self.speaker_hint_outcome.is_some() && self.speaker_hint.is_none() {
            return Err(ReportError::Invalid("speaker hint outcome"));
        }
        for hash in [
            &self.segmentation_model_sha256,
            &self.embedding_model_sha256,
        ]
        .into_iter()
        .flatten()
        {
            if hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(ReportError::Invalid("model identity"));
            }
        }
        if let Some(decoder) = &self.decoder {
            let expected_strategy = if decoder.beam_size >= 2 {
                "beam_search"
            } else {
                "greedy"
            };
            if decoder.beam_size == 1
                || decoder.beam_size > 8
                || decoder.strategy != expected_strategy
                || !decoder.vad_threshold.is_finite()
                || !(0.0..=1.0).contains(&decoder.vad_threshold)
                || !decoder.vad_samples_overlap.is_finite()
                || !(0.0..=1.0).contains(&decoder.vad_samples_overlap)
                || decoder.vad_min_silence_duration_ms < 0
                || decoder.vad_speech_pad_ms < 0
                || !matches!(
                    decoder.timestamp_method.as_str(),
                    "source_mapped_segments" | "dtw_words_with_segment_fallback"
                )
            {
                return Err(ReportError::Invalid("decoder evidence"));
            }
        }
        let p = &self.parameters;
        if [
            p.threshold,
            p.absorb_max_distance,
            p.hint_merge_max_distance,
        ]
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=2.0).contains(v))
            || [
                p.min_segment_seconds,
                p.min_gap_seconds,
                p.min_speaker_seconds,
            ]
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=MAX_DURATION).contains(v))
        {
            return Err(ReportError::Invalid("parameters"));
        }
        if [
            self.activity.len(),
            self.regions.len(),
            self.attributions.len(),
            self.asr_segments.len(),
        ]
        .iter()
        .any(|n| *n > MAX_ITEMS)
        {
            return Err(ReportError::Invalid("item count"));
        }
        let mut previous_end = 0.0;
        for span in &self.activity {
            let ids: BTreeSet<_> = span.speakers.iter().collect();
            if !bounds(span.start, span.end, self.duration_seconds)
                || span.start >= span.end
                || span.start < previous_end
                || ids.len() != span.speakers.len()
                || span.speakers.is_empty()
                || span.speakers.len() > 64
                || !span.speakers.iter().all(|s| valid_id(s))
            {
                return Err(ReportError::Invalid("activity"));
            }
            previous_end = span.end;
        }
        for (index, region) in self.regions.iter().enumerate() {
            if region.index != index
                || !bounds(region.start, region.end, self.duration_seconds)
                || !valid_id(&region.speaker)
                || [
                    region.assigned_centroid_distance,
                    region.nearest_other_centroid_distance,
                ]
                .into_iter()
                .flatten()
                .any(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
                || region.used_track_fallback
                    != (region.embedding_status != EmbeddingStatus::Usable)
                || [
                    region.active_speech_seconds,
                    region.overlapping_speech_seconds,
                ]
                .into_iter()
                .flatten()
                .any(|v| !v.is_finite() || v < 0.0
                    || v > region.end - region.start + ACTIVITY_DURATION_TOLERANCE)
            {
                return Err(ReportError::Invalid("region evidence"));
            }
        }
        for segment in &self.asr_segments {
            if !bounds(segment.start, segment.end, self.duration_seconds)
                || segment.avg_logprob.is_some_and(|v| !v.is_finite())
                || segment
                    .no_speech_prob
                    .is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v))
            {
                return Err(ReportError::Invalid("ASR evidence"));
            }
        }
        for (index, word) in self.attributions.iter().enumerate() {
            if word.index != index
                || !bounds(word.start, word.end, self.duration_seconds)
                || !valid_id(&word.speaker)
                || word
                    .margin_seconds
                    .is_some_and(|v| !v.is_finite() || v < 0.0)
                || word
                    .gap_seconds
                    .is_some_and(|v| !v.is_finite() || !(0.0..=MAX_DURATION).contains(&v))
                || word.support.len() > 64
                || word.support.iter().any(|s| {
                    !valid_id(&s.speaker)
                        || !s.overlap_seconds.is_finite()
                        || s.overlap_seconds < 0.0
                        || s.overlap_seconds > word.end - word.start + 1e-9
                })
            {
                return Err(ReportError::Invalid("attribution evidence"));
            }
            let ids: BTreeSet<_> = word.support.iter().map(|s| &s.speaker).collect();
            if ids.len() != word.support.len() {
                return Err(ReportError::Invalid("duplicate attribution support"));
            }
        }
        Ok(())
    }

    /// Acoustic RTTM. Text content is not duplicated for concurrent speakers.
    pub fn activity_rttm(&self) -> Result<String, ReportError> {
        self.validate()?;
        let mut output = String::new();
        for span in &self.activity {
            for speaker in &span.speakers {
                output.push_str(&format!(
                    "SPEAKER {} 1 {:.6} {:.6} <NA> <NA> {} <NA> <NA>\n",
                    self.source_sha256,
                    span.start,
                    span.end - span.start,
                    speaker
                ));
            }
        }
        Ok(output)
    }
}

/// Exact union of per-speaker activity, preserving concurrent voices.
pub fn activity_spans(regions: &[(f64, f64, String)], duration: f64) -> Vec<ActivitySpan> {
    let mut events = Vec::new();
    for (start, end, speaker) in regions {
        if bounds(*start, *end, duration) && start < end && valid_id(speaker) {
            events.push((*start, true, speaker.as_str()));
            events.push((*end, false, speaker.as_str()));
        }
    }
    events.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(right.2))
    });
    let mut output: Vec<ActivitySpan> = Vec::new();
    let mut active = BTreeMap::<&str, usize>::new();
    let mut index = 0;
    while index < events.len() {
        let start = events[index].0;
        while index < events.len() && events[index].0 == start {
            let (_, begins, speaker) = events[index];
            if begins {
                *active.entry(speaker).or_default() += 1;
            } else if let Some(count) = active.get_mut(speaker) {
                *count -= 1;
                if *count == 0 {
                    active.remove(speaker);
                }
            }
            index += 1;
        }
        let Some(next) = events.get(index) else {
            break;
        };
        let end = next.0;
        let speakers = active
            .keys()
            .map(|id| (*id).to_string())
            .collect::<Vec<_>>();
        if speakers.is_empty() {
            continue;
        }
        if let Some(last) = output.last_mut() {
            if last.end == start && last.speakers == speakers {
                last.end = end;
                continue;
            }
        }
        output.push(ActivitySpan {
            start,
            end,
            speakers,
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn simultaneous_activity_is_preserved_and_duplicates_are_unioned() {
        let regions = vec![
            (0.0, 2.0, "a".into()),
            (0.5, 1.5, "a".into()),
            (1.0, 3.0, "b".into()),
            (1.5, 2.5, "c".into()),
        ];
        let spans = activity_spans(&regions, 3.0);
        assert_eq!(spans.len(), 5);
        assert_eq!(spans[2].speakers, vec!["a", "b", "c"]);
        assert_eq!((spans[2].start, spans[2].end), (1.5, 2.0));
    }
    #[test]
    fn adjacent_turns_are_not_simultaneous() {
        let spans = activity_spans(&[(0.0, 1.0, "a".into()), (1.0, 2.0, "b".into())], 2.0);
        assert_eq!(spans.len(), 2);
        assert!(spans.iter().all(|s| s.speakers.len() == 1));
    }
    #[test]
    fn gaps_are_not_filled_with_speech() {
        let spans = activity_spans(&[(0.0, 1.0, "a".into()), (2.0, 3.0, "a".into())], 3.0);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].end, 1.0);
        assert_eq!(spans[1].start, 2.0);
    }
}
