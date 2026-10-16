use super::*;

#[cfg(test)]
mod packaged_x64_cpu_tests {
    use super::require_packaged_x64_cpu;

    #[test]
    fn packaged_baseline_requires_every_feature_before_native_model_loading() {
        for bits in 0..16 {
            let features = std::array::from_fn(|index| bits & (1 << index) != 0);
            assert_eq!(
                require_packaged_x64_cpu(Some("avx2"), features).is_ok(),
                bits == 15
            );
            assert!(require_packaged_x64_cpu(None, features).is_ok());
        }
        assert!(require_packaged_x64_cpu(Some("unknown"), [true; 4]).is_err());
    }

    #[test]
    fn unsupported_cpu_error_is_actionable_and_contains_no_model_or_audio_data() {
        let error = require_packaged_x64_cpu(Some("avx2"), [false; 4])
            .unwrap_err()
            .to_string();
        for feature in ["AVX2", "FMA", "F16C", "BMI2", "source build"] {
            assert!(error.contains(feature));
        }
    }
}

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

    #[test]
    fn strips_leaked_timestamp_tokens() {
        assert_eq!(
            strip_timestamp_tokens("<|0.00|> Hej där<|2.40|>"),
            " Hej där"
        );
        assert_eq!(strip_timestamp_tokens("plain text"), "plain text");
        assert_eq!(
            strip_timestamp_tokens("a <|unterminated"),
            "a <|unterminated"
        );
    }

    #[test]
    fn real_timestamps_pass_through_sanitizer() {
        assert_eq!(
            sanitize_segment_bounds(31.2, 44.8, 71.0, 30.0),
            (31.2, 44.8)
        );
        // Padded final window end is clamped to the audio duration.
        assert_eq!(
            sanitize_segment_bounds(60.0, 90.0, 71.0, 58.0),
            (60.0, 71.0)
        );
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

#[cfg(test)]
mod warm_model_cache_tests {
    use super::*;

    fn key(model: WhisperModel, profile: ContextProfile) -> RuntimeKey {
        RuntimeKey { model, profile }
    }

    #[test]
    fn distinguishes_active_cached_and_missing_models() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        assert_eq!(
            model_availability(Some(swedish), Some(english), swedish),
            ModelAvailability::Active
        );
        assert_eq!(
            model_availability(Some(swedish), Some(english), english),
            ModelAvailability::Cached
        );
        assert_eq!(
            model_availability(
                Some(swedish),
                Some(english),
                key(WhisperModel::KbWhisperBase, ContextProfile::TokenAlignment,),
            ),
            ModelAvailability::Missing
        );
    }

    #[test]
    fn caches_two_base_models_within_budget() {
        assert!(can_cache_pair(
            key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention,),
            key(WhisperModel::BaseEn, ContextProfile::FlashAttention),
        ));
    }

    #[test]
    fn refuses_duplicate_or_oversized_cache_pairs() {
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        assert!(!can_cache_pair(english, english));
        assert!(!can_cache_pair(
            key(
                WhisperModel::KbWhisperMedium,
                ContextProfile::FlashAttention,
            ),
            english,
        ));
    }

    #[test]
    fn treats_same_model_with_different_profiles_as_distinct_runtimes() {
        let flash = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let alignment = key(WhisperModel::BaseEn, ContextProfile::TokenAlignment);

        assert_eq!(
            model_availability(Some(flash), None, alignment),
            ModelAvailability::Missing
        );
        assert!(can_cache_pair(flash, alignment));
    }

    #[test]
    fn selects_token_alignment_only_for_diarization() {
        assert_eq!(
            ContextProfile::for_diarization(false),
            ContextProfile::FlashAttention
        );
        assert_eq!(
            ContextProfile::for_diarization(true),
            ContextProfile::TokenAlignment
        );
    }

    #[test]
    fn cached_runtime_switch_preserves_both_warm_payloads() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let mut active_key = Some(swedish);
        let mut active_context = Some("swedish-context");
        let mut active_state = Some("swedish-state");
        let mut cached = Some(CachedWhisperRuntime {
            key: english,
            context: "english-context",
            state: Some("english-state"),
        });

        assert!(activate_cached_runtime(
            english,
            &mut active_key,
            &mut active_context,
            &mut active_state,
            &mut cached,
        ));
        assert_eq!(active_key, Some(english));
        assert_eq!(active_context, Some("english-context"));
        assert_eq!(active_state, Some("english-state"));

        let cached = cached.unwrap();
        assert_eq!(cached.key, swedish);
        assert_eq!(cached.context, "swedish-context");
        assert_eq!(cached.state, Some("swedish-state"));
    }

    #[test]
    fn cache_miss_leaves_active_and_secondary_unchanged() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let mut active_key = Some(swedish);
        let mut active_context = Some("swedish-context");
        let mut active_state = Some("swedish-state");
        let mut cached = Some(CachedWhisperRuntime {
            key: english,
            context: "english-context",
            state: Some("english-state"),
        });

        assert!(!activate_cached_runtime(
            key(WhisperModel::NbWhisperBase, ContextProfile::FlashAttention,),
            &mut active_key,
            &mut active_context,
            &mut active_state,
            &mut cached,
        ));
        assert_eq!(active_key, Some(swedish));
        assert_eq!(active_context, Some("swedish-context"));
        assert_eq!(active_state, Some("swedish-state"));
        assert_eq!(cached.as_ref().unwrap().key, english);
    }
}

#[cfg(all(test, feature = "diarization"))]
mod coverage_text_tests {
    use super::meaningful_coverage_text;

    #[test]
    fn lossy_replacement_is_retained_when_segment_has_meaningful_text() {
        assert_eq!(
            meaningful_coverage_text(b"\xff spoken text"),
            Some("\u{FFFD} spoken text".to_string())
        );
    }

    #[test]
    fn timestamp_only_and_whitespace_segments_are_excluded() {
        assert_eq!(meaningful_coverage_text(b" <|0.00|> <|1.20|> "), None);
        assert_eq!(meaningful_coverage_text(b"   \t\n"), None);
    }
}

#[cfg(all(test, feature = "diarization"))]
mod word_grouping_tests {
    use super::{group_tokens_into_words, words_from_segments, TokenTiming};

    fn tok(text: &str, t_dtw: f64) -> TokenTiming {
        TokenTiming {
            t_dtw,
            text: text.to_string(),
        }
    }

    fn tok_invalid(text: &str) -> TokenTiming {
        TokenTiming {
            t_dtw: -1.0,
            text: text.to_string(),
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(group_tokens_into_words(&[]).is_empty());
    }

    #[test]
    fn single_token_is_one_word() {
        let tokens = vec![tok(" Hello", 1.0)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "Hello");
        assert!((words[0].0 - 1.0).abs() < 1e-9);
        assert!((words[0].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn leading_space_creates_word_boundary() {
        // " Hello" starts word 1, " world" starts word 2
        let tokens = vec![tok(" Hello", 1.0), tok(" world", 2.0)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].2, "Hello");
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn punctuation_attaches_to_prior_word() {
        // "," has no leading space → appends to "Hello"
        let tokens = vec![
            tok(" Hello", 1.0),
            tok(",", 1.2),
            tok(" world", 2.0),
            tok(".", 2.1),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].2, "Hello,");
        assert_eq!(words[1].2, "world.");
    }

    #[test]
    fn multi_token_word_gets_min_max_timing() {
        // Word "un" composed of two tokens at t=1.0 and t=1.5
        let tokens = vec![
            tok(" un", 1.0),
            tok("e", 1.5), // no leading space → appends
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "une");
        assert!((words[0].0 - 1.0).abs() < 1e-9, "start should be min");
        assert!((words[0].1 - 1.5).abs() < 1e-9, "end should be max");
    }

    #[test]
    fn special_tokens_skipped() {
        let tokens = vec![
            tok(" Hello", 1.0),
            tok("[_BEG_]", 0.0),      // bracket special → skip
            tok("<|nospeech|>", 0.5), // angle bracket special → skip
            tok("", 0.0),             // empty → skip
            tok(" world", 2.0),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2, "specials should be skipped");
        assert_eq!(words[0].2, "Hello");
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn invalid_dtw_word_inherits_previous_end() {
        // Word 1: t=1.0, Word 2: no valid DTW → should inherit 1.0
        let tokens = vec![tok(" Hello", 1.0), tok_invalid(" world")];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert!(
            (words[1].0 - 1.0).abs() < 1e-9,
            "start should inherit previous end"
        );
        assert!(
            (words[1].1 - 1.0).abs() < 1e-9,
            "end should inherit previous end"
        );
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn first_content_token_without_leading_space_starts_word() {
        // First token has no leading space but should still create a word
        let tokens = vec![tok("Bonjour", 0.5)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "Bonjour");
        assert!((words[0].0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn start_never_exceeds_end() {
        // Two valid tokens at the same time → start == end
        let tokens = vec![tok(" test", 3.0), tok("ing", 3.0)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert!(words[0].0 <= words[0].1, "start must not exceed end");
    }

    // ---- words_from_segments tests ----

    fn seg(tokens: Vec<TokenTiming>) -> Vec<TokenTiming> {
        tokens
    }

    /// (a) All tokens invalid across all segments → returns empty (bug #1 fix).
    #[test]
    fn words_from_segments_all_invalid_returns_empty() {
        let segs = vec![
            seg(vec![tok_invalid(" Hello"), tok_invalid(" world")]),
            seg(vec![tok_invalid(" foo"), tok_invalid(" bar")]),
        ];
        let result = words_from_segments(&segs);
        assert!(
            result.is_empty(),
            "should be empty when no valid DTW exists, got: {result:?}"
        );
    }

    /// (b) Second segment's first content word has invalid DTW → inherits from
    /// last valid end of segment 1, NOT 0.0 (bug #2 fix).
    /// Asserts monotonic non-decreasing timestamps.
    #[test]
    fn words_from_segments_cross_segment_inheritance_is_monotonic() {
        let segs = vec![
            // Segment 0: one valid word at t=2.0
            seg(vec![tok(" Hello", 2.0)]),
            // Segment 1: first word has no valid DTW, should inherit 2.0 not 0.0
            seg(vec![tok_invalid(" world"), tok(" there", 3.0)]),
        ];
        let result = words_from_segments(&segs);
        // Should have 3 words: "Hello", "world", "there"
        assert_eq!(result.len(), 3, "expected 3 words, got: {result:?}");

        // All timestamps must be monotonic non-decreasing
        for i in 1..result.len() {
            assert!(
                result[i].0 >= result[i - 1].0,
                "start[{i}]={} < start[{}]={} — not monotonic",
                result[i].0,
                i - 1,
                result[i - 1].0
            );
            assert!(
                result[i].1 >= result[i - 1].1,
                "end[{i}]={} < end[{}]={} — not monotonic",
                result[i].1,
                i - 1,
                result[i - 1].1
            );
        }

        // "world" should inherit segment 0's last valid end (2.0), not 0.0
        let world = &result[1];
        assert_eq!(world.2, "world");
        assert!(
            (world.0 - 2.0).abs() < 1e-9,
            "world start should be 2.0 (inherited), got {}",
            world.0
        );
    }

    /// (c) Second segment's first token lacks a leading space → still starts a
    /// NEW word (no cross-segment merge).
    #[test]
    fn words_from_segments_segment_boundary_forces_word_break() {
        let segs = vec![
            // Segment 0: word "Hello" (token without leading space but first of seg)
            seg(vec![tok("Hello", 1.0)]),
            // Segment 1: "world" also lacks a leading space — must NOT merge with "Hello"
            seg(vec![tok("world", 2.0)]),
        ];
        let result = words_from_segments(&segs);
        assert_eq!(
            result.len(),
            2,
            "segment boundary must create word break; got: {result:?}"
        );
        assert_eq!(result[0].2, "Hello");
        assert_eq!(result[1].2, "world");
    }

    /// (d) First segment all-invalid, later segment has valid DTW → any_valid is
    /// true so result is non-empty.
    #[test]
    fn words_from_segments_any_valid_in_later_segment_returns_words() {
        let segs = vec![
            // Segment 0: all invalid
            seg(vec![tok_invalid(" silence")]),
            // Segment 1: valid
            seg(vec![tok(" speech", 5.0)]),
        ];
        let result = words_from_segments(&segs);
        assert!(
            !result.is_empty(),
            "should have words when any segment has valid DTW"
        );
        // "silence" inherits 0.0 (nothing before it), "speech" has t=5.0
        let speech = result
            .iter()
            .find(|w| w.2 == "speech")
            .expect("should have 'speech'");
        assert!(
            (speech.0 - 5.0).abs() < 1e-9,
            "speech start should be 5.0, got {}",
            speech.0
        );
    }
}

#[cfg(test)]
mod progress_callback_tests {
    use super::{
        clamped_progress_callback, parallel_progress_callback, started_progress_callback,
        ParallelProgress,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn file_progress_leaves_zero_at_encode_start_is_monotonic_and_ends_at_100() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let (mut callback, begin) = started_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });
        // Nothing is emitted before the encode-start hook (lock wait, prep).
        assert!(received.lock().unwrap().is_empty());

        // Mirrors the real hook order: encode-start event, then begin().
        let order = Arc::clone(&received);
        let encode_start = || order.lock().unwrap().push(-1);
        encode_start();
        begin();
        assert_eq!(*received.lock().unwrap(), [-1, 1]);
        begin(); // idempotent

        for percentage in [0, 0, 30, 20, 30, 70, 100, 101] {
            callback(percentage);
        }

        let seen = received.lock().unwrap().clone();
        assert_eq!(seen, [-1, 1, 30, 70, 100]);
        assert!(seen[1..].windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(*seen.last().unwrap(), 100);
    }

    #[test]
    fn first_real_callback_emits_the_initial_one_when_no_encode_hook_ran() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let (mut callback, _begin) = started_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });
        callback(0);
        callback(40);
        assert_eq!(*received.lock().unwrap(), [1, 40]);
    }

    #[test]
    fn final_window_progress_is_clamped_to_audio_percentage_range() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let mut callback = clamped_progress_callback(move |percentage| {
            captured.lock().unwrap().push(percentage);
        });

        for percentage in [-1, 0, 99, 100, 101] {
            callback(percentage);
        }

        assert_eq!(*received.lock().unwrap(), [0, 0, 99, 100, 100]);
    }

    #[test]
    fn parallel_progress_is_weighted_and_never_moves_backwards() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        let progress = Arc::new(Mutex::new(ParallelProgress {
            percentages: vec![0, 0],
            weights: vec![1, 3],
            total_weight: 4,
            last_reported: -1,
            callback: Box::new(move |percentage| {
                captured.lock().unwrap().push(percentage);
            }),
        }));
        let mut first = parallel_progress_callback(Arc::clone(&progress), 0);
        let mut second = parallel_progress_callback(progress, 1);

        first(100);
        second(50);
        first(0);
        second(100);
        first(100);

        assert_eq!(*received.lock().unwrap(), [25, 62, 75, 100]);
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
    fn assembled_transcript_discards_no_speech_marked_hallucination() {
        let segments = [TranscriptSegment {
            start: 0.0,
            end: 0.3,
            text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
            avg_logprob: Some(-0.1),
            no_speech_prob: 0.9,
        }];

        assert_eq!(assemble_transcript(&segments), "");
    }

    #[test]
    fn assembled_transcript_discards_no_speech_segment_between_sentences() {
        let segments = [
            TranscriptSegment {
                start: 0.0,
                end: 1.0,
                text: "First.".to_string(),
                avg_logprob: Some(-0.1),
                no_speech_prob: 0.01,
            },
            TranscriptSegment {
                start: 1.0,
                end: 1.3,
                text: "<|nospeech|>-Hej Tack! Tack! Tack!".to_string(),
                avg_logprob: Some(-0.1),
                no_speech_prob: 0.9,
            },
            TranscriptSegment {
                start: 1.3,
                end: 2.0,
                text: "Second.".to_string(),
                avg_logprob: Some(-0.1),
                no_speech_prob: 0.01,
            },
        ];

        assert_eq!(assemble_transcript(&segments), "First. Second.");
    }

    #[test]
    fn assembled_transcript_separates_sentence_ending_segments() {
        let segments = [
            TranscriptSegment {
                start: 0.0,
                end: 1.0,
                text: "First.".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
            TranscriptSegment {
                start: 1.0,
                end: 2.0,
                text: "Second?".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
            TranscriptSegment {
                start: 2.0,
                end: 3.0,
                text: "Third!".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
            TranscriptSegment {
                start: 3.0,
                end: 4.0,
                text: "Fourth".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
        ];

        assert_eq!(
            assemble_transcript(&segments),
            "First. Second? Third! Fourth"
        );
    }

    #[test]
    fn assembled_transcript_preserves_existing_separator_without_double_space() {
        let segments = [
            TranscriptSegment {
                start: 0.0,
                end: 1.0,
                text: "First.".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
            TranscriptSegment {
                start: 1.0,
                end: 2.0,
                text: " Second".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            },
        ];

        assert_eq!(assemble_transcript(&segments), "First. Second");
    }

    #[test]
    fn assembled_transcript_separates_sentences_inside_one_segment() {
        let segments = [TranscriptSegment {
            start: 0.0,
            end: 4.0,
            text: "Ja.Jag kastar att prata svenska.Jag testar att prata svenska.Jag.".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        }];

        assert_eq!(
            assemble_transcript(&segments),
            "Ja. Jag kastar att prata svenska. Jag testar att prata svenska. Jag."
        );
    }

    #[test]
    fn assembled_transcript_does_not_split_decimal_numbers() {
        let segments = [TranscriptSegment {
            start: 0.0,
            end: 1.0,
            text: "Version 1.2 är klar. Nästa steg.".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        }];

        assert_eq!(
            assemble_transcript(&segments),
            "Version 1.2 är klar. Nästa steg."
        );
    }

    #[test]
    fn assembled_transcript_does_not_split_initialisms() {
        let segments = [TranscriptSegment {
            start: 0.0,
            end: 1.0,
            text: "Jag bor i U.S.A.Nästa mening.".to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        }];

        assert_eq!(
            assemble_transcript(&segments),
            "Jag bor i U.S.A. Nästa mening."
        );
    }

    #[test]
    fn overlapping_continuous_speech_is_stitched_without_dropping_words() {
        let chunk_results = vec![
            vec![TranscriptSegment {
                start: 0.0,
                end: 65.0,
                text: " Vi testar sammanhängande tal över chunkgränsen".to_string(),
                avg_logprob: Some(-0.1),
                no_speech_prob: 0.0,
            }],
            vec![TranscriptSegment {
                start: 55.0,
                end: 120.0,
                text: " sammanhängande tal över chunkgränsen utan att kapa ord.".to_string(),
                avg_logprob: Some(-0.1),
                no_speech_prob: 0.0,
            }],
        ];

        let stitched = stitch_chunk_results(chunk_results, 120.0);
        assert_eq!(
            assemble_transcript(&stitched).trim(),
            "Vi testar sammanhängande tal över chunkgränsen utan att kapa ord."
        );
        assert!(stitched.windows(2).all(|pair| pair[0].end <= pair[1].start));
    }

    #[test]
    fn ambiguous_single_word_overlap_is_preserved_instead_of_dropped() {
        let chunk_results = vec![
            vec![TranscriptSegment {
                start: 0.0,
                end: 65.0,
                text: " Jag sa ja".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            }],
            vec![TranscriptSegment {
                start: 55.0,
                end: 120.0,
                text: " ja till förslaget".to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            }],
        ];

        let stitched = stitch_chunk_results(chunk_results, 120.0);
        assert_eq!(
            assemble_transcript(&stitched).trim(),
            "Jag sa ja ja till förslaget"
        );
    }

    #[test]
    fn matching_anchor_replaces_a_differently_decoded_overlap() {
        let chunk_results = vec![
            vec![TranscriptSegment {
                start: 0.0,
                end: 65.0,
                text: " Tidigare text. Där kommer det in på rätt svår. Det har ju också ett mellanskikt där."
                    .to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            }],
            vec![TranscriptSegment {
                start: 55.0,
                end: 120.0,
                text: " Där kommer du in på rätt svår mark. Du har ju också ett mellanskikt där anonymisering fungerar."
                    .to_string(),
                avg_logprob: None,
                no_speech_prob: 0.0,
            }],
        ];

        let stitched = stitch_chunk_results(chunk_results, 120.0);
        assert_eq!(
            assemble_transcript(&stitched).trim(),
            "Tidigare text. Där kommer du in på rätt svår mark. Du har ju också ett mellanskikt där anonymisering fungerar."
        );
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
    fn auto_parallelism_requires_long_beam_audio_and_bounded_model() {
        let long_audio = 10 * 60 * 16_000;
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 5),
            2
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio - 1, WhisperModel::KbWhisperSmall, 5),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 0),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperLarge, 5),
            1
        );
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
    fn segment_bounds_clamp_whisper_window_to_audio() {
        let (start, end) = sanitize_segment_bounds(-1007.28, 30.0, 3.456, 0.0);
        assert_eq!(start, 0.0);
        assert_eq!(end, 3.456);
    }

    #[test]
    fn segment_bounds_are_monotonic_and_finite() {
        assert_eq!(sanitize_segment_bounds(1.0, 2.0, 10.0, 4.0), (4.0, 4.0));
        assert_eq!(
            sanitize_segment_bounds(f64::NAN, f64::INFINITY, 10.0, 4.0),
            (4.0, 4.0)
        );
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

/// Tests for the WP2b hardening: the bounded-wait `ModelBusy` guard and the real
/// abort callback. These exercise only the locking/flag machinery — they need no
/// loaded model, because the state mutex holds `Option<WhisperState>` and can be
/// locked while holding `None`.
#[cfg(test)]
mod warm_state_tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    /// Acquire and hold the (private) state mutex on a background thread, blocking
    /// until `hold` elapses. Returns once the lock is confirmed held, plus the
    /// join handle so the caller can wait for release.
    fn hold_state_lock(backend: &Arc<WhisperBackend>, hold: Duration) -> thread::JoinHandle<()> {
        let holder = Arc::clone(backend);
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _guard = holder.state.lock().unwrap();
            tx.send(()).expect("signal lock acquired");
            thread::sleep(hold);
        });
        rx.recv()
            .expect("background thread should acquire the lock");
        handle
    }

    #[test]
    fn lock_state_bounded_returns_model_busy_when_held_past_grace() {
        let backend = Arc::new(WhisperBackend::new());
        // Hold the lock well past the grace budget.
        let holder = hold_state_lock(&backend, Duration::from_millis(400));

        let start = Instant::now();
        let res = backend.lock_state_bounded(Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(
            matches!(res, Err(DictationError::ModelBusy)),
            "expected ModelBusy while the lock is held past grace, got {res:?}"
        );
        // Must give up near the grace budget, NOT block until the holder releases.
        assert!(
            elapsed < Duration::from_millis(300),
            "should return within ~grace, not block for the full hold; took {elapsed:?}"
        );
        holder.join().unwrap();
    }

    #[test]
    fn lock_state_bounded_acquires_when_lock_freed_within_grace() {
        // Back-to-back safety: a briefly-held lock (released well within grace)
        // must NOT produce a spurious ModelBusy.
        let backend = Arc::new(WhisperBackend::new());
        let holder = hold_state_lock(&backend, Duration::from_millis(60));

        let res = backend.lock_state_bounded(Duration::from_secs(2));
        assert!(
            res.is_ok(),
            "should acquire once the briefly-held lock is released, got {res:?}"
        );
        drop(res); // release before joining
        holder.join().unwrap();
    }

    #[test]
    fn lock_state_bounded_acquires_immediately_when_uncontended() {
        let backend = WhisperBackend::new();
        let res = backend.lock_state_bounded(Duration::from_millis(0));
        assert!(res.is_ok(), "uncontended lock must be acquired immediately");
    }

    #[test]
    fn abort_trampoline_polarity_matches_ggml() {
        // ggml contract: the abort callback returns `true` to ABORT. Verify the
        // trampoline mirrors the AtomicBool and treats null as "don't abort".
        let flag = AtomicBool::new(false);
        let ptr = &flag as *const AtomicBool as *mut c_void;
        unsafe {
            assert!(
                !WhisperBackend::abort_trampoline(ptr),
                "flag=false must NOT abort"
            );
            flag.store(true, Ordering::SeqCst);
            assert!(
                WhisperBackend::abort_trampoline(ptr),
                "flag=true must abort"
            );
            assert!(
                !WhisperBackend::abort_trampoline(std::ptr::null_mut()),
                "null user_data must NOT abort"
            );
        }
    }

    /// Abort-flag ownership: only the call that ACQUIRES the state lock may
    /// touch the flag. A caller that gives up with ModelBusy never owned the
    /// lock, so it must leave a pending abort request (belonging to the current
    /// lock holder's inference) untouched — otherwise it would cancel the abort
    /// of a stuck call and re-introduce the wedge.
    #[test]
    fn model_busy_return_leaves_abort_flag_untouched() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = hold_state_lock(&backend, Duration::from_millis(300));

        // A timed-out caller has requested an abort of the in-flight inference.
        backend.request_abort();
        assert!(backend.abort_flag.load(Ordering::SeqCst));

        // A new call that fails with ModelBusy must NOT clear that request.
        let res = backend.with_warm_state_grace(Duration::from_millis(30), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {res:?}"
        );
        assert!(
            backend.abort_flag.load(Ordering::SeqCst),
            "ModelBusy path must not clear a pending abort request — that would \
             cancel the abort of the still-running inference and re-wedge the app"
        );
        holder.join().unwrap();
    }

    /// Once a call ACQUIRES the state lock, any prior abort request is stale by
    /// definition (its target inference has finished and released the lock —
    /// the flag lifecycle is owned by the lock holder), so it is cleared before
    /// the new inference starts. Exercised via the ModelNotLoaded early-return,
    /// which happens after lock acquisition but needs no model file.
    #[test]
    fn stale_abort_flag_cleared_once_lock_is_owned() {
        let backend = WhisperBackend::new();
        backend.request_abort();
        assert!(backend.abort_flag.load(Ordering::SeqCst));

        // Uncontended: the call acquires the lock, then fails ModelNotLoaded
        // (no context) — but by then it owns the flag and must have cleared it.
        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelNotLoaded)),
            "expected ModelNotLoaded (no context in test env), got {res:?}"
        );
        assert!(
            !backend.abort_flag.load(Ordering::SeqCst),
            "a call that acquired the state lock must clear the stale abort flag \
             before starting its own inference"
        );
    }

    /// A sticky cancel that lands between inference calls is not lost to the
    /// lock-acquisition flag clear: the next call fails without running.
    #[test]
    fn sticky_cancel_survives_lock_acquisition_and_blocks_new_inference() {
        let backend = WhisperBackend::new();
        backend.request_cancel();
        let mut ran = false;
        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| {
            ran = true;
            Ok(())
        });
        assert!(!ran, "no inference may start after a sticky cancel");
        assert!(
            matches!(res, Err(DictationError::Cancelled)),
            "a sticky cancel is the Cancelled error, got {res:?}"
        );
        assert!(backend.is_cancel_requested());
    }

    /// A panic mid-inference poisons the warm-state lock (#359). The next
    /// transcription must get a clean error or result, not a panic, and must
    /// not reuse the interrupted warm state.
    #[test]
    fn warm_state_poisoned_by_a_panic_is_recovered_and_discarded() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = Arc::clone(&backend);
        let panicked = thread::spawn(move || {
            let _state = holder.state.lock().unwrap();
            let _context = holder.context.lock().unwrap();
            panic!("injected panic mid-inference (#359)");
        })
        .join();
        assert!(panicked.is_err());
        assert!(backend.state.is_poisoned() && backend.context.is_poisoned());

        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelNotLoaded)),
            "expected a clean ModelNotLoaded after recovery, got {res:?}"
        );
        assert!(!backend.state.is_poisoned());
        assert!(backend.state.lock().unwrap().is_none());
        assert_eq!(backend.loaded_model(), None);
    }

    /// Pins the documented fail-closed exception (ADR 0005): dictation holds
    /// `load_lock` across inference, so a panic mid-inference poisons it as
    /// well as `state`. Every later dictation then returns the actionable
    /// restart error; it neither panics nor resumes the interrupted runtime.
    #[test]
    fn panic_mid_dictation_keeps_failing_closed_with_the_restart_error() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = Arc::clone(&backend);
        let panicked = thread::spawn(move || {
            let _load = holder.load_lock.lock().unwrap();
            let _state = holder.state.lock().unwrap();
            panic!("injected panic mid-inference (#359)");
        })
        .join();
        assert!(panicked.is_err());
        assert!(backend.load_lock.is_poisoned() && backend.state.is_poisoned());

        for _ in 0..2 {
            let mut timings = DictationTimings::default();
            let result = backend.transcribe_dictation(
                WhisperModel::Base,
                &[0.0; 8_000],
                Language::English,
                &TranscribeOptions::default(),
                &mut timings,
            );
            assert!(
                matches!(result, Err(DictationError::TranscriptionFailed(ref message))
                    if message.contains("restart Sagascript")),
                "expected the fail-closed restart error, got {result:?}"
            );
            assert!(!timings.inference_started);
        }
        assert!(backend.load_lock.is_poisoned(), "fail-closed is sticky");
    }
}
