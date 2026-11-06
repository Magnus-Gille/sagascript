//! The streaming resampler of the compact tier (design §4.4, D1/D7).
//!
//! The compact tier stores 16 kHz, 16-bit, mono. The device delivers its own
//! rate and one or two channels, so every block is downmixed (the mean of the
//! channels) and resampled as it arrives. `audio::resample` resamples one
//! finished buffer at stop; this works on a stream:
//!
//! * **Chunked.** Input is collected until a full chunk is available and then
//!   run through a `rubato` `SincFixedIn` (the same sinc parameters as the
//!   batch resampler). Memory is a chunk, not the recording.
//! * **Delay compensation.** `rubato` centres its sinc kernel: output frame
//!   `n` is the signal at input time `n / 16000`, and what its
//!   [`output_delay`](Resampler::output_delay) reports is *latency*: the last
//!   [`StreamResampler::delay`] frames of the stream come out only after more
//!   input has arrived, which [`finish`](StreamResampler::finish) pays by
//!   feeding zeros. So nothing is shifted: the track starts at the host time
//!   of the first input frame and its clock anchors need no correction. (A
//!   first version discarded `delay` frames at the start, assuming the delay
//!   were a shift; the impulse test showed that moved every sample 2.7 ms
//!   early, and the test now pins the alignment to within one stored frame.)
//! * **Flush.** [`finish`](StreamResampler::finish) feeds the filter zeros
//!   until everything that depends on real input has come out, and returns
//!   exactly `round(input_frames * 16000 / rate)` frames in total.
//! * **Rounding** to 16 bits is the store's: to nearest, no dither.
//!
//! A gap ends a *segment*: the session finishes the resampler, writes the
//! silence, and starts a fresh one, so stored positions of gaps and anchors
//! are exact and the filter never smears audio across a hole.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::recording::RecordingError;

/// The compact tier's sample rate.
pub const COMPACT_RATE: u32 = 16_000;
/// Input frames per resampler call.
const CHUNK_FRAMES: usize = 1024;

/// Downmixes to mono and resamples to 16 kHz as audio arrives.
pub struct StreamResampler {
    inner: Option<SincFixedIn<f32>>,
    channels: usize,
    ratio: f64,
    /// Mono input waiting for a full chunk.
    pending: Vec<f32>,
    out: Vec<Vec<f32>>,
    delay: usize,
    input_frames: u64,
    emitted: u64,
}

impl StreamResampler {
    /// A resampler for `source_rate` Hz and `channels` (1 or 2) interleaved.
    pub fn new(source_rate: u32, channels: u8) -> Result<Self, RecordingError> {
        if source_rate == 0 || !(1..=2).contains(&channels) {
            return Err(RecordingError::UnsupportedFormat {
                reason: "the compact tier takes one or two channels at a real rate",
            });
        }
        let ratio = f64::from(COMPACT_RATE) / f64::from(source_rate);
        let (inner, out, delay) =
            if source_rate == COMPACT_RATE {
                (None, vec![Vec::new()], 0)
            } else {
                let params = SincInterpolationParameters {
                    sinc_len: 256,
                    f_cutoff: 0.95,
                    interpolation: SincInterpolationType::Linear,
                    oversampling_factor: 256,
                    window: WindowFunction::BlackmanHarris2,
                };
                let resampler = SincFixedIn::<f32>::new(ratio, 1.0, params, CHUNK_FRAMES, 1)
                    .map_err(|_| RecordingError::UnsupportedFormat {
                        reason: "this sample rate cannot be resampled to 16 kHz",
                    })?;
                let out = vec![vec![0.0; resampler.output_frames_max()]];
                let delay = resampler.output_delay();
                (Some(resampler), out, delay)
            };
        Ok(Self {
            inner,
            channels: usize::from(channels),
            ratio,
            pending: Vec::with_capacity(CHUNK_FRAMES + 4096),
            out,
            delay,
            input_frames: 0,
            emitted: 0,
        })
    }

    /// Output frames the filter holds back: latency that `finish` pays, not a shift.
    pub fn delay(&self) -> usize {
        self.delay
    }

    /// Input frames consumed so far.
    pub fn input_frames(&self) -> u64 {
        self.input_frames
    }

    /// Output frames returned so far.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// Whether the audio passes through unfiltered (it is 16 kHz already).
    pub fn is_passthrough(&self) -> bool {
        self.inner.is_none()
    }

    /// Add interleaved input; the resampled mono frames that became available
    /// are appended to `out`.
    pub fn process(
        &mut self,
        interleaved: &[f32],
        out: &mut Vec<f32>,
    ) -> Result<(), RecordingError> {
        let channels = self.channels;
        self.input_frames += (interleaved.len() / channels) as u64;
        if channels == 1 {
            self.pending.extend_from_slice(interleaved);
        } else {
            self.pending.extend(
                interleaved
                    .chunks_exact(channels)
                    .map(|frame| frame.iter().sum::<f32>() / channels as f32),
            );
        }
        let Some(inner) = self.inner.as_mut() else {
            self.emitted += self.pending.len() as u64;
            out.append(&mut self.pending);
            return Ok(());
        };
        loop {
            let need = inner.input_frames_next();
            if self.pending.len() < need {
                return Ok(());
            }
            let (used, produced) = inner
                .process_into_buffer(&[&self.pending[..need]], &mut self.out, None)
                .map_err(RecordingError::encoder)?;
            self.pending.drain(..used);
            emit(&self.out[0], produced, &mut self.emitted, out);
        }
    }

    /// Flush the filter and append the last frames to `out`, so that the total
    /// returned over the life of the resampler is exactly
    /// `round(input_frames * 16000 / rate)` frames. The resampler must not be
    /// used afterwards.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), RecordingError> {
        let target = (self.input_frames as f64 * self.ratio).round() as u64;
        if self.inner.is_some() && self.input_frames > 0 {
            // The partial chunk, zero padded, then zeros until the delay is paid.
            let mut first = true;
            for _ in 0..16 {
                if self.emitted >= target {
                    break;
                }
                let Some(inner) = self.inner.as_mut() else {
                    break;
                };
                let produced = if first && !self.pending.is_empty() {
                    let (_, produced) = inner
                        .process_partial_into_buffer(
                            Some(&[std::mem::take(&mut self.pending)]),
                            &mut self.out,
                            None,
                        )
                        .map_err(RecordingError::encoder)?;
                    produced
                } else {
                    let (_, produced) = inner
                        .process_partial_into_buffer(None::<&[Vec<f32>]>, &mut self.out, None)
                        .map_err(RecordingError::encoder)?;
                    produced
                };
                first = false;
                emit(&self.out[0], produced, &mut self.emitted, out);
            }
        }
        // Exactly the target: drop the surplus of the last chunk (it came
        // from the padding) or, should the filter ever come up short, repeat
        // silence rather than lose the alignment.
        if self.emitted > target {
            let surplus = (self.emitted - target) as usize;
            out.truncate(out.len() - surplus.min(out.len()));
            self.emitted = target;
        }
        while self.emitted < target {
            out.push(0.0);
            self.emitted += 1;
        }
        self.pending.clear();
        Ok(())
    }
}

/// Append `produced` frames of `buffer` to `out`.
fn emit(buffer: &[f32], produced: usize, emitted: &mut u64, out: &mut Vec<f32>) {
    let kept = &buffer[..produced];
    *emitted += kept.len() as u64;
    out.extend_from_slice(kept);
}

#[cfg(test)]
#[path = "resampler_tests.rs"]
mod tests;
