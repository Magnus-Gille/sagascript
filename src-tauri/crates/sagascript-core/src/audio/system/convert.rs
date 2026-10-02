//! Real-time-safe-ish conversion of native capture chunks to 16 kHz mono, plus
//! timeline gap filling. Runs on a consumer thread, never in the audio callback.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

use crate::audio::resample::{mix_to_mono, TARGET_SAMPLE_RATE};

const CHUNK: usize = 1024;

/// Streaming downmix + resample to 16 kHz mono. Feed arbitrary-sized
/// interleaved chunks; output is appended as soon as full resampler blocks are
/// available. `finish` flushes the tail (zero-padded).
pub struct StreamingConverter {
    channels: usize,
    resampler: Option<SincFixedIn<f32>>,
    pending: Vec<f32>,
}

impl StreamingConverter {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        if sample_rate == 0 || channels == 0 {
            return Err("sample rate and channel count must be > 0".into());
        }
        let resampler = if sample_rate == TARGET_SAMPLE_RATE {
            None
        } else {
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            };
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            Some(
                SincFixedIn::<f32>::new(ratio, 2.0, params, CHUNK, 1)
                    .map_err(|e| format!("Failed to create resampler: {e}"))?,
            )
        };
        Ok(Self { channels: channels as usize, resampler, pending: Vec::new() })
    }

    /// Push interleaved frames; returns newly available 16 kHz mono samples.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, String> {
        let mono = mix_to_mono(interleaved, self.channels);
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(mono);
        };
        self.pending.extend_from_slice(&mono);
        let mut out = Vec::new();
        while self.pending.len() >= CHUNK {
            let block: Vec<f32> = self.pending.drain(..CHUNK).collect();
            let r = resampler.process(&[block], None).map_err(|e| format!("Resample failed: {e}"))?;
            out.extend_from_slice(&r[0]);
        }
        Ok(out)
    }

    /// Flush remaining input (zero-padded to a full block, output trimmed).
    pub fn finish(&mut self) -> Result<Vec<f32>, String> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let real = self.pending.len();
        let mut block = std::mem::take(&mut self.pending);
        block.resize(CHUNK, 0.0);
        let r = resampler.process(&[block], None).map_err(|e| format!("Resample failed: {e}"))?;
        let keep = ((real as f64 / CHUNK as f64) * r[0].len() as f64).round() as usize;
        Ok(r[0][..keep.min(r[0].len())].to_vec())
    }
}

/// WASAPI loopback (and a muted/idle process tap) deliver no data while nothing
/// plays, which would shorten the system track relative to the microphone.
/// Given wall-clock elapsed time and the 16 kHz samples delivered so far, this
/// returns how many silent samples to insert so both tracks share a timeline.
/// Gaps under `tolerance_ms` are left alone (normal scheduling jitter).
pub fn silence_to_insert(elapsed_ms: u64, delivered_samples: u64, tolerance_ms: u64) -> u64 {
    let expected = elapsed_ms * TARGET_SAMPLE_RATE as u64 / 1000;
    let tolerance = tolerance_ms * TARGET_SAMPLE_RATE as u64 / 1000;
    let missing = expected.saturating_sub(delivered_samples);
    if missing > tolerance { missing } else { 0 }
}

/// Drift between two tracks that should be the same length, in milliseconds
/// (positive: the first track is longer).
pub fn drift_ms(a_samples: usize, b_samples: usize) -> i64 {
    (a_samples as i64 - b_samples as i64) * 1000 / TARGET_SAMPLE_RATE as i64
}

/// Sample layout of a capture packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmLayout {
    F32,
    I16,
}

/// Decode a raw little-endian capture packet to f32. A packet flagged silent
/// (WASAPI `AUDCLNT_BUFFERFLAGS_SILENT`) yields `frames * channels` zeros
/// regardless of buffer content.
pub fn decode_pcm_packet(bytes: &[u8], layout: PcmLayout, silent: bool, samples: usize) -> Vec<f32> {
    if silent {
        return vec![0.0; samples];
    }
    match layout {
        PcmLayout::F32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .take(samples)
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
        PcmLayout::I16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .take(samples)
            .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32, freq: f32, channels: usize) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        let mut v = Vec::with_capacity(n * channels);
        for i in 0..n {
            let s = (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin() * 0.5;
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    #[test]
    fn passthrough_at_16k_downmixes_stereo() {
        let mut c = StreamingConverter::new(16_000, 2).unwrap();
        let out = c.push(&[1.0, 0.0, 0.5, 0.5]).unwrap();
        assert_eq!(out, vec![0.5, 0.5]);
        assert!(c.finish().unwrap().is_empty());
    }

    #[test]
    fn streaming_48k_stereo_yields_expected_length_in_odd_chunks() {
        let input = sine(48_000, 2.0, 440.0, 2);
        let mut c = StreamingConverter::new(48_000, 2).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 777) {
            out.extend(c.push(chunk).unwrap());
        }
        out.extend(c.finish().unwrap());
        let expected = 32_000i64;
        assert!((out.len() as i64 - expected).abs() < 600, "len {}", out.len());
        let peak = out[4000..out.len() - 4000].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!((0.4..0.6).contains(&peak), "peak {peak}");
    }

    #[test]
    fn rejects_zero_rate() {
        assert!(StreamingConverter::new(0, 2).is_err());
        assert!(StreamingConverter::new(48_000, 0).is_err());
    }

    #[test]
    fn gap_filling_respects_tolerance() {
        assert_eq!(silence_to_insert(1000, 16_000, 100), 0);
        assert_eq!(silence_to_insert(1050, 16_000, 100), 0);
        assert_eq!(silence_to_insert(2000, 16_000, 100), 16_000);
        assert_eq!(silence_to_insert(500, 16_000, 100), 0);
    }

    #[test]
    fn decodes_packets() {
        let f: Vec<u8> = [0.5f32, -0.25].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(decode_pcm_packet(&f, PcmLayout::F32, false, 2), vec![0.5, -0.25]);
        let i: Vec<u8> = [16384i16, -32768].iter().flat_map(|x| x.to_le_bytes()).collect();
        assert_eq!(decode_pcm_packet(&i, PcmLayout::I16, false, 2), vec![0.5, -1.0]);
        assert_eq!(decode_pcm_packet(&f, PcmLayout::F32, true, 4), vec![0.0; 4]);
    }

    #[test]
    fn drift_sign_and_scale() {
        assert_eq!(drift_ms(16_160, 16_000), 10);
        assert_eq!(drift_ms(16_000, 16_320), -20);
    }
}
