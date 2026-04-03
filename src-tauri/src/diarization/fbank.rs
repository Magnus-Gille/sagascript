/// 80-dim log-mel filterbank feature extraction for speaker embedding.
///
/// Matches WeSpeaker ResNet34-LM training configuration:
/// - 16kHz input, 80 mel bins, 25ms window, 10ms hop
/// - Global mean subtraction across all frames
use rustfft::{FftPlanner, num_complex::Complex};

const SAMPLE_RATE: f32 = 16_000.0;
pub const N_FFT: usize = 400; // 25ms window at 16kHz
pub const HOP_LENGTH: usize = 160; // 10ms hop
pub const N_MELS: usize = 80;
const FMIN: f32 = 20.0;
const FMAX: f32 = 7600.0;
const N_FREQS: usize = N_FFT / 2 + 1; // 201

/// Convert frequency in Hz to mel scale.
fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

/// Convert mel value back to Hz.
fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10f32.powf(mel / 2595.0) - 1.0)
}

/// Build an [N_MELS x N_FREQS] triangular mel filterbank matrix.
fn mel_filterbank() -> Vec<[f32; N_FREQS]> {
    let mel_min = hz_to_mel(FMIN);
    let mel_max = hz_to_mel(FMAX);

    // N_MELS + 2 evenly-spaced mel points (includes boundary points)
    let mel_points: Vec<f32> = (0..=(N_MELS + 1))
        .map(|i| mel_min + (mel_max - mel_min) * i as f32 / (N_MELS + 1) as f32)
        .collect();

    // Convert to bin indices in the FFT frequency grid
    let freq_bins: Vec<f32> = mel_points
        .iter()
        .map(|&m| mel_to_hz(m) / SAMPLE_RATE * N_FFT as f32)
        .collect();

    // Build triangular filters
    let mut filters = vec![[0.0f32; N_FREQS]; N_MELS];
    for m in 0..N_MELS {
        let f_left = freq_bins[m];
        let f_center = freq_bins[m + 1];
        let f_right = freq_bins[m + 2];

        for (k, filter_val) in filters[m].iter_mut().enumerate() {
            let k_f = k as f32;
            if k_f >= f_left && k_f <= f_center && f_center > f_left {
                *filter_val = (k_f - f_left) / (f_center - f_left);
            } else if k_f > f_center && k_f <= f_right && f_right > f_center {
                *filter_val = (f_right - k_f) / (f_right - f_center);
            }
        }
    }

    filters
}

/// Compute 80-dim log-mel filterbank features from 16kHz mono f32 audio.
///
/// Returns one feature vector per frame. Frame count = `(audio.len() - N_FFT) / HOP_LENGTH + 1`.
/// Global mean subtraction is applied across all frames per mel bin.
///
/// Returns an empty vec if audio is too short to produce any frames.
pub fn compute_fbank(audio: &[f32]) -> Vec<[f32; N_MELS]> {
    if audio.len() < N_FFT {
        return Vec::new();
    }

    let n_frames = (audio.len() - N_FFT) / HOP_LENGTH + 1;

    // Hann window
    let hann: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (N_FFT - 1) as f32).cos()))
        .collect();

    let filters = mel_filterbank();

    // FFT planner (reused across frames)
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);

    let mut features = vec![[0.0f32; N_MELS]; n_frames];

    for (frame_idx, frame_features) in features.iter_mut().enumerate() {
        let start = frame_idx * HOP_LENGTH;
        let frame = &audio[start..start + N_FFT];

        // Apply Hann window and convert to complex
        let mut buffer: Vec<Complex<f32>> = frame
            .iter()
            .zip(hann.iter())
            .map(|(&s, &w)| Complex::new(s * w, 0.0))
            .collect();

        fft.process(&mut buffer);

        // Power spectrum (one-sided): |X(k)|^2
        let power: Vec<f32> = buffer[..N_FREQS]
            .iter()
            .map(|c| c.norm_sqr())
            .collect();

        // Apply mel filterbank and log
        for m in 0..N_MELS {
            let energy: f32 = filters[m]
                .iter()
                .zip(power.iter())
                .map(|(&f, &p)| f * p)
                .sum();
            frame_features[m] = energy.max(1e-10).ln();
        }
    }

    // Global mean subtraction per mel bin
    if !features.is_empty() {
        let mut mean = [0.0f32; N_MELS];
        for frame in &features {
            for (m, &v) in frame.iter().enumerate() {
                mean[m] += v;
            }
        }
        let n = features.len() as f32;
        for m in &mut mean {
            *m /= n;
        }
        for frame in &mut features {
            for (m, v) in frame.iter_mut().enumerate() {
                *v -= mean[m];
            }
        }
    }

    features
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_matches_expected() {
        // 1 second of audio = 16000 samples → (16000 - 400) / 160 + 1 = 98 frames
        let audio = vec![0.0f32; 16_000];
        let features = compute_fbank(&audio);
        let expected = (16_000 - N_FFT) / HOP_LENGTH + 1;
        assert_eq!(features.len(), expected, "frame count mismatch");
    }

    #[test]
    fn output_has_correct_dimensionality() {
        let audio = vec![0.0f32; 16_000];
        let features = compute_fbank(&audio);
        assert!(!features.is_empty());
        for frame in &features {
            assert_eq!(frame.len(), N_MELS);
        }
    }

    #[test]
    fn silence_has_zero_mean_after_subtraction() {
        // Silence → all identical log values before subtraction → zero after
        let audio = vec![0.0f32; 16_000];
        let features = compute_fbank(&audio);
        for frame in &features {
            for &v in frame.iter() {
                assert!(
                    v.abs() < 1e-4,
                    "silence should have near-zero features after mean subtraction, got {v}"
                );
            }
        }
    }

    #[test]
    fn mean_is_approximately_zero_per_bin() {
        // After global mean subtraction, per-bin mean across all frames should be ~0
        let mut audio = vec![0.0f32; 16_000];
        // Add some variation so the test is non-trivial
        for (i, s) in audio.iter_mut().enumerate() {
            *s = (i as f32 * 0.001).sin() * 0.5;
        }
        let features = compute_fbank(&audio);
        assert!(!features.is_empty());

        let n = features.len() as f32;
        for m in 0..N_MELS {
            let mean: f32 = features.iter().map(|f| f[m]).sum::<f32>() / n;
            assert!(
                mean.abs() < 1e-4,
                "mean for bin {m} should be ~0 after subtraction, got {mean}"
            );
        }
    }

    #[test]
    fn audio_too_short_returns_empty() {
        let audio = vec![0.0f32; N_FFT - 1];
        let features = compute_fbank(&audio);
        assert!(features.is_empty(), "audio shorter than N_FFT should return empty");
    }

    #[test]
    fn exactly_one_frame_at_minimum_length() {
        let audio = vec![0.0f32; N_FFT];
        let features = compute_fbank(&audio);
        assert_eq!(features.len(), 1);
    }

    #[test]
    fn mel_filterbank_has_correct_shape() {
        let fb = mel_filterbank();
        assert_eq!(fb.len(), N_MELS);
        for row in &fb {
            assert_eq!(row.len(), N_FREQS);
        }
    }

    #[test]
    fn mel_filterbank_rows_sum_to_positive() {
        let fb = mel_filterbank();
        for (m, row) in fb.iter().enumerate() {
            let sum: f32 = row.iter().sum();
            assert!(sum > 0.0, "mel filter {m} should have positive sum, got {sum}");
        }
    }

    #[test]
    fn hz_to_mel_monotonic() {
        // Higher frequency → higher mel value
        assert!(hz_to_mel(1000.0) > hz_to_mel(500.0));
        assert!(hz_to_mel(4000.0) > hz_to_mel(1000.0));
    }

    #[test]
    fn mel_hz_roundtrip() {
        for hz in [100.0, 500.0, 1000.0, 4000.0, 8000.0] {
            let roundtrip = mel_to_hz(hz_to_mel(hz));
            assert!(
                (roundtrip - hz).abs() < 0.01,
                "hz→mel→hz roundtrip failed for {hz}: got {roundtrip}"
            );
        }
    }
}
