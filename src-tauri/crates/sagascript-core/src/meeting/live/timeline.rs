//! A bounded 16 kHz timeline for one native audio source.

use std::collections::VecDeque;
use std::fmt;

use super::resampler::{ResamplerError, StreamResampler};

pub(crate) const MAX_RETAINED_SAMPLES: usize = 32_000;
pub(crate) const MAX_RETAINED_CHUNKS: usize = 128;
const MAX_NATIVE_SAMPLES: usize = 2_048;
const TIMELINE_CHUNK_SIZE: usize = 256;
const OUTPUT_SAMPLE_RATE: u64 = 16_000;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
const MAX_DRIFT_NS: i128 = 50_000_000;
const MAX_RESAMPLER_OUTPUT: usize = 4_096;
const RESAMPLER_OUTPUT_SLACK: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineError {
    InputTooLarge,
    NonFiniteInput,
    InvalidTimestamp,
    RateChange,
    BackwardOverlap,
    LateData,
    Capacity,
    DrainOrder,
    Finished,
    Resampler(ResamplerError),
}

impl fmt::Display for TimelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Self::Resampler(error) = self {
            return write!(formatter, "live_timeline.resampler: {error}");
        }
        formatter.write_str(match self {
            Self::InputTooLarge => "live_timeline.input_too_large",
            Self::NonFiniteInput => "live_timeline.non_finite_input",
            Self::InvalidTimestamp => "live_timeline.invalid_timestamp",
            Self::RateChange => "live_timeline.rate_change",
            Self::BackwardOverlap => "live_timeline.backward_overlap",
            Self::LateData => "live_timeline.late_data",
            Self::Capacity => "live_timeline.capacity",
            Self::DrainOrder => "live_timeline.drain_order",
            Self::Finished => "live_timeline.finished",
            Self::Resampler(_) => unreachable!("handled above"),
        })
    }
}

impl std::error::Error for TimelineError {}

impl From<ResamplerError> for TimelineError {
    fn from(error: ResamplerError) -> Self {
        Self::Resampler(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discontinuity {
    pub start_sample: u64,
    pub end_sample: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushOutcome {
    pub start_sample: u64,
    pub end_sample: u64,
    pub offset_samples: i64,
    pub discontinuity: Option<Discontinuity>,
}

struct TimelineChunk {
    start_sample: u64,
    samples: VecDeque<f32>,
}

pub struct NativeTimeline {
    session_host_ns: u64,
    input_rate: Option<u32>,
    first_host_ns: Option<u64>,
    anchor_sample: u64,
    native_samples_seen: u64,
    next_output_sample: u64,
    drained_cursor: u64,
    drain_started: bool,
    resampler: Option<StreamResampler>,
    chunks: VecDeque<TimelineChunk>,
    retained_samples: usize,
    idle_reset: bool,
    finished: bool,
}

impl NativeTimeline {
    pub fn new(session_host_ns: u64) -> Self {
        Self {
            session_host_ns,
            input_rate: None,
            first_host_ns: None,
            anchor_sample: 0,
            native_samples_seen: 0,
            next_output_sample: 0,
            drained_cursor: 0,
            drain_started: false,
            resampler: None,
            chunks: VecDeque::new(),
            retained_samples: 0,
            idle_reset: false,
            finished: false,
        }
    }

    pub fn push(
        &mut self,
        host_ns: u64,
        input_rate: u32,
        mono: &[f32],
    ) -> Result<PushOutcome, TimelineError> {
        if self.finished {
            return Err(TimelineError::Finished);
        }
        if mono.len() > MAX_NATIVE_SAMPLES {
            return Err(TimelineError::InputTooLarge);
        }
        if mono.iter().any(|sample| !sample.is_finite()) {
            return Err(TimelineError::NonFiniteInput);
        }
        if let Some(rate) = self.input_rate {
            if rate != input_rate {
                return Err(TimelineError::RateChange);
            }
        }
        let observed_anchor = host_to_sample(self.session_host_ns, host_ns)?;
        let mut discontinuity = None;
        let mut offset_samples = 0;

        if self.first_host_ns.is_none() {
            // Construct the resampler before publishing any first-callback
            // state. An invalid rate must leave this timeline reusable.
            let resampler = StreamResampler::new(input_rate)?;
            if self.idle_reset && observed_anchor < self.next_output_sample {
                return Err(TimelineError::LateData);
            }
            self.input_rate = Some(input_rate);
            self.first_host_ns = Some(host_ns);
            self.anchor_sample = observed_anchor;
            if self.idle_reset {
                if observed_anchor > self.next_output_sample {
                    discontinuity = Some(Discontinuity {
                        start_sample: self.next_output_sample,
                        end_sample: observed_anchor,
                    });
                }
                self.next_output_sample = observed_anchor;
                self.idle_reset = false;
            } else {
                self.next_output_sample = observed_anchor;
            }
            self.resampler = Some(resampler);
        } else {
            let rate = self
                .input_rate
                .expect("initialized with first host timestamp");
            let expected_host = expected_host_ns(
                self.first_host_ns.expect("first host timestamp"),
                self.native_samples_seen,
                rate,
            )?;
            let host_offset = host_ns as i128 - expected_host as i128;
            let expected_anchor = self
                .anchor_sample
                .checked_add(
                    (self.native_samples_seen as u128 * OUTPUT_SAMPLE_RATE as u128 / rate as u128)
                        as u64,
                )
                .ok_or(TimelineError::InvalidTimestamp)?;
            offset_samples = signed_difference(observed_anchor, expected_anchor);

            if host_offset < -MAX_DRIFT_NS {
                return Err(TimelineError::BackwardOverlap);
            }
            if host_offset > MAX_DRIFT_NS {
                if observed_anchor < self.next_output_sample {
                    return Err(TimelineError::LateData);
                }
                self.ensure_append_capacity(self.next_output_sample, max_finish_output(rate))?;
                let old_start = self.next_output_sample;
                let flushed = self
                    .resampler
                    .as_mut()
                    .ok_or(TimelineError::InvalidTimestamp)?
                    .finish()?;
                self.append_samples(old_start, &flushed)?;
                self.next_output_sample = old_start
                    .checked_add(flushed.len() as u64)
                    .ok_or(TimelineError::InvalidTimestamp)?;
                let discontinuity_start = self.next_output_sample;
                if observed_anchor < discontinuity_start {
                    return Err(TimelineError::LateData);
                }
                discontinuity = Some(Discontinuity {
                    start_sample: discontinuity_start,
                    end_sample: observed_anchor,
                });
                self.first_host_ns = Some(host_ns);
                self.anchor_sample = observed_anchor;
                self.native_samples_seen = 0;
                self.next_output_sample = observed_anchor;
                self.resampler = Some(StreamResampler::new(rate)?);
            }
        }

        let output_start = self.next_output_sample;
        self.ensure_append_capacity(output_start, max_push_output(input_rate, mono.len()))?;
        let output = self
            .resampler
            .as_mut()
            .ok_or(TimelineError::InvalidTimestamp)?
            .push(mono)?;
        self.append_samples(output_start, &output)?;
        self.next_output_sample = self
            .next_output_sample
            .checked_add(output.len() as u64)
            .ok_or(TimelineError::InvalidTimestamp)?;
        self.native_samples_seen = self
            .native_samples_seen
            .checked_add(mono.len() as u64)
            .ok_or(TimelineError::InvalidTimestamp)?;

        Ok(PushOutcome {
            start_sample: output_start,
            end_sample: self.next_output_sample,
            offset_samples,
            discontinuity,
        })
    }

    /// Flush filter-held samples before an expected callback idle period.
    ///
    /// The retained timeline remains drainable. The next callback starts a
    /// fresh host-time anchor while retaining the established input rate and
    /// output cursor.
    pub fn flush_idle(&mut self) -> Result<(), TimelineError> {
        if self.finished {
            return Err(TimelineError::Finished);
        }
        let Some(rate) = self.input_rate else {
            return Ok(());
        };
        if self.idle_reset {
            return Ok(());
        }
        self.ensure_append_capacity(self.next_output_sample, max_finish_output(rate))?;
        let start = self.next_output_sample;
        let flushed = self
            .resampler
            .as_mut()
            .ok_or(TimelineError::InvalidTimestamp)?
            .finish()?;
        self.append_samples(start, &flushed)?;
        self.next_output_sample = self
            .next_output_sample
            .checked_add(flushed.len() as u64)
            .ok_or(TimelineError::InvalidTimestamp)?;
        self.resampler = Some(StreamResampler::new(rate)?);
        self.first_host_ns = None;
        self.anchor_sample = self.next_output_sample;
        self.native_samples_seen = 0;
        self.idle_reset = true;
        Ok(())
    }

    pub fn drain_frame(&mut self, start_sample: u64) -> Result<[f32; 160], TimelineError> {
        if !self.drain_started {
            if start_sample != 0 {
                return Err(TimelineError::DrainOrder);
            }
            self.drain_started = true;
        } else if start_sample != self.drained_cursor {
            return Err(TimelineError::DrainOrder);
        }
        let end_sample = start_sample
            .checked_add(160)
            .ok_or(TimelineError::InvalidTimestamp)?;
        let mut frame = [0.0; 160];
        let mut cursor = start_sample;

        while cursor < end_sample && !self.chunks.is_empty() {
            let (chunk_start, chunk_len) = {
                let chunk = self.chunks.front().expect("checked non-empty");
                (chunk.start_sample, chunk.samples.len())
            };
            let chunk_end = chunk_start
                .checked_add(chunk_len as u64)
                .ok_or(TimelineError::InvalidTimestamp)?;
            if chunk_end <= cursor {
                self.retained_samples -= chunk_len;
                self.chunks.pop_front();
                continue;
            }
            if chunk_start > cursor {
                cursor = chunk_start.min(end_sample);
                continue;
            }
            if chunk_start < cursor {
                let trim = (cursor - chunk_start) as usize;
                let trim = trim.min(chunk_len);
                let chunk = self.chunks.front_mut().expect("checked non-empty");
                for _ in 0..trim {
                    chunk.samples.pop_front();
                }
                chunk.start_sample += trim as u64;
                self.retained_samples -= trim;
                if trim == chunk_len {
                    self.chunks.pop_front();
                    continue;
                }
            }
            let count = (end_sample - cursor) as usize;
            let count = count.min(
                self.chunks
                    .front()
                    .expect("checked non-empty")
                    .samples
                    .len(),
            );
            let frame_offset = (cursor - start_sample) as usize;
            let chunk = self.chunks.front_mut().expect("checked non-empty");
            for sample in frame[frame_offset..frame_offset + count].iter_mut() {
                *sample = chunk.samples.pop_front().expect("counted sample");
            }
            self.retained_samples -= count;
            cursor += count as u64;
            let empty = self
                .chunks
                .front()
                .expect("checked non-empty")
                .samples
                .is_empty();
            if empty {
                self.chunks.pop_front();
            } else {
                self.chunks
                    .front_mut()
                    .expect("checked non-empty")
                    .start_sample += count as u64;
            }
        }
        self.drained_cursor = end_sample;
        Ok(frame)
    }

    pub fn finish(&mut self) -> Result<(), TimelineError> {
        if self.finished {
            return Ok(());
        }
        let start = self.next_output_sample;
        if let Some(rate) = self.input_rate {
            self.ensure_append_capacity(start, max_finish_output(rate))?;
        }
        let output = self
            .resampler
            .as_mut()
            .map(StreamResampler::finish)
            .transpose()?
            .unwrap_or_default();
        self.append_samples(start, &output)?;
        self.next_output_sample = self
            .next_output_sample
            .checked_add(output.len() as u64)
            .ok_or(TimelineError::InvalidTimestamp)?;
        self.finished = true;
        Ok(())
    }

    #[cfg(test)]
    fn retained_samples(&self) -> usize {
        self.retained_samples
    }

    #[cfg(test)]
    fn retained_chunks(&self) -> usize {
        self.chunks.len()
    }

    fn append_samples(&mut self, start_sample: u64, samples: &[f32]) -> Result<(), TimelineError> {
        if samples.is_empty() {
            return Ok(());
        }
        self.ensure_append_capacity(start_sample, samples.len())?;
        let next_retained = self.retained_samples + samples.len();

        let mut offset = 0usize;
        let mut cursor = start_sample;
        if let Some(last) = self.chunks.back_mut() {
            let last_end = last.start_sample + last.samples.len() as u64;
            if last_end == cursor && last.samples.len() < TIMELINE_CHUNK_SIZE {
                let take = (TIMELINE_CHUNK_SIZE - last.samples.len()).min(samples.len());
                last.samples.extend(&samples[..take]);
                offset = take;
                cursor += take as u64;
            }
        }
        while offset < samples.len() {
            let take = TIMELINE_CHUNK_SIZE.min(samples.len() - offset);
            self.chunks.push_back(TimelineChunk {
                start_sample: cursor,
                samples: samples[offset..offset + take].iter().copied().collect(),
            });
            offset += take;
            cursor += take as u64;
        }
        self.retained_samples = next_retained;
        Ok(())
    }

    fn ensure_append_capacity(
        &self,
        start_sample: u64,
        sample_count: usize,
    ) -> Result<(), TimelineError> {
        if sample_count == 0 {
            return Ok(());
        }
        if start_sample < self.drained_cursor {
            return Err(TimelineError::LateData);
        }
        let next_retained = self
            .retained_samples
            .checked_add(sample_count)
            .ok_or(TimelineError::Capacity)?;
        if next_retained > MAX_RETAINED_SAMPLES {
            return Err(TimelineError::Capacity);
        }
        let mut remaining = sample_count;
        if let Some(last) = self.chunks.back() {
            let last_end = last.start_sample + last.samples.len() as u64;
            if last_end == start_sample && last.samples.len() < TIMELINE_CHUNK_SIZE {
                remaining = remaining.saturating_sub(TIMELINE_CHUNK_SIZE - last.samples.len());
            }
        }
        let additional_chunks = remaining.div_ceil(TIMELINE_CHUNK_SIZE);
        if self.chunks.len() + additional_chunks > MAX_RETAINED_CHUNKS {
            return Err(TimelineError::Capacity);
        }
        Ok(())
    }
}

fn max_push_output(input_rate: u32, input_samples: usize) -> usize {
    if input_samples == 0 {
        return 0;
    }
    if input_rate == OUTPUT_SAMPLE_RATE as u32 {
        return input_samples;
    }
    let ratio_bound = (input_samples as u128 * OUTPUT_SAMPLE_RATE as u128)
        .saturating_add(input_rate as u128 - 1)
        / input_rate as u128;
    usize::try_from(ratio_bound)
        .unwrap_or(MAX_RESAMPLER_OUTPUT)
        .saturating_add(RESAMPLER_OUTPUT_SLACK)
        .min(MAX_RESAMPLER_OUTPUT)
}

fn max_finish_output(input_rate: u32) -> usize {
    if input_rate == OUTPUT_SAMPLE_RATE as u32 {
        0
    } else {
        MAX_RESAMPLER_OUTPUT
    }
}

fn host_to_sample(session_host_ns: u64, host_ns: u64) -> Result<u64, TimelineError> {
    if host_ns < session_host_ns {
        return Err(TimelineError::InvalidTimestamp);
    }
    let delta = host_ns - session_host_ns;
    let rounded =
        (delta as u128 * OUTPUT_SAMPLE_RATE as u128 + NANOS_PER_SECOND / 2) / NANOS_PER_SECOND;
    u64::try_from(rounded).map_err(|_| TimelineError::InvalidTimestamp)
}

fn expected_host_ns(
    first_host_ns: u64,
    native_samples_seen: u64,
    input_rate: u32,
) -> Result<u64, TimelineError> {
    let delta = native_samples_seen as u128 * NANOS_PER_SECOND / input_rate as u128;
    u64::try_from(first_host_ns as u128 + delta).map_err(|_| TimelineError::InvalidTimestamp)
}

fn signed_difference(observed: u64, expected: u64) -> i64 {
    let difference = observed as i128 - expected as i128;
    difference.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod timeline_tests;
