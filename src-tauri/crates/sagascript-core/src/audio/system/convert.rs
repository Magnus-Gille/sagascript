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
    ratio: f64,
    /// Mono input frames received and output frames emitted.
    total_in: u64,
    total_out: u64,
}

impl StreamingConverter {
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self, String> {
        if sample_rate == 0 || channels == 0 {
            return Err("sample rate and channel count must be > 0".into());
        }
        let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
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
            Some(
                SincFixedIn::<f32>::new(ratio, 2.0, params, CHUNK, 1)
                    .map_err(|e| format!("Failed to create resampler: {e}"))?,
            )
        };
                Ok(Self { channels: channels as usize, resampler, pending: Vec::new(), ratio, total_in: 0, total_out: 0 })
    }

    /// rubato's sinc resampler is centred: output frame n already corresponds to
    /// input n/ratio (verified by `terminal_impulse_survives_flush_in_place`), so
    /// there is no leading delay to discard. The cost is on the tail: the last
    /// `sinc_len / 2` input frames only produce output once more input arrives,
    /// which `finish` supplies as zero padding.
    fn account(&mut self, block: Vec<f32>) -> Vec<f32> {
        self.total_out += block.len() as u64;
        block
    }

    /// Push interleaved frames; returns newly available 16 kHz mono samples.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<Vec<f32>, String> {
        let mono = mix_to_mono(interleaved, self.channels);
        if self.resampler.is_none() {
            return Ok(mono);
        }
        self.total_in += mono.len() as u64;
        self.pending.extend_from_slice(&mono);
        let mut out = Vec::new();
        while self.pending.len() >= CHUNK {
            let block: Vec<f32> = self.pending.drain(..CHUNK).collect();
            let r = self.resampler.as_mut().unwrap().process(&[block], None).map_err(|e| format!("Resample failed: {e}"))?;
            let r = self.account(r.into_iter().next().unwrap_or_default());
            out.extend_from_slice(&r);
        }
        Ok(out)
    }

    /// Flush the tail: feed zero padding through the resampler until every real
    /// input frame has produced its output (the centred filter's look-ahead), then
    /// trim to exactly `round(total_in * ratio)` frames.
    pub fn finish(&mut self) -> Result<Vec<f32>, String> {
        if self.resampler.is_none() || self.total_in == 0 {
            return Ok(Vec::new());
        }
        let expected = (self.total_in as f64 * self.ratio).round() as u64;
        let mut out = Vec::new();
        // Bounded: each block yields ~CHUNK*ratio frames and the delay is a few hundred.
        for _ in 0..64 {
            if self.total_out >= expected {
                break;
            }
            let mut block = std::mem::take(&mut self.pending);
            block.resize(CHUNK, 0.0);
            let r = self.resampler.as_mut().unwrap().process(&[block], None).map_err(|e| format!("Resample failed: {e}"))?;
            let r = self.account(r.into_iter().next().unwrap_or_default());
            out.extend_from_slice(&r);
        }
        let emitted_before = self.total_out - out.len() as u64;
        let keep = expected.saturating_sub(emitted_before) as usize;
        out.truncate(keep);
        Ok(out)
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

    /// Total output length equals round(input * ratio) at every alignment,
    /// including exact multiples of the 1024-frame block.
    #[test]
    fn output_length_is_exact_at_block_boundaries() {
        for frames in [1024usize, 2048, 3000, 1, 4800] {
            let mut c = StreamingConverter::new(48_000, 1).unwrap();
            let mut out = c.push(&vec![0.25; frames]).unwrap();
            out.extend(c.finish().unwrap());
            assert_eq!(out.len(), (frames as f64 / 3.0).round() as usize, "frames {frames}");
        }
    }

    /// An impulse at the very end of the input must survive the flush, at the
    /// right place, also when the input ends exactly
    /// on a block boundary.
    #[test]
    fn terminal_impulse_survives_flush_in_place() {
        for frames in [2048usize, 3000] {
            let mut input = vec![0.0f32; frames];
            *input.last_mut().unwrap() = 1.0;
            let mut c = StreamingConverter::new(48_000, 1).unwrap();
            let mut out = c.push(&input).unwrap();
            out.extend(c.finish().unwrap());
            let (idx, peak) = out
                .iter()
                .enumerate()
                .fold((0, 0f32), |m, (i, s)| if s.abs() > m.1 { (i, s.abs()) } else { m });
            let expected_idx = (frames - 1) / 3;
            assert!(peak > 0.2, "frames {frames}: impulse lost, peak {peak}");
            assert!((idx as i64 - expected_idx as i64).abs() <= 2, "frames {frames}: impulse at {idx}, expected ~{expected_idx}");
        }
    }

    #[test]
    fn terminal_impulse_and_length_at_44k1_and_short_input() {
        // 44.1 kHz (non-integer ratio), fed in odd partitions.
        for frames in [4410usize, 5000, 100, 20] {
            let mut input = vec![0.0f32; frames];
            *input.last_mut().unwrap() = 1.0;
            let mut c = StreamingConverter::new(44_100, 1).unwrap();
            let mut out = Vec::new();
            for part in input.chunks(333) {
                out.extend(c.push(part).unwrap());
            }
            out.extend(c.finish().unwrap());
            let expected_len = (frames as f64 * 16_000.0 / 44_100.0).round() as usize;
            assert_eq!(out.len(), expected_len, "frames {frames}");
            let (idx, peak) = out
                .iter()
                .enumerate()
                .fold((0, 0f32), |m, (i, s)| if s.abs() > m.1 { (i, s.abs()) } else { m });
            assert!(peak > 0.1, "frames {frames}: impulse lost");
            let want = ((frames - 1) as f64 * 16_000.0 / 44_100.0) as i64;
            assert!((idx as i64 - want).abs() <= 2, "frames {frames}: at {idx}, want ~{want}");
        }
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
