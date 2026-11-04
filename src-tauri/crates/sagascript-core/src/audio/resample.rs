use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

/// Required audio format for Whisper
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Mix multi-channel audio to mono by averaging all channels.
pub fn mix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// High-quality sinc resample from `source_rate` to `TARGET_SAMPLE_RATE` (16 kHz).
/// Uses rubato's SincFixedIn with sinc interpolation.
/// Returns the input unchanged if rates already match.
pub fn resample_to_16khz(mono: Vec<f32>, source_rate: u32) -> Result<Vec<f32>, String> {
    resample_to_16khz_with_progress(mono, source_rate, None)
}

/// Like [`resample_to_16khz`], but reports chunked progress as a 0.0–1.0
/// fraction of input samples consumed (only for the multi-chunk path;
/// single-chunk inputs report nothing until done). The callback must be
/// cheap — it runs ~`len/1024` times on the decode thread.
pub fn resample_to_16khz_with_progress(
    mono: Vec<f32>,
    source_rate: u32,
    on_progress: Option<&dyn Fn(f64)>,
) -> Result<Vec<f32>, String> {
    resample_to_16khz_with_control(mono, source_rate, on_progress, &|| Ok(()))
}

/// Cancellation is checked before work and between every bounded resampler call.
pub fn resample_to_16khz_with_control(
    mono: Vec<f32>,
    source_rate: u32,
    on_progress: Option<&dyn Fn(f64)>,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<Vec<f32>, String> {
    resample_to_16khz_bounded(mono, source_rate, on_progress, checkpoint, usize::MAX)
}

/// Like [`resample_to_16khz_with_control`], but the output never exceeds
/// `max_output_samples`: the limit is checked (including rubato's flush and
/// padding output) before every growth, and reservations are fallible, so an
/// oversized conversion returns an error instead of allocating or aborting.
pub(crate) fn resample_to_16khz_bounded(
    mono: Vec<f32>,
    source_rate: u32,
    on_progress: Option<&dyn Fn(f64)>,
    checkpoint: &dyn Fn() -> Result<(), String>,
    max_output_samples: usize,
) -> Result<Vec<f32>, String> {
    checkpoint()?;
    if source_rate == TARGET_SAMPLE_RATE || mono.is_empty() {
        return Ok(mono);
    }

    if source_rate == 0 {
        return Err("source sample rate must be > 0".to_string());
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let ratio = TARGET_SAMPLE_RATE as f64 / source_rate as f64;
    let chunk_size = 1024.min(mono.len());

    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk_size, 1)
        .map_err(|e| format!("Failed to create resampler: {e}"))?;

    // Expected output plus rubato's padding/flush headroom (a few max-size
    // output chunks), reserved fallibly and refused if over the limit.
    let expected = (mono.len() as f64 * ratio).ceil() as usize;
    let headroom = resampler
        .output_frames_max()
        .saturating_mul(3)
        .saturating_add(1024);
    let reserve = expected.saturating_add(headroom);
    let mut output: Vec<f32> = Vec::new();
    let initial = reserve.min(max_output_samples);
    if expected > max_output_samples {
        return Err("resampled output exceeds the memory budget".to_string());
    }
    output
        .try_reserve_exact(initial)
        .map_err(|_| "resampler output allocation refused".to_string())?;
    let append = |output: &mut Vec<f32>, extra: &[f32]| -> Result<(), String> {
        let new_len = output.len().saturating_add(extra.len());
        if new_len > max_output_samples {
            return Err("resampled output exceeds the memory budget".to_string());
        }
        if new_len > output.capacity() {
            output
                .try_reserve_exact(new_len - output.len())
                .map_err(|_| "resampler output allocation refused".to_string())?;
        }
        output.extend_from_slice(extra);
        Ok(())
    };

    // Process full chunks (rubato maintains state between calls)
    let full_chunks = mono.len() / chunk_size;
    for i in 0..full_chunks {
        checkpoint()?;
        let start = i * chunk_size;
        let chunk = vec![mono[start..start + chunk_size].to_vec()];
        let result = resampler
            .process(&chunk, None)
            .map_err(|e| format!("Resample failed: {e}"))?;
        append(&mut output, &result[0])?;
        if let Some(on_progress) = on_progress {
            if full_chunks > 0 {
                on_progress(i as f64 / full_chunks as f64);
            }
        }
    }

    // Process remaining samples with process_partial (handles short final chunk + flush)
    let remaining_start = full_chunks * chunk_size;
    checkpoint()?;
    if remaining_start < mono.len() {
        let remainder = vec![mono[remaining_start..].to_vec()];
        let result = resampler
            .process_partial(Some(&remainder), None)
            .map_err(|e| format!("Resample partial failed: {e}"))?;
        append(&mut output, &result[0])?;
    } else {
        let result = resampler
            .process_partial(None::<&[Vec<f32>]>, None)
            .map_err(|e| format!("Resample flush failed: {e}"))?;
        append(&mut output, &result[0])?;
    }

    checkpoint()?;
    Ok(output)
}

/// The batch resampler's fixed input chunk size, in source samples.
const STREAM_CHUNK: usize = 1024;

fn sinc_params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    }
}

/// Incremental counterpart of [`resample_to_16khz`]: feed mono source-rate
/// samples in chunks of any length, get 16 kHz samples out as they become
/// available.
///
/// The concatenation of everything [`Self::push`] and [`Self::finish`] append
/// is **bit-identical** to `resample_to_16khz(all_input, source_rate)`, for any
/// split of the input into chunks. It buffers input into the same fixed
/// 1024-sample chunks with the same `SincFixedIn` parameters as the batch
/// code, and `finish` treats the remainder exactly like the batch code does:
/// `process_partial(Some(remainder))`, or `process_partial(None)` when the
/// remainder is empty.
///
/// One batch quirk is reproduced on purpose: the batch code sizes its chunks
/// `1024.min(len)`, so an input *shorter than 1024 samples* is one single
/// chunk of its own length. This type cannot know the total up front, so it
/// holds such a short input back and, in `finish`, builds the same
/// shorter-chunk resampler; inputs of 1024 or more samples never take that
/// path. At the 16 kHz target rate input passes straight through.
///
/// The resampler itself is created on the first non-empty input, so an empty
/// stream (and a zero rate with no samples) is `Ok` with no output, as in the
/// batch code.
pub struct StreamingResampler {
    source_rate: u32,
    /// Created at the first full chunk; `None` while fewer than
    /// [`STREAM_CHUNK`] samples were seen (and always for 16 kHz input).
    resampler: Option<SincFixedIn<f32>>,
    /// Input not yet resampled: always fewer than [`STREAM_CHUNK`] samples
    /// between calls.
    pending: Vec<f32>,
    finished: bool,
}

impl StreamingResampler {
    pub fn new(source_rate: u32) -> Self {
        Self {
            source_rate,
            resampler: None,
            pending: Vec::new(),
            finished: false,
        }
    }

    fn passthrough(&self) -> bool {
        self.source_rate == TARGET_SAMPLE_RATE
    }

    fn build(&self, chunk_size: usize) -> Result<SincFixedIn<f32>, String> {
        if self.source_rate == 0 {
            return Err("source sample rate must be > 0".to_string());
        }
        let ratio = TARGET_SAMPLE_RATE as f64 / self.source_rate as f64;
        SincFixedIn::<f32>::new(ratio, 2.0, sinc_params(), chunk_size, 1)
            .map_err(|e| format!("Failed to create resampler: {e}"))
    }

    /// Append the 16 kHz samples that `mono` completes to `out`. May append
    /// nothing while input is still being buffered into a chunk.
    pub fn push(&mut self, mono: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        debug_assert!(!self.finished, "push after finish");
        if self.passthrough() {
            out.extend_from_slice(mono);
            return Ok(());
        }
        let mut rest = mono;
        // Top up a partial chunk first.
        if !self.pending.is_empty() {
            let take = (STREAM_CHUNK - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() < STREAM_CHUNK {
                return Ok(());
            }
            let chunk = std::mem::take(&mut self.pending);
            self.process_chunk(&chunk, out)?;
            self.pending = chunk;
            self.pending.clear();
        }
        // Whole chunks straight from the caller's slice, without a copy.
        let (chunks, remainder) = rest.as_chunks::<STREAM_CHUNK>();
        for chunk in chunks {
            self.process_chunk(chunk, out)?;
        }
        self.pending.extend_from_slice(remainder);
        Ok(())
    }

    fn process_chunk(&mut self, chunk: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        if self.resampler.is_none() {
            self.resampler = Some(self.build(STREAM_CHUNK)?);
        }
        let resampler = self.resampler.as_mut().expect("created above");
        let result = resampler
            .process(&[chunk], None)
            .map_err(|e| format!("Resample failed: {e}"))?;
        out.extend_from_slice(&result[0]);
        Ok(())
    }

    /// Flush: append the tail of the signal (the short last chunk and the
    /// filter's flush). The resampler is spent afterwards.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), String> {
        self.finished = true;
        if self.passthrough() {
            return Ok(());
        }
        let remainder = std::mem::take(&mut self.pending);
        let Some(mut resampler) = self.resampler.take() else {
            // Fewer than one chunk in total: the batch code makes the chunk as
            // long as the whole input, processes it, then flushes.
            if remainder.is_empty() {
                return Ok(());
            }
            let mut resampler = self.build(remainder.len())?;
            let result = resampler
                .process(&[remainder.as_slice()], None)
                .map_err(|e| format!("Resample failed: {e}"))?;
            out.extend_from_slice(&result[0]);
            let result = resampler
                .process_partial(None::<&[Vec<f32>]>, None)
                .map_err(|e| format!("Resample flush failed: {e}"))?;
            out.extend_from_slice(&result[0]);
            return Ok(());
        };
        let result = if remainder.is_empty() {
            resampler
                .process_partial(None::<&[Vec<f32>]>, None)
                .map_err(|e| format!("Resample flush failed: {e}"))?
        } else {
            resampler
                .process_partial(Some(&[remainder.as_slice()]), None)
                .map_err(|e| format!("Resample partial failed: {e}"))?
        };
        out.extend_from_slice(&result[0]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- mix_to_mono --

    #[test]
    fn mix_to_mono_passthrough_single_channel() {
        let data = vec![0.1, 0.2, 0.3];
        let result = mix_to_mono(&data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn mix_to_mono_stereo() {
        let data = vec![1.0, 0.0, 0.0, 1.0];
        let result = mix_to_mono(&data, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.5).abs() < 1e-6);
        assert!((result[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mix_to_mono_stereo_identical_channels() {
        let data = vec![0.7, 0.7, -0.3, -0.3];
        let result = mix_to_mono(&data, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.7).abs() < 1e-6);
        assert!((result[1] - (-0.3)).abs() < 1e-6);
    }

    #[test]
    fn mix_to_mono_surround_51() {
        let data = vec![0.6; 6];
        let result = mix_to_mono(&data, 6);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn mix_to_mono_empty() {
        let result = mix_to_mono(&[], 2);
        assert!(result.is_empty());
    }

    // -- resample_to_16khz --

    #[test]
    fn resample_same_rate_passthrough() {
        let data = vec![0.1, 0.2, 0.3, 0.4];
        let result = resample_to_16khz(data.clone(), TARGET_SAMPLE_RATE).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn resample_downsample_from_48khz() {
        // Sinc filter flush adds a small tail (typically <2% extra samples)
        let data: Vec<f32> = (0..48000).map(|i| (i as f32) / 48000.0).collect();
        let result = resample_to_16khz(data, 48_000).unwrap();
        assert!(
            result.len() >= 15999 && result.len() <= 16400,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_downsample_from_44100() {
        let data: Vec<f32> = vec![0.0; 44100];
        let result = resample_to_16khz(data, 44_100).unwrap();
        assert!(
            result.len() >= 15999 && result.len() <= 16400,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_upsample_from_8khz() {
        let data: Vec<f32> = vec![0.5; 8000];
        let result = resample_to_16khz(data, 8_000).unwrap();
        assert!(
            result.len() >= 15999 && result.len() <= 16200,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_empty_input() {
        let result = resample_to_16khz(vec![], 44_100).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn cancellation_during_resampling_stops_before_processing_remaining_chunks() {
        use std::cell::Cell;
        let cancelled = Cell::new(false);
        let reports = Cell::new(0);
        let result = resample_to_16khz_with_control(
            vec![0.25; 44_100],
            44_100,
            Some(&|_| {
                reports.set(reports.get() + 1);
                cancelled.set(true);
            }),
            &|| {
                if cancelled.get() {
                    Err("cancelled in conversion".into())
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.unwrap_err(), "cancelled in conversion");
        assert_eq!(reports.get(), 1);
    }

    #[test]
    fn resample_progress_reports_increasing_fractions() {
        use std::cell::RefCell;
        // 5000 samples at 44.1kHz -> 4 full 1024-chunks: fractions must be
        // non-decreasing and stay inside [0, 1).
        let seen = RefCell::new(Vec::new());
        let result = resample_to_16khz_with_progress(
            vec![0.25; 5000],
            44_100,
            Some(&|frac| seen.borrow_mut().push(frac)),
        )
        .unwrap();
        assert!(!result.is_empty());
        let seen = seen.borrow();
        assert_eq!(seen.len(), 4, "one report per full chunk, got {seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0] <= w[1]),
            "fractions must not move backwards: {seen:?}"
        );
        assert!(
            seen.iter().all(|&f| (0.0..1.0).contains(&f)),
            "fractions stay in [0, 1): {seen:?}"
        );
    }

    #[test]
    fn resample_preserves_values_at_same_rate() {
        let data = vec![-1.0, 0.0, 0.5, 1.0];
        let result = resample_to_16khz(data.clone(), TARGET_SAMPLE_RATE).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn resample_rejects_zero_sample_rate() {
        let data = vec![0.1, 0.2, 0.3];
        let result = resample_to_16khz(data, 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("source sample rate"));
    }

    #[test]
    fn resample_sinc_sine_wave_integrity() {
        let sample_rate = 48_000u32;
        let duration_samples = sample_rate as usize;
        let freq = 440.0f32;
        let data: Vec<f32> = (0..duration_samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect();

        let result = resample_to_16khz(data, sample_rate).unwrap();
        assert!(
            result.len() >= 15999 && result.len() <= 16400,
            "len={}",
            result.len()
        );

        // Signal should have energy (not all zeros)
        let rms: f32 = (result.iter().map(|s| s * s).sum::<f32>() / result.len() as f32).sqrt();
        assert!(
            rms > 0.3,
            "RMS too low: {rms} — resampled signal lost energy"
        );

        // No NaN or Inf values
        assert!(
            result.iter().all(|s| s.is_finite()),
            "Output contains NaN or Inf"
        );
    }

    // -- StreamingResampler --

    /// xorshift64: deterministic pseudo-random numbers without a dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    fn signal(len: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed | 1);
        (0..len)
            .map(|i| {
                (i as f32 * 0.037).sin() * 0.6
                    + ((rng.next() >> 40) as f32 / 16_777_216.0 - 0.5) * 0.2
            })
            .collect()
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|f| f.to_bits()).collect()
    }

    /// Feed `input` in random pieces (including empty ones) and return the whole output.
    fn stream_in_random_pieces(
        input: &[f32],
        rate: u32,
        rng: &mut Rng,
        max_piece: usize,
    ) -> Vec<f32> {
        let mut resampler = StreamingResampler::new(rate);
        let mut out = Vec::new();
        let mut at = 0;
        while at < input.len() {
            let n = rng.below(max_piece + 1).min(input.len() - at);
            resampler.push(&input[at..at + n], &mut out).unwrap();
            at += n;
        }
        resampler.finish(&mut out).unwrap();
        out
    }

    #[test]
    fn streaming_resample_is_bit_identical_to_batch_for_any_chunking() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for rate in [8_000u32, 44_100, 48_000] {
            // Lengths at, around and past the chunk size, with and without a
            // remainder (multiples of 1024 take the `process_partial(None)` path).
            let mut lengths = vec![1024, 1025, 2047, 2048, 3 * 1024, 5000, 12_345];
            lengths.extend((0..2).map(|_| 1024 + rng.below(12_000)));
            for len in lengths {
                let input = signal(len, rng.next());
                let batch = resample_to_16khz(input.clone(), rate).unwrap();
                for max_piece in [1usize, 333, 1500, 9000] {
                    let streamed = stream_in_random_pieces(&input, rate, &mut rng, max_piece);
                    assert_eq!(
                        bits(&streamed),
                        bits(&batch),
                        "rate {rate} len {len} max_piece {max_piece}"
                    );
                }
            }
        }
    }

    #[test]
    fn streaming_resample_matches_batch_for_inputs_shorter_than_one_chunk() {
        let mut rng = Rng(42);
        for rate in [8_000u32, 44_100, 48_000] {
            for len in [1usize, 2, 17, 700, 1023] {
                let input = signal(len, rng.next());
                let batch = resample_to_16khz(input.clone(), rate).unwrap();
                for max_piece in [1usize, 5, 2000] {
                    let streamed = stream_in_random_pieces(&input, rate, &mut rng, max_piece);
                    assert_eq!(bits(&streamed), bits(&batch), "rate {rate} len {len}");
                }
            }
        }
    }

    #[test]
    fn streaming_resample_passes_16khz_through_and_accepts_an_empty_stream() {
        let input = signal(5000, 7);
        let mut rng = Rng(5);
        let streamed = stream_in_random_pieces(&input, TARGET_SAMPLE_RATE, &mut rng, 999);
        assert_eq!(bits(&streamed), bits(&input));
        // An empty stream is fine at any rate, even 0, like the batch code.
        for rate in [0, 16_000, 44_100] {
            let mut out = Vec::new();
            let mut resampler = StreamingResampler::new(rate);
            resampler.push(&[], &mut out).unwrap();
            resampler.finish(&mut out).unwrap();
            assert!(out.is_empty(), "rate {rate}");
        }
    }

    #[test]
    fn streaming_resample_rejects_a_zero_rate_with_samples() {
        let mut out = Vec::new();
        let mut resampler = StreamingResampler::new(0);
        let error = resampler
            .push(&vec![0.1; 2048], &mut out)
            .and_then(|()| resampler.finish(&mut out))
            .unwrap_err();
        assert!(error.contains("source sample rate"), "{error}");
        let mut short = StreamingResampler::new(0);
        short.push(&[0.1, 0.2], &mut out).unwrap();
        assert!(short
            .finish(&mut out)
            .unwrap_err()
            .contains("source sample rate"));
    }

    #[test]
    fn streaming_resample_emits_output_as_input_arrives() {
        // Output must not be held back until the end: after a few chunks some
        // 16 kHz audio is out already.
        let mut resampler = StreamingResampler::new(48_000);
        let mut out = Vec::new();
        resampler
            .push(&vec![0.25; 4 * 1024 + 10], &mut out)
            .unwrap();
        assert!(out.len() >= 1300, "got {}", out.len());
    }

    #[test]
    fn bounded_resample_rejects_before_allocating_and_matches_unbounded() {
        let data: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin()).collect();
        let free = resample_to_16khz_with_control(data.clone(), 8_000, None, &|| Ok(())).unwrap();
        assert!(free.len() > 1024);
        // Limit just below the true output: must error rather than grow.
        let err = resample_to_16khz_bounded(data.clone(), 8_000, None, &|| Ok(()), free.len() - 1);
        assert!(err.is_err(), "limit below the real output must be rejected");
        // An exactly sufficient limit succeeds with bit-identical output.
        let ok = resample_to_16khz_bounded(data, 8_000, None, &|| Ok(()), free.len()).unwrap();
        assert_eq!(
            ok.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
            free.iter().map(|f| f.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn bounded_resample_refuses_oversized_estimate_up_front() {
        let r = resample_to_16khz_bounded(vec![0.0; 44_100], 44_100, None, &|| Ok(()), 100);
        assert!(r.is_err());
    }
}
