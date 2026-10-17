use super::*;

#[cfg(test)]
mod decode_flags_tests {
    use super::*;

    #[test]
    fn every_path_decodes_with_timestamps() {
        for segment_timestamps in [false, true] {
            let flags = DecodeFlags::for_path(DecodePath::Text { segment_timestamps });
            assert!(!flags.no_timestamps);
            assert_eq!(flags.token_timestamps, segment_timestamps);
        }
        #[cfg(feature = "diarization")]
        {
            let flags = DecodeFlags::for_path(DecodePath::WordTimestamps);
            assert!(!flags.no_timestamps);
            assert!(flags.token_timestamps);
        }
    }
}

#[cfg(test)]
mod language_detection_preview_tests {
    use super::language_detection_preview;

    #[test]
    fn keeps_short_audio_intact() {
        let audio = vec![0.1; 16_000];
        assert_eq!(language_detection_preview(&audio).len(), audio.len());
    }

    #[test]
    fn chooses_loudest_thirty_second_window() {
        let window = 30 * 16_000;
        let mut audio = vec![0.001; window * 3];
        audio[window..window * 2].fill(0.1);

        let preview = language_detection_preview(&audio);

        assert_eq!(preview.len(), window);
        assert!(preview.iter().all(|&sample| sample == 0.1));
    }
}

/// Moved from the former `segment_confidence_tests` (#371); the module name is kept so
/// the test ids stay stable.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

    #[test]
    fn silent_audio_skips_decoder_without_a_loaded_model() {
        let backend = WhisperBackend::new();
        for language in [Language::Swedish, Language::English] {
            for samples in [4052, 6400, 8000] {
                let audio = vec![0.0; samples];
                assert!(backend
                    .transcribe_live_sync_with_options(
                        &audio,
                        language,
                        &TranscribeOptions::default(),
                        |_| {},
                    )
                    .unwrap()
                    .is_empty());
                assert!(matches!(
                    backend.transcribe_sync_with_options_segments(
                        &audio,
                        language,
                        &TranscribeOptions::default(),
                        |_| {},
                        None,
                    ),
                    Err(DictationError::ModelNotLoaded)
                ));
            }
        }
    }

    #[test]
    fn live_dictation_guard_rejects_empty_and_non_finite_audio_without_model_access() {
        let backend = WhisperBackend::new();
        let options = TranscribeOptions::default();

        let mut empty_timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: true,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };
        assert!(matches!(
            backend.transcribe_live_dictation(
                WhisperModel::Base,
                &[],
                Language::English,
                &options,
                &mut empty_timings,
            ),
            Err(DictationError::NoAudioCaptured)
        ));
        assert_eq!(empty_timings.model_ms, 0.0);
        assert_eq!(empty_timings.inference_ms, 0.0);
        assert!(!empty_timings.model_cached);
        assert!(!empty_timings.model_acquisition_started);
        assert!(!empty_timings.inference_started);
        assert!(empty_timings.model_ready_at.is_none());

        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let audio = [0.0, invalid, 0.0];
            let mut timings = DictationTimings::default();
            assert!(matches!(
                backend.transcribe_live_dictation(
                    WhisperModel::Base,
                    &audio,
                    Language::English,
                    &options,
                    &mut timings,
                ),
                Err(DictationError::TranscriptionFailed(message))
                    if message.contains("non-finite")
            ));
            assert!(!timings.model_acquisition_started);
            assert!(!timings.inference_started);
            assert!(timings.model_ready_at.is_none());
        }
    }

    #[test]
    fn live_silence_resets_timings_and_skips_model_access() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: true,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };

        let result = backend.transcribe_live_dictation(
            WhisperModel::Base,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert_eq!(result.unwrap(), "");
        assert_eq!(backend.loaded_model(), None);
        assert_eq!(timings.model_ms, 0.0);
        assert_eq!(timings.inference_ms, 0.0);
        assert!(!timings.model_cached);
        assert!(!timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
    }

    #[test]
    fn raw_dictation_resets_timings_and_marks_only_model_acquisition_on_failure() {
        let backend = WhisperBackend::new();
        let mut timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: false,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };

        let poisoning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _load = backend.load_lock.lock().unwrap();
            panic!("poison load lock for deterministic timing test");
        }));
        assert!(poisoning.is_err());

        let result = backend.transcribe_dictation(
            WhisperModel::Base,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert!(
            matches!(result, Err(DictationError::TranscriptionFailed(ref message))
            if message.contains("restart Sagascript"))
        );
        assert!(timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert!(timings.model_ms.is_finite());
        assert!(timings.model_ms >= 0.0);
        assert_eq!(timings.inference_ms, 0.0);
    }

    #[test]
    fn warmup_still_requires_a_model_and_does_not_skip_silent_inference() {
        assert!(matches!(
            WhisperBackend::new().warmup(Language::English),
            Err(DictationError::ModelNotLoaded)
        ));
    }

    #[test]
    fn mean_logprob_empty_is_none() {
        assert_eq!(mean_logprob(&[]), None);
    }

    #[test]
    fn mean_logprob_averages() {
        let got = mean_logprob(&[-0.2, -0.4, -0.6]).unwrap();
        assert!((got - (-0.4)).abs() < 1e-6, "got {got}");
    }

    #[test]
    fn mean_logprob_single_value() {
        assert_eq!(mean_logprob(&[-1.5]), Some(-1.5));
    }
}
