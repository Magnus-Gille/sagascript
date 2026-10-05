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
pub fn resample_to_16khz_bounded(
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
    let headroom = resampler.output_frames_max().saturating_mul(3).saturating_add(1024);
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
            vec![0.25; 44_100], 44_100,
            Some(&|_| { reports.set(reports.get() + 1); cancelled.set(true); }),
            &|| if cancelled.get() { Err("cancelled in conversion".into()) } else { Ok(()) },
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
        assert!(rms > 0.3, "RMS too low: {rms} — resampled signal lost energy");

        // No NaN or Inf values
        assert!(
            result.iter().all(|s| s.is_finite()),
            "Output contains NaN or Inf"
        );
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
