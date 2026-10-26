//! Bounded, callback-friendly mono resampling for live audio.

use std::collections::VecDeque;
use std::fmt;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const MIN_INPUT_RATE: u32 = 8_000;
const MAX_INPUT_RATE: u32 = 192_000;
const MAX_INPUT_SAMPLES: usize = 8_192;
const CHUNK_SIZE: usize = 1_024;
const MAX_BUFFERED_OUTPUT: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResamplerError {
    InvalidRate,
    InputTooLarge,
    NonFiniteInput,
    Finished,
    InputLengthOverflow,
    ResamplerFailure,
    OutputLengthMismatch,
}

impl fmt::Display for ResamplerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRate => "invalid input sample rate",
            Self::InputTooLarge => "input callback is too large",
            Self::NonFiniteInput => "input contains a non-finite sample",
            Self::Finished => "resampler is finished",
            Self::InputLengthOverflow => "input length overflow",
            Self::ResamplerFailure => "resampler operation failed",
            Self::OutputLengthMismatch => "resampler produced an unexpected output length",
        })
    }
}

impl std::error::Error for ResamplerError {}

/// Incremental mono resampler with a fixed 16 kHz output rate.
///
/// Callbacks may have arbitrary boundaries, but each callback is bounded to
/// [`MAX_INPUT_SAMPLES`] samples. The resampler retains at most one input
/// chunk and a small output tail between calls.
pub struct StreamResampler {
    input_rate: u32,
    total_input: u64,
    emitted_output: u64,
    pending_input: VecDeque<f32>,
    pending_output: VecDeque<f32>,
    resampler: Option<SincFixedIn<f32>>,
    delay_remaining: usize,
    finished: bool,
}

impl StreamResampler {
    pub fn new(input_rate: u32) -> Result<Self, ResamplerError> {
        if !(MIN_INPUT_RATE..=MAX_INPUT_RATE).contains(&input_rate) {
            return Err(ResamplerError::InvalidRate);
        }

        let resampler = if input_rate == TARGET_SAMPLE_RATE {
            None
        } else {
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let ratio = TARGET_SAMPLE_RATE as f64 / input_rate as f64;
            Some(
                SincFixedIn::<f32>::new(ratio, 2.0, params, CHUNK_SIZE, 1)
                    .map_err(|_| ResamplerError::ResamplerFailure)?,
            )
        };

        let delay_remaining = resampler
            .as_ref()
            .map(|value| value.output_delay())
            .unwrap_or(0);

        Ok(Self {
            input_rate,
            total_input: 0,
            emitted_output: 0,
            pending_input: VecDeque::with_capacity(CHUNK_SIZE),
            pending_output: VecDeque::with_capacity(MAX_BUFFERED_OUTPUT),
            resampler,
            delay_remaining,
            finished: false,
        })
    }

    /// Push one callback's samples and return output that is safe to emit.
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, ResamplerError> {
        if self.finished {
            return Err(ResamplerError::Finished);
        }
        if samples.len() > MAX_INPUT_SAMPLES {
            return Err(ResamplerError::InputTooLarge);
        }
        if samples.iter().any(|sample| !sample.is_finite()) {
            return Err(ResamplerError::NonFiniteInput);
        }
        self.total_input = self
            .total_input
            .checked_add(samples.len() as u64)
            .ok_or(ResamplerError::InputLengthOverflow)?;

        if self.input_rate == TARGET_SAMPLE_RATE {
            self.emitted_output = self.total_input;
            return Ok(samples.to_vec());
        }

        self.pending_input.extend(samples.iter().copied());
        let mut output = Vec::new();
        while self.pending_input.len() >= CHUNK_SIZE {
            let chunk: Vec<f32> = self.pending_input.drain(..CHUNK_SIZE).collect();
            self.process_full_chunk(&chunk)?;
            output.extend(self.emit_ready()?);
        }
        output.extend(self.emit_ready()?);
        Ok(output)
    }

    /// Flush the filter and return the exact remaining output for the input.
    pub fn finish(&mut self) -> Result<Vec<f32>, ResamplerError> {
        if self.finished {
            return Ok(Vec::new());
        }
        self.finished = true;

        if self.input_rate == TARGET_SAMPLE_RATE {
            return Ok(Vec::new());
        }

        if self.total_input > 0 {
            let remainder: Vec<f32> = self.pending_input.drain(..).collect();
            if remainder.is_empty() {
                let result = self
                    .resampler
                    .as_mut()
                    .ok_or(ResamplerError::ResamplerFailure)?
                    .process_partial(None::<&[Vec<f32>]>, None)
                    .map_err(|_| ResamplerError::ResamplerFailure)?;
                self.append_output(&result[0])?;
            } else {
                let input = vec![remainder];
                let result = self
                    .resampler
                    .as_mut()
                    .ok_or(ResamplerError::ResamplerFailure)?
                    .process_partial(Some(&input), None)
                    .map_err(|_| ResamplerError::ResamplerFailure)?;
                self.append_output(&result[0])?;

                // A partial call pads the input chunk, but the filter still
                // needs one zero flush to release its right-hand delay.
                let flush = self
                    .resampler
                    .as_mut()
                    .ok_or(ResamplerError::ResamplerFailure)?
                    .process_partial(None::<&[Vec<f32>]>, None)
                    .map_err(|_| ResamplerError::ResamplerFailure)?;
                self.append_output(&flush[0])?;
            }
        }

        let expected = self.expected_output_count();
        let remaining = expected.saturating_sub(self.emitted_output) as usize;
        if self.pending_output.len() < remaining {
            return Err(ResamplerError::OutputLengthMismatch);
        }
        let mut output = Vec::with_capacity(remaining);
        for _ in 0..remaining {
            if let Some(sample) = self.pending_output.pop_front() {
                output.push(sample);
            }
        }
        self.emitted_output = expected;
        self.pending_output.clear();
        Ok(output)
    }

    fn process_full_chunk(&mut self, chunk: &[f32]) -> Result<(), ResamplerError> {
        let input = vec![chunk.to_vec()];
        let result = self
            .resampler
            .as_mut()
            .ok_or(ResamplerError::ResamplerFailure)?
            .process(&input, None)
            .map_err(|_| ResamplerError::ResamplerFailure)?;
        self.append_output(&result[0])?;
        Ok(())
    }

    fn append_output(&mut self, output: &[f32]) -> Result<(), ResamplerError> {
        let skip = self.delay_remaining.min(output.len());
        self.delay_remaining -= skip;
        let retained = output.len() - skip;
        if self.pending_output.len().saturating_add(retained) > MAX_BUFFERED_OUTPUT {
            return Err(ResamplerError::OutputLengthMismatch);
        }
        self.pending_output.extend(output[skip..].iter().copied());
        Ok(())
    }

    fn emit_ready(&mut self) -> Result<Vec<f32>, ResamplerError> {
        let target = self.expected_output_count();
        let available = target.saturating_sub(self.emitted_output) as usize;
        let count = available.min(self.pending_output.len());
        let mut output = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(sample) = self.pending_output.pop_front() {
                output.push(sample);
            }
        }
        self.emitted_output = self
            .emitted_output
            .checked_add(output.len() as u64)
            .ok_or(ResamplerError::InputLengthOverflow)?;
        Ok(output)
    }

    fn expected_output_count(&self) -> u64 {
        (self.total_input as u128 * TARGET_SAMPLE_RATE as u128 / self.input_rate as u128) as u64
    }

    #[cfg(test)]
    fn buffered_input_samples(&self) -> usize {
        self.pending_input.len()
    }

    #[cfg(test)]
    fn buffered_output_samples(&self) -> usize {
        self.pending_output.len()
    }
}

#[cfg(test)]
#[path = "resampler_tests.rs"]
mod resampler_tests;
