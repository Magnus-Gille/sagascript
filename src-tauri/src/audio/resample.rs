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
pub fn resample_to_16khz(mono: Vec<f32>, source_rate: u32) -> Vec<f32> {
    if source_rate == TARGET_SAMPLE_RATE || mono.is_empty() {
        return mono;
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
        .expect("Failed to create resampler");

    let mut output = Vec::with_capacity((mono.len() as f64 * ratio) as usize + 1024);

    // Process full chunks (rubato maintains state between calls)
    let full_chunks = mono.len() / chunk_size;
    for i in 0..full_chunks {
        let start = i * chunk_size;
        let chunk = vec![mono[start..start + chunk_size].to_vec()];
        let result = resampler.process(&chunk, None).expect("Resample failed");
        output.extend_from_slice(&result[0]);
    }

    // Process remaining samples with process_partial (handles short final chunk + flush)
    let remaining_start = full_chunks * chunk_size;
    if remaining_start < mono.len() {
        let remainder = vec![mono[remaining_start..].to_vec()];
        let result = resampler
            .process_partial(Some(&remainder), None)
            .expect("Resample partial failed");
        output.extend_from_slice(&result[0]);
    } else {
        let result = resampler
            .process_partial(None::<&[Vec<f32>]>, None)
            .expect("Resample flush failed");
        output.extend_from_slice(&result[0]);
    }

    output
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
        let result = resample_to_16khz(data.clone(), TARGET_SAMPLE_RATE);
        assert_eq!(result, data);
    }

    #[test]
    fn resample_downsample_from_48khz() {
        // Sinc filter flush adds a small tail (typically <2% extra samples)
        let data: Vec<f32> = (0..48000).map(|i| (i as f32) / 48000.0).collect();
        let result = resample_to_16khz(data, 48_000);
        assert!(
            result.len() >= 15999 && result.len() <= 16400,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_downsample_from_44100() {
        let data: Vec<f32> = vec![0.0; 44100];
        let result = resample_to_16khz(data, 44_100);
        assert!(
            result.len() >= 15999 && result.len() <= 16400,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_upsample_from_8khz() {
        let data: Vec<f32> = vec![0.5; 8000];
        let result = resample_to_16khz(data, 8_000);
        assert!(
            result.len() >= 15999 && result.len() <= 16200,
            "len={}",
            result.len()
        );
    }

    #[test]
    fn resample_empty_input() {
        let result = resample_to_16khz(vec![], 44_100);
        assert!(result.is_empty());
    }

    #[test]
    fn resample_preserves_values_at_same_rate() {
        let data = vec![-1.0, 0.0, 0.5, 1.0];
        let result = resample_to_16khz(data.clone(), TARGET_SAMPLE_RATE);
        assert_eq!(result, data);
    }

    #[test]
    fn resample_sinc_sine_wave_integrity() {
        let sample_rate = 48_000u32;
        let duration_samples = sample_rate as usize;
        let freq = 440.0f32;
        let data: Vec<f32> = (0..duration_samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect();

        let result = resample_to_16khz(data, sample_rate);
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
}
