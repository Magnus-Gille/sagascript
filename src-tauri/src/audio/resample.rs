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

/// Nearest-neighbor resample from `source_rate` to `TARGET_SAMPLE_RATE` (16 kHz).
/// Returns the input unchanged if rates already match.
pub fn resample_to_16khz(mono: Vec<f32>, source_rate: u32) -> Vec<f32> {
    if source_rate == TARGET_SAMPLE_RATE {
        return mono;
    }
    let ratio = TARGET_SAMPLE_RATE as f64 / source_rate as f64;
    let out_len = (mono.len() as f64 * ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src_idx = ((i as f64 / ratio) as usize).min(mono.len().saturating_sub(1));
            mono[src_idx]
        })
        .collect()
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
        // L=1.0, R=0.0 → avg 0.5; L=0.0, R=1.0 → avg 0.5
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
        // 6 channels, 1 frame: all 0.6 → avg 0.6
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
        // 48kHz → 16kHz = 1/3 ratio, so 48000 samples → ~16000
        let data: Vec<f32> = (0..48000).map(|i| (i as f32) / 48000.0).collect();
        let result = resample_to_16khz(data, 48_000);
        assert_eq!(result.len(), 16000);
    }

    #[test]
    fn resample_downsample_from_44100() {
        let data: Vec<f32> = vec![0.0; 44100];
        let result = resample_to_16khz(data, 44_100);
        // 44100 * (16000/44100) ≈ 16000
        assert_eq!(result.len(), 16000);
    }

    #[test]
    fn resample_upsample_from_8khz() {
        // 8kHz → 16kHz = 2x, so 8000 samples → 16000
        let data: Vec<f32> = vec![0.5; 8000];
        let result = resample_to_16khz(data, 8_000);
        assert_eq!(result.len(), 16000);
        // All values should still be 0.5 (nearest-neighbor)
        for &s in &result {
            assert!((s - 0.5).abs() < 1e-6);
        }
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
}
