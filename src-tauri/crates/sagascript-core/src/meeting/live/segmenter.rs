//! Deterministic PCM windowing for an incremental live meeting stream.
//!
//! This module only applies caller-supplied speech flags.  It does not perform
//! VAD, inference, persistence, timing, or queue management.

use std::collections::VecDeque;

use super::document::Source;

const REQUIRED_SAMPLE_RATE: u32 = 16_000;
const MAX_SEGMENT_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub sample_rate: u32,
    pub pre_roll_ms: u64,
    pub min_speech_ms: u64,
    pub silence_ms: u64,
    pub max_segment_ms: u64,
    pub preview_interval_ms: u64,
    pub overlap_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: REQUIRED_SAMPLE_RATE,
            pre_roll_ms: 200,
            min_speech_ms: 250,
            silence_ms: 600,
            max_segment_ms: MAX_SEGMENT_MS,
            preview_interval_ms: 1_000,
            overlap_ms: 250,
        }
    }
}

impl Config {
    pub fn validate(self) -> Result<(), SegmentError> {
        if self.sample_rate != REQUIRED_SAMPLE_RATE
            || self.max_segment_ms == 0
            || self.max_segment_ms > MAX_SEGMENT_MS
            || self.pre_roll_ms > self.max_segment_ms
            || self.min_speech_ms == 0
            || self.min_speech_ms > self.max_segment_ms
            || self.silence_ms == 0
            || self.silence_ms > self.max_segment_ms
            || self.preview_interval_ms == 0
            || self.preview_interval_ms > self.max_segment_ms
            || self.overlap_ms >= self.max_segment_ms
        {
            return Err(SegmentError::InvalidConfig);
        }
        Ok(())
    }

    fn samples_for(self, milliseconds: u64) -> usize {
        ((milliseconds * u64::from(self.sample_rate)) / 1_000) as usize
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowJob {
    pub id: u64,
    pub source: Source,
    pub start_sample: u64,
    pub end_sample: u64,
    pub pcm: Vec<f32>,
    pub final_window: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentError {
    InvalidConfig,
    NonFinitePcm,
    DiscontinuousInput,
    SampleOverflow,
    IdExhausted,
}

impl SegmentError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfig => "live_segmenter.invalid_config",
            Self::NonFinitePcm => "live_segmenter.non_finite_pcm",
            Self::DiscontinuousInput => "live_segmenter.discontinuous_input",
            Self::SampleOverflow => "live_segmenter.sample_overflow",
            Self::IdExhausted => "live_segmenter.id_exhausted",
        }
    }
}

impl std::fmt::Display for SegmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for SegmentError {}

#[derive(Debug, Clone)]
struct PendingPhrase {
    id: u64,
    start_sample: u64,
    pcm: Vec<f32>,
    speech_samples: u64,
    has_new_speech: bool,
    is_overlap_continuation: bool,
    silence_samples: u64,
    next_preview_speech: u64,
    revision: u64,
}

pub struct Segmenter {
    config: Config,
    source: Source,
    next_sample: Option<u64>,
    next_id: u64,
    history_start: Option<u64>,
    history: VecDeque<f32>,
    pending: Option<PendingPhrase>,
}

impl Segmenter {
    pub fn new(config: Config, source: Source) -> Result<Self, SegmentError> {
        config.validate()?;
        Ok(Self {
            config,
            source,
            next_sample: None,
            next_id: 0,
            history_start: None,
            history: VecDeque::new(),
            pending: None,
        })
    }

    pub fn push(
        &mut self,
        start_sample: u64,
        samples: &[f32],
        speech: bool,
    ) -> Result<Vec<WindowJob>, SegmentError> {
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(SegmentError::NonFinitePcm);
        }
        let end_sample = start_sample
            .checked_add(samples.len() as u64)
            .ok_or(SegmentError::SampleOverflow)?;
        if let Some(expected) = self.next_sample {
            if expected != start_sample {
                return Err(SegmentError::DiscontinuousInput);
            }
        }
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        self.next_sample = Some(end_sample);
        let mut output = Vec::new();
        let mut cursor = 0;
        while cursor < samples.len() {
            if self.pending.is_none() {
                let current_sample = start_sample + cursor as u64;
                if speech {
                    self.begin_phrase(current_sample)?;
                } else {
                    self.append_history(current_sample, &samples[cursor..]);
                    break;
                }
            }

            let current_sample = start_sample + cursor as u64;
            let consumed = if speech {
                self.process_speech(current_sample, &samples[cursor..], &mut output)?
            } else {
                self.process_silence(current_sample, &samples[cursor..], &mut output)?
            };
            cursor += consumed;
            if consumed == 0 {
                return Err(SegmentError::SampleOverflow);
            }
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Vec<WindowJob> {
        let Some(pending) = self.pending.take() else {
            self.history.clear();
            self.history_start = None;
            return Vec::new();
        };
        self.history.clear();
        self.history_start = None;
        if pending.speech_samples == 0
            || (pending.is_overlap_continuation && !pending.has_new_speech)
        {
            Vec::new()
        } else {
            vec![final_job(self.source, pending)]
        }
    }

    pub fn abort(&mut self) {
        self.next_sample = None;
        self.history_start = None;
        self.history.clear();
        self.pending = None;
    }

    fn begin_phrase(&mut self, current_sample: u64) -> Result<(), SegmentError> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SegmentError::IdExhausted)?;
        let history_len = self.history.len() as u64;
        let start_sample = self.history_start.unwrap_or(current_sample);
        let mut pcm = Vec::with_capacity(self.config.samples_for(self.config.max_segment_ms));
        pcm.extend(self.history.iter().copied());
        self.history.clear();
        self.history_start = None;
        self.pending = Some(PendingPhrase {
            id: self.next_id,
            start_sample: current_sample.saturating_sub(history_len).max(start_sample),
            pcm,
            speech_samples: 0,
            has_new_speech: false,
            is_overlap_continuation: false,
            silence_samples: 0,
            next_preview_speech: self.config.samples_for(self.config.min_speech_ms) as u64,
            revision: 0,
        });
        Ok(())
    }

    fn process_speech(
        &mut self,
        current_sample: u64,
        samples: &[f32],
        output: &mut Vec<WindowJob>,
    ) -> Result<usize, SegmentError> {
        let mut cursor = 0;
        while cursor < samples.len() {
            let Some(pending) = self.pending.as_mut() else {
                self.begin_phrase(current_sample + cursor as u64)?;
                continue;
            };
            let max_samples = self.config.samples_for(self.config.max_segment_ms);
            let capacity = max_samples.saturating_sub(pending.pcm.len());
            if capacity == 0 {
                self.force_max(output, true);
                continue;
            }
            let preview_remaining = pending
                .next_preview_speech
                .saturating_sub(pending.speech_samples)
                as usize;
            let mut take = (samples.len() - cursor).min(capacity);
            if preview_remaining > 0 {
                take = take.min(preview_remaining);
            }
            pending
                .pcm
                .extend_from_slice(&samples[cursor..cursor + take]);
            pending.speech_samples += take as u64;
            pending.has_new_speech = true;
            pending.silence_samples = 0;
            cursor += take;

            if pending.speech_samples >= pending.next_preview_speech {
                pending.revision = pending.revision.saturating_add(1);
                output.push(preview_job(self.source, pending));
                pending.next_preview_speech =
                    pending.next_preview_speech.saturating_add(
                        self.config.samples_for(self.config.preview_interval_ms) as u64,
                    );
            }
            if pending.pcm.len() == max_samples {
                self.force_max(output, true);
            }
        }
        Ok(cursor)
    }

    fn process_silence(
        &mut self,
        current_sample: u64,
        samples: &[f32],
        output: &mut Vec<WindowJob>,
    ) -> Result<usize, SegmentError> {
        let mut cursor = 0;
        while cursor < samples.len() {
            let Some(pending) = self.pending.as_mut() else {
                self.append_history(current_sample + cursor as u64, &samples[cursor..]);
                return Ok(samples.len());
            };
            let max_samples = self.config.samples_for(self.config.max_segment_ms);
            let capacity = max_samples.saturating_sub(pending.pcm.len());
            let silence_limit = self.config.samples_for(self.config.silence_ms) as u64;
            let silence_remaining = silence_limit.saturating_sub(pending.silence_samples) as usize;
            let take = (samples.len() - cursor)
                .min(capacity)
                .min(silence_remaining);
            if take == 0 {
                if pending.pcm.len() == max_samples {
                    self.force_max(output, false);
                    continue;
                }
                self.finish_after_silence(output);
                continue;
            }
            pending
                .pcm
                .extend_from_slice(&samples[cursor..cursor + take]);
            pending.silence_samples += take as u64;
            cursor += take;
            if pending.pcm.len() == max_samples {
                self.force_max(output, false);
            } else if pending.silence_samples == silence_limit {
                self.finish_after_silence(output);
            }
        }
        Ok(cursor)
    }

    fn finish_after_silence(&mut self, output: &mut Vec<WindowJob>) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        if (!pending.is_overlap_continuation || pending.has_new_speech)
            && pending.speech_samples >= self.config.samples_for(self.config.min_speech_ms) as u64
        {
            output.push(final_job(self.source, pending));
        }
    }

    fn force_max(&mut self, output: &mut Vec<WindowJob>, preserve_overlap: bool) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let overlap = self.config.samples_for(self.config.overlap_ms);
        let tail_start = pending.pcm.len().saturating_sub(overlap);
        let next_start = pending.start_sample + tail_start as u64;
        let tail = pending.pcm[tail_start..].to_vec();
        if (!pending.is_overlap_continuation || pending.has_new_speech)
            && pending.speech_samples >= self.config.samples_for(self.config.min_speech_ms) as u64
        {
            output.push(final_job(self.source, pending));
        }
        if preserve_overlap {
            let min_speech = self.config.samples_for(self.config.min_speech_ms) as u64;
            let preview_interval = self.config.samples_for(self.config.preview_interval_ms) as u64;
            let next_preview_speech = if overlap as u64 >= min_speech {
                (overlap as u64).saturating_add(preview_interval)
            } else {
                min_speech
            };
            self.pending = Some(PendingPhrase {
                id: self.next_id_after_phrase(),
                start_sample: next_start,
                pcm: tail,
                speech_samples: overlap as u64,
                has_new_speech: false,
                is_overlap_continuation: true,
                silence_samples: 0,
                next_preview_speech,
                revision: 0,
            });
        }
    }

    fn next_id_after_phrase(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    fn append_history(&mut self, start_sample: u64, samples: &[f32]) {
        let limit = self.config.samples_for(self.config.pre_roll_ms);
        if limit == 0 {
            self.history.clear();
            self.history_start = None;
            return;
        }
        if samples.len() >= limit {
            let skip = samples.len() - limit;
            self.history.clear();
            self.history_start = Some(start_sample + skip as u64);
            self.history.extend(samples[skip..].iter().copied());
            return;
        }
        if self.history.is_empty() {
            self.history_start = Some(start_sample);
        } else {
            match self.history_start {
                Some(history_start)
                    if history_start.saturating_add(self.history.len() as u64) == start_sample => {}
                _ => {
                    self.history.clear();
                    self.history_start = Some(start_sample);
                }
            }
        }
        self.history.extend(samples.iter().copied());
        while self.history.len() > limit {
            self.history.pop_front();
            self.history_start = self.history_start.map(|sample| sample + 1);
        }
    }

    #[cfg(test)]
    fn buffered_samples(&self) -> usize {
        self.history.len() + self.pending.as_ref().map_or(0, |pending| pending.pcm.len())
    }
}

fn preview_job(source: Source, pending: &PendingPhrase) -> WindowJob {
    WindowJob {
        id: pending.id,
        source,
        start_sample: pending.start_sample,
        end_sample: pending.start_sample + pending.pcm.len() as u64,
        pcm: pending.pcm.clone(),
        final_window: false,
        revision: pending.revision,
    }
}

fn final_job(source: Source, pending: PendingPhrase) -> WindowJob {
    let end_sample = pending.start_sample + pending.pcm.len() as u64;
    WindowJob {
        id: pending.id,
        source,
        start_sample: pending.start_sample,
        end_sample,
        pcm: pending.pcm,
        final_window: true,
        revision: pending.revision.saturating_add(1),
    }
}

#[cfg(test)]
#[path = "segmenter_tests.rs"]
mod segmenter_tests;
