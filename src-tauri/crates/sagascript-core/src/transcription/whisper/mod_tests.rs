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

/// Pure-function tests for the segment-confidence support (#81): the
/// avg-logprob helper and the `TranscriptSegment` JSON contract. The FFI
/// accessor calls (no_speech_probability, token_data) need a loaded model and
/// are covered by the end-to-end CLI smoke instead.
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
    fn dictation_timings_keep_private_state_out_of_json_contract() {
        let timings = DictationTimings {
            model_ms: 12.0,
            inference_ms: 34.0,
            model_cached: true,
            model_acquisition_started: true,
            inference_started: true,
            model_ready_at: Some(Instant::now()),
        };
        let json = serde_json::to_value(timings).unwrap();

        assert_eq!(json["model_ms"], 12.0);
        assert_eq!(json["inference_ms"], 34.0);
        assert_eq!(json["model_cached"], true);
        assert!(json.get("model_acquisition_started").is_none());
        assert!(json.get("inference_started").is_none());
        assert!(json.get("model_ready_at").is_none());
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
    fn segment_timestamps_are_opt_in() {
        assert!(!TranscribeOptions::default().segment_timestamps);
    }

    #[test]
    fn parallel_chunking_is_opt_in() {
        assert_eq!(TranscribeOptions::default().parallel_chunks, 1);
    }

    #[test]
    fn diarization_decoder_config_preserves_explicit_beam_and_vad_contract() {
        let options = TranscribeOptions {
            beam_size: 4,
            temperature_fallback: false,
            vad_model_path: Some("/tmp/vad.bin".to_string()),
            ..TranscribeOptions::default()
        };
        let config = effective_diarization_decoder_config(&options).unwrap();
        assert_eq!(config.strategy, "beam_search");
        assert_eq!(config.beam_size, 4);
        assert!(!config.temperature_fallback);
        assert!(config.vad_enabled);
        assert_eq!(config.vad_min_silence_duration_ms, 200);
        assert_eq!(config.vad_speech_pad_ms, 50);
    }

    #[test]
    fn diarization_decoder_rejects_unsupported_beam_width_before_inference() {
        for beam_size in [1, 9, u32::MAX] {
            let options = TranscribeOptions {
                beam_size,
                ..TranscribeOptions::default()
            };
            assert!(validate_diarization_options(&options).is_err());
        }
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

    #[test]
    fn transcript_segment_serializes_confidence_fields() {
        let seg = TranscriptSegment {
            start: 1.25,
            end: 3.5,
            text: " hello".to_string(),
            avg_logprob: Some(-0.42),
            no_speech_prob: 0.01,
        };
        let json = serde_json::to_value(&seg).unwrap();
        assert_eq!(json["start"], 1.25);
        assert_eq!(json["end"], 3.5);
        assert_eq!(json["text"], " hello");
        assert!((json["avg_logprob"].as_f64().unwrap() - (-0.42)).abs() < 1e-6);
        assert!((json["no_speech_prob"].as_f64().unwrap() - 0.01).abs() < 1e-6);
    }

    #[test]
    fn transcript_segment_serializes_missing_logprob_as_null() {
        let seg = TranscriptSegment {
            start: 0.0,
            end: 0.0,
            text: String::new(),
            avg_logprob: None,
            no_speech_prob: 0.9,
        };
        let json = serde_json::to_value(&seg).unwrap();
        assert!(json["avg_logprob"].is_null());
    }
}
