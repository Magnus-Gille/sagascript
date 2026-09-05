//! Small, model-free signal-presence checks for decoded 16 kHz mono audio.

const FRAME_SAMPLES: usize = 320; // 20 ms at 16 kHz
const MIN_FRAME_RMS: f64 = 0.0015;

/// Return whether any 20 ms frame has enough signal energy to transcribe.
///
/// Input is expected to be mono PCM at 16 kHz, matching the core decoder and
/// capture paths. The input is never modified. A final partial frame is
/// normalized by its actual length rather than by a zero-padded frame size.
pub fn has_audio_signal(audio: &[f32]) -> Result<bool, &'static str> {
    if audio.is_empty() {
        return Ok(false);
    }
    if audio.iter().any(|sample| !sample.is_finite()) {
        return Err("audio contains non-finite samples");
    }

    for frame in audio.chunks(FRAME_SAMPLES) {
        let sum_squared = frame
            .iter()
            .map(|&sample| {
                let sample = f64::from(sample);
                sample * sample
            })
            .sum::<f64>();
        let rms = (sum_squared / frame.len() as f64).sqrt();
        if rms >= MIN_FRAME_RMS {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::has_audio_signal;
    use crate::audio::decoder::decode_audio_file;
    use std::path::PathBuf;

    const ZERO_LENGTHS: &[usize] = &[1, 159, 160, 319, 320, 321, 4_000, 8_000, 16_000];
    const NOISE_LENGTHS: &[usize] = &[4_000, 8_000]; // 0.25 s and 0.5 s

    #[test]
    fn silence_is_not_speech_for_supported_frame_boundaries() {
        for &length in ZERO_LENGTHS {
            let audio = vec![0.0; length];
            assert_eq!(has_audio_signal(&audio), Ok(false), "length={length}");
        }
    }

    #[test]
    fn empty_audio_is_not_speech() {
        assert_eq!(has_audio_signal(&[]), Ok(false));
    }

    #[test]
    fn non_finite_samples_are_rejected_even_at_the_tail() {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for length in [1, 159, 320, 321, 4_000] {
                let mut audio = vec![0.0; length];
                audio[length - 1] = invalid;
                assert_eq!(
                    has_audio_signal(&audio),
                    Err("audio contains non-finite samples"),
                    "invalid={invalid:?}, length={length}"
                );
            }
        }
    }

    #[test]
    fn detector_does_not_modify_input() {
        let audio = (0..321)
            .map(|index| (index as f32 / 321.0 * 2.0) - 1.0)
            .collect::<Vec<_>>();
        let original = audio.clone();

        let _ = has_audio_signal(&audio);

        assert_eq!(audio, original);
    }

    #[test]
    fn deterministic_noise_below_floor_is_rejected() {
        for &length in NOISE_LENGTHS {
            for &target_rms in &[0.0001, 0.001] {
                let audio = seeded_uniform_noise(length, target_rms);
                assert_eq!(
                    has_audio_signal(&audio),
                    Ok(false),
                    "length={length}, rms={target_rms}"
                );
            }
        }
    }

    #[test]
    fn deterministic_noise_at_or_above_floor_is_signal() {
        for &length in NOISE_LENGTHS {
            for &target_rms in &[0.003, 0.01] {
                let audio = seeded_uniform_noise(length, target_rms);
                assert_eq!(
                    has_audio_signal(&audio),
                    Ok(true),
                    "length={length}, rms={target_rms}"
                );
            }
        }
    }

    #[test]
    fn partial_frame_rms_uses_actual_sample_count() {
        assert_eq!(has_audio_signal(&[0.002]), Ok(true));
    }

    #[test]
    fn norwegian_fixture_is_detected_at_original_and_attenuated_volume() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-audio/norwegian-short-3s.mp3");
        let audio = decode_audio_file(&path).expect("tracked Norwegian fixture should decode");
        assert!(!audio.is_empty());
        assert!(has_audio_signal(&audio).expect("original fixture should be valid"));

        let attenuated = audio.iter().map(|sample| sample * 0.1).collect::<Vec<_>>();
        assert!(has_audio_signal(&attenuated).expect("attenuated fixture should be valid"));
    }

    #[test]
    fn max_energy_short_speech_crops_pass_at_both_volumes() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../test-audio/norwegian-short-3s.mp3");
        let audio = decode_audio_file(&path).expect("tracked Norwegian fixture should decode");

        for duration_ms in [100, 200, 300] {
            let samples = duration_ms * 16;
            let original_crop = padded_max_energy_crop(&audio, samples);
            let attenuated_crop = original_crop
                .iter()
                .map(|sample| sample * 0.1)
                .collect::<Vec<_>>();
            assert_eq!(
                has_audio_signal(&original_crop),
                Ok(true),
                "{duration_ms} ms original crop rejected"
            );
            assert_eq!(
                has_audio_signal(&attenuated_crop),
                Ok(true),
                "{duration_ms} ms attenuated crop rejected"
            );
        }
    }

    fn seeded_uniform_noise(length: usize, target_rms: f32) -> Vec<f32> {
        let mut state = 0x4d595df4d0f33173_u64;
        let mut noise = Vec::with_capacity(length);
        for _ in 0..length {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let unit = (state >> 11) as f32 / ((1_u64 << 53) as f32);
            noise.push(unit * 2.0 - 1.0);
        }
        let rms =
            (noise.iter().map(|sample| sample * sample).sum::<f32>() / noise.len() as f32).sqrt();
        let scale = target_rms / rms;
        noise.into_iter().map(|sample| sample * scale).collect()
    }

    fn padded_max_energy_crop(audio: &[f32], samples: usize) -> Vec<f32> {
        assert!(samples <= audio.len());

        let mut energy = audio[..samples]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>();
        let mut best_energy = energy;
        let mut best_start = 0;
        for start in 1..=audio.len() - samples {
            energy += audio[start + samples - 1] * audio[start + samples - 1]
                - audio[start - 1] * audio[start - 1];
            if energy > best_energy {
                best_energy = energy;
                best_start = start;
            }
        }

        let mut padded = vec![0.0; 3_200]; // 200 ms of leading context
        padded.extend_from_slice(&audio[best_start..best_start + samples]);
        padded.resize(padded.len() + 3_200, 0.0); // 200 ms of trailing context
        padded
    }
}
