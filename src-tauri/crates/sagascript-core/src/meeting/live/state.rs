//! Bounded state for one live meeting session.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::document::{Gap, LiveDocument, LiveDocumentError, LiveSegment, SegmentStatus, Source};

const MAX_DURATION_MS: u64 = 14_400_000;
const MAX_SEGMENTS: usize = 20_000;
const MAX_GAPS: usize = 1_000;
const MAX_TOTAL_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEGMENT_TEXT_BYTES: usize = 1024 * 1024;
const MAX_ID_CHARS: usize = 128;
const MAX_ERROR_BYTES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Starting,
    Running,
    Draining,
    Refining,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefinementStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refinement {
    pub status: RefinementStatus,
    pub progress_percent: u8,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceStatus {
    pub source: Source,
    pub connected: bool,
    pub signal_level: f32,
    pub last_audio_ms: Option<u64>,
    pub last_text_ms: Option<u64>,
    /// End of the recognized audio to availability of its latest text, excluding UI polling.
    pub text_delay_ms: Option<u64>,
    pub queued_ms: u64,
    pub dropped_frames: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub id: String,
    pub revision: u64,
    pub phase: Phase,
    #[serde(default)]
    pub refinement: Option<Refinement>,
    pub elapsed_ms: u64,
    pub sources: Vec<SourceStatus>,
    pub segments: Vec<LiveSegment>,
    pub gaps: Vec<Gap>,
    pub error: Option<String>,
    pub audio_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    InvalidTransition,
    InvalidId,
    IdCollision,
    InvalidTime,
    InvalidText,
    StaleSegment,
    FinalSegment,
    Capacity,
    TextTooLarge,
    InvalidGap,
    InvalidHealth,
    InvalidError,
    Document(LiveDocumentError),
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTransition => "live_state.invalid_transition",
            Self::InvalidId => "live_state.invalid_id",
            Self::IdCollision => "live_state.id_collision",
            Self::InvalidTime => "live_state.invalid_time",
            Self::InvalidText => "live_state.invalid_text",
            Self::StaleSegment => "live_state.stale_segment",
            Self::FinalSegment => "live_state.final_segment",
            Self::Capacity => "live_state.capacity",
            Self::TextTooLarge => "live_state.text_too_large",
            Self::InvalidGap => "live_state.invalid_gap",
            Self::InvalidHealth => "live_state.invalid_health",
            Self::InvalidError => "live_state.invalid_error",
            Self::Document(error) => error.code(),
        })
    }
}

impl std::error::Error for StateError {}

pub struct State {
    id: String,
    revision: u64,
    phase: Phase,
    refinement: Option<Refinement>,
    elapsed_ms: u64,
    sources: Vec<SourceStatus>,
    segments: BTreeMap<(u8, String), LiveSegment>,
    gaps: Vec<Gap>,
    text_bytes: usize,
    error: Option<String>,
    audio_available: bool,
}

impl State {
    pub fn new(id: String) -> Result<Self, StateError> {
        if !valid_id(&id) {
            return Err(StateError::InvalidId);
        }
        Ok(Self {
            id,
            revision: 0,
            phase: Phase::Starting,
            refinement: None,
            elapsed_ms: 0,
            sources: vec![
                source_status(Source::Microphone),
                source_status(Source::System),
            ],
            segments: BTreeMap::new(),
            gaps: Vec::new(),
            text_bytes: 0,
            error: None,
            audio_available: false,
        })
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    pub fn source_status(&self, source: Source) -> SourceStatus {
        self.sources[source_rank(source) as usize].clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut segments: Vec<_> = self.segments.values().cloned().collect();
        segments.sort_by(compare_segments);
        let mut gaps = self.gaps.clone();
        gaps.sort_by(compare_gaps);
        Snapshot {
            id: self.id.clone(),
            revision: self.revision,
            phase: self.phase,
            refinement: self.refinement.clone(),
            elapsed_ms: self.elapsed_ms,
            sources: self.sources.clone(),
            segments,
            gaps,
            error: self.error.clone(),
            audio_available: self.audio_available,
        }
    }

    pub fn started(&mut self) -> Result<(), StateError> {
        if self.phase != Phase::Starting {
            return Err(StateError::InvalidTransition);
        }
        self.phase = Phase::Running;
        self.bump_revision();
        Ok(())
    }

    pub fn begin_stop(&mut self) -> Result<(), StateError> {
        if self.phase != Phase::Running {
            return Err(StateError::InvalidTransition);
        }
        self.phase = Phase::Draining;
        self.bump_revision();
        Ok(())
    }

    pub fn finish(&mut self, has_audio: bool) -> Result<(), StateError> {
        if self.phase != Phase::Draining {
            return Err(StateError::InvalidTransition);
        }
        self.phase = Phase::Completed;
        self.audio_available = has_audio;
        self.bump_revision();
        Ok(())
    }

    pub fn begin_refinement(&mut self, has_audio: bool) -> Result<(), StateError> {
        if self.phase != Phase::Draining {
            return Err(StateError::InvalidTransition);
        }
        self.phase = Phase::Refining;
        self.refinement = Some(Refinement {
            status: RefinementStatus::Running,
            progress_percent: 0,
            error: None,
        });
        self.audio_available = has_audio;
        self.bump_revision();
        Ok(())
    }

    pub fn set_refinement_progress(&mut self, progress_percent: u8) -> Result<(), StateError> {
        if self.phase != Phase::Refining {
            return Err(StateError::InvalidTransition);
        }
        if progress_percent > 100 {
            return Err(StateError::InvalidHealth);
        }
        let refinement = self
            .refinement
            .as_mut()
            .ok_or(StateError::InvalidTransition)?;
        if progress_percent < refinement.progress_percent {
            return Err(StateError::InvalidHealth);
        }
        refinement.progress_percent = progress_percent;
        self.bump_revision();
        Ok(())
    }

    pub fn complete_refinement(&mut self, segments: Vec<LiveSegment>) -> Result<(), StateError> {
        if self.phase != Phase::Refining {
            return Err(StateError::InvalidTransition);
        }
        let replacement = validate_refinement_segments(segments)?;
        self.segments = replacement;
        self.text_bytes = self
            .segments
            .values()
            .map(|segment| segment.text.len())
            .sum();
        self.phase = Phase::Completed;
        self.refinement = Some(Refinement {
            status: RefinementStatus::Completed,
            progress_percent: 100,
            error: None,
        });
        self.bump_revision();
        Ok(())
    }

    pub fn finish_refinement_fallback(
        &mut self,
        cancelled: bool,
        message: Option<String>,
    ) -> Result<(), StateError> {
        if self.phase != Phase::Refining {
            return Err(StateError::InvalidTransition);
        }
        if let Some(message) = &message {
            validate_error(message)?;
        }
        self.phase = Phase::Completed;
        self.error = message.clone();
        self.refinement = Some(Refinement {
            status: if cancelled {
                RefinementStatus::Cancelled
            } else {
                RefinementStatus::Failed
            },
            progress_percent: self
                .refinement
                .as_ref()
                .map(|refinement| refinement.progress_percent)
                .unwrap_or(0),
            error: message,
        });
        self.bump_revision();
        Ok(())
    }

    pub fn fail(&mut self, message: impl Into<String>) -> Result<(), StateError> {
        self.finish_failed(message, false)
    }

    pub fn finish_failed(
        &mut self,
        message: impl Into<String>,
        has_audio: bool,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        let message = message.into();
        validate_error(&message)?;
        self.phase = Phase::Failed;
        self.error = Some(message);
        self.audio_available = has_audio;
        self.bump_revision();
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), StateError> {
        self.ensure_mutable()?;
        self.phase = Phase::Cancelled;
        self.bump_revision();
        Ok(())
    }

    /// Advance elapsed time by a bounded number of milliseconds.
    pub fn advance_by(&mut self, elapsed_ms: u64) -> Result<(), StateError> {
        self.ensure_mutable()?;
        let next = self
            .elapsed_ms
            .checked_add(elapsed_ms)
            .ok_or(StateError::InvalidTime)?;
        if next > MAX_DURATION_MS {
            return Err(StateError::InvalidTime);
        }
        self.elapsed_ms = next;
        self.bump_revision();
        Ok(())
    }

    /// Compatibility alias for [`Self::advance_by`]; the argument is additive.
    pub fn advance(&mut self, elapsed_ms: u64) -> Result<(), StateError> {
        self.advance_by(elapsed_ms)
    }

    pub fn upsert(&mut self, segment: LiveSegment) -> Result<(), StateError> {
        self.ensure_mutable()?;
        validate_segment(&segment)?;
        let key = (source_rank(segment.source), segment.id.clone());
        if let Some(existing) = self.segments.get(&key) {
            if existing.status == SegmentStatus::Final {
                return Err(StateError::FinalSegment);
            }
            if segment.revision <= existing.revision {
                return Err(StateError::StaleSegment);
            }
        } else if self.segments.len() >= MAX_SEGMENTS {
            return Err(StateError::Capacity);
        }
        // IDs are unique across the two source lanes; avoid scanning the
        // entire transcript on every preview and final result.
        if self.segments.contains_key(&(1 - key.0, key.1.clone())) {
            return Err(StateError::IdCollision);
        }

        let old_bytes = self
            .segments
            .get(&key)
            .map(|existing| existing.text.len())
            .unwrap_or(0);
        let next_bytes = self
            .text_bytes
            .checked_sub(old_bytes)
            .and_then(|bytes| bytes.checked_add(segment.text.len()))
            .ok_or(StateError::TextTooLarge)?;
        if next_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(StateError::TextTooLarge);
        }
        self.text_bytes = next_bytes;
        self.segments.insert(key, segment);
        self.bump_revision();
        Ok(())
    }

    pub fn append_gap(&mut self, gap: Gap) -> Result<(), StateError> {
        self.ensure_mutable()?;
        validate_gap(&gap)?;
        if let Some(previous) = self.gaps.last_mut() {
            if previous.source == gap.source
                && previous.reason == gap.reason
                && previous.end_ms == gap.start_ms
            {
                previous.end_ms = gap.end_ms;
                self.bump_revision();
                return Ok(());
            }
        }
        if self.gaps.len() >= MAX_GAPS {
            return Err(StateError::Capacity);
        }
        self.gaps.push(gap);
        self.bump_revision();
        Ok(())
    }

    pub fn set_source_connected(
        &mut self,
        source: Source,
        connected: bool,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        self.source_status_mut(source).connected = connected;
        self.bump_revision();
        Ok(())
    }

    pub fn set_source_signal_level(
        &mut self,
        source: Source,
        signal_level: f32,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        if !signal_level.is_finite() || !(0.0..=1.0).contains(&signal_level) {
            return Err(StateError::InvalidHealth);
        }
        self.source_status_mut(source).signal_level = signal_level;
        self.bump_revision();
        Ok(())
    }

    pub fn set_source_last_audio_ms(
        &mut self,
        source: Source,
        timestamp_ms: Option<u64>,
    ) -> Result<(), StateError> {
        self.set_source_timestamp(source, timestamp_ms, true)
    }

    pub fn set_source_last_text_ms(
        &mut self,
        source: Source,
        timestamp_ms: Option<u64>,
    ) -> Result<(), StateError> {
        self.set_source_timestamp(source, timestamp_ms, false)
    }

    pub fn set_source_text_delay_ms(
        &mut self,
        source: Source,
        delay_ms: u64,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        if delay_ms > MAX_DURATION_MS {
            return Err(StateError::InvalidHealth);
        }
        self.source_status_mut(source).text_delay_ms = Some(delay_ms);
        self.bump_revision();
        Ok(())
    }

    pub fn set_source_queued_ms(
        &mut self,
        source: Source,
        queued_ms: u64,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        if queued_ms > MAX_DURATION_MS {
            return Err(StateError::InvalidHealth);
        }
        self.source_status_mut(source).queued_ms = queued_ms;
        self.bump_revision();
        Ok(())
    }

    pub fn add_source_dropped_frames(
        &mut self,
        source: Source,
        dropped_frames: u64,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        let status = self.source_status_mut(source);
        status.dropped_frames = status
            .dropped_frames
            .checked_add(dropped_frames)
            .ok_or(StateError::InvalidHealth)?;
        self.bump_revision();
        Ok(())
    }

    pub fn set_source_error(
        &mut self,
        source: Source,
        error: Option<String>,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        if let Some(value) = &error {
            validate_error(value)?;
        }
        self.source_status_mut(source).error = error;
        self.bump_revision();
        Ok(())
    }

    pub fn export_document(
        &self,
        language: impl Into<String>,
        model: impl Into<String>,
        audio_sha256: Option<String>,
    ) -> Result<LiveDocument, StateError> {
        if !matches!(
            self.phase,
            Phase::Completed | Phase::Cancelled | Phase::Failed
        ) {
            return Err(StateError::InvalidTransition);
        }
        let segments = self
            .segments
            .values()
            .filter(|segment| segment.status == SegmentStatus::Final)
            .cloned()
            .collect();
        let preview_dropped = self
            .segments
            .values()
            .any(|segment| segment.status == SegmentStatus::Preview);
        LiveDocument::new(
            self.id.clone(),
            language,
            model,
            self.elapsed_ms,
            segments,
            self.gaps.clone(),
            self.phase == Phase::Completed && self.gaps.is_empty() && !preview_dropped,
            audio_sha256,
        )
        .map_err(StateError::Document)
    }

    fn set_source_timestamp(
        &mut self,
        source: Source,
        timestamp_ms: Option<u64>,
        audio: bool,
    ) -> Result<(), StateError> {
        self.ensure_mutable()?;
        if timestamp_ms
            .is_some_and(|timestamp| timestamp > MAX_DURATION_MS || timestamp > self.elapsed_ms)
        {
            return Err(StateError::InvalidTime);
        }
        let status = self.source_status_mut(source);
        if audio {
            status.last_audio_ms = timestamp_ms;
        } else {
            status.last_text_ms = timestamp_ms;
        }
        self.bump_revision();
        Ok(())
    }

    fn source_status_mut(&mut self, source: Source) -> &mut SourceStatus {
        &mut self.sources[source_rank(source) as usize]
    }

    fn ensure_mutable(&self) -> Result<(), StateError> {
        if matches!(
            self.phase,
            Phase::Refining | Phase::Completed | Phase::Cancelled | Phase::Failed
        ) {
            Err(StateError::InvalidTransition)
        } else {
            Ok(())
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

fn source_status(source: Source) -> SourceStatus {
    SourceStatus {
        source,
        connected: false,
        signal_level: 0.0,
        last_audio_ms: None,
        last_text_ms: None,
        text_delay_ms: None,
        queued_ms: 0,
        dropped_frames: 0,
        error: None,
    }
}

fn source_rank(source: Source) -> u8 {
    match source {
        Source::Microphone => 0,
        Source::System => 1,
    }
}

fn validate_refinement_segments(
    segments: Vec<LiveSegment>,
) -> Result<BTreeMap<(u8, String), LiveSegment>, StateError> {
    if segments.len() > MAX_SEGMENTS {
        return Err(StateError::Capacity);
    }
    let mut replacement = BTreeMap::new();
    let mut text_bytes = 0usize;
    for segment in segments {
        validate_segment(&segment)?;
        if segment.status != SegmentStatus::Final {
            return Err(StateError::FinalSegment);
        }
        text_bytes = text_bytes
            .checked_add(segment.text.len())
            .ok_or(StateError::TextTooLarge)?;
        if text_bytes > MAX_TOTAL_TEXT_BYTES {
            return Err(StateError::TextTooLarge);
        }
        let key = (source_rank(segment.source), segment.id.clone());
        if replacement.contains_key(&key) || replacement.contains_key(&(1 - key.0, key.1.clone())) {
            return Err(StateError::IdCollision);
        }
        replacement.insert(key, segment);
    }
    Ok(replacement)
}

fn validate_segment(segment: &LiveSegment) -> Result<(), StateError> {
    if segment.id.trim().is_empty()
        || segment.id.chars().count() > MAX_ID_CHARS
        || segment.id.chars().any(char::is_control)
    {
        return Err(StateError::InvalidId);
    }
    if segment.end_ms < segment.start_ms || segment.end_ms > MAX_DURATION_MS {
        return Err(StateError::InvalidTime);
    }
    if segment.text.len() > MAX_SEGMENT_TEXT_BYTES {
        return Err(StateError::TextTooLarge);
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_ID_CHARS
        && !value.chars().any(char::is_control)
}

fn validate_gap(gap: &Gap) -> Result<(), StateError> {
    if gap.end_ms < gap.start_ms || gap.end_ms > MAX_DURATION_MS {
        return Err(StateError::InvalidGap);
    }
    if gap.reason.trim().is_empty() || gap.reason.chars().any(char::is_control) {
        return Err(StateError::InvalidGap);
    }
    if gap.reason.len() > MAX_SEGMENT_TEXT_BYTES {
        return Err(StateError::TextTooLarge);
    }
    Ok(())
}

fn validate_error(error: &str) -> Result<(), StateError> {
    if error.trim().is_empty()
        || error.len() > MAX_ERROR_BYTES
        || error.chars().any(char::is_control)
    {
        Err(StateError::InvalidError)
    } else {
        Ok(())
    }
}

fn compare_segments(left: &LiveSegment, right: &LiveSegment) -> Ordering {
    left.start_ms
        .cmp(&right.start_ms)
        .then_with(|| left.end_ms.cmp(&right.end_ms))
        .then_with(|| source_rank(left.source).cmp(&source_rank(right.source)))
        .then_with(|| left.id.cmp(&right.id))
        .then_with(|| left.revision.cmp(&right.revision))
}

fn compare_gaps(left: &Gap, right: &Gap) -> Ordering {
    left.start_ms
        .cmp(&right.start_ms)
        .then_with(|| left.end_ms.cmp(&right.end_ms))
        .then_with(|| source_rank(left.source).cmp(&source_rank(right.source)))
        .then_with(|| left.reason.cmp(&right.reason))
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;
