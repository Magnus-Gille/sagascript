use super::*;

/// Pure-function tests for the segment-confidence support (#81): the
/// avg-logprob helper and the `TranscriptSegment` JSON contract. The FFI
/// accessor calls (no_speech_probability, token_data) need a loaded model and
/// are covered by the end-to-end CLI smoke instead.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

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
