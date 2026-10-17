use super::*;

/// Generous for a wait that is released by an event, never by the clock.
const PATIENT: Duration = Duration::from_secs(30);

fn is_held<T>(mutex: &Mutex<T>) -> bool {
    matches!(mutex.try_lock(), Err(TryLockError::WouldBlock))
}

/// Poll until `mutex` is held by someone else. The poll can take the lock for
/// an instant; the other thread then simply waits that long.
fn wait_until_held<T>(mutex: &Mutex<T>) {
    let failsafe = Instant::now() + PATIENT;
    while !is_held(mutex) {
        assert!(Instant::now() < failsafe, "the lock was never taken");
        assert!(!mutex.is_poisoned(), "lock is poisoned");
        std::thread::yield_now();
    }
}

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

/// More characterisation of the DTW word grouping (#371): the inputs the tests
/// above leave out. Words are never dropped, and a word without a valid DTW
/// time borrows the previous word's end.
#[cfg(all(test, feature = "diarization"))]
mod word_grouping_edge_tests {
    use super::{
        group_tokens_into_words, meaningful_coverage_text, words_from_segments, TokenTiming,
    };

    fn tok(text: &str, t_dtw: f64) -> TokenTiming {
        TokenTiming {
            t_dtw,
            text: text.to_string(),
        }
    }

    fn owned(words: &[(f64, f64, &str)]) -> Vec<(f64, f64, String)> {
        words
            .iter()
            .map(|&(start, end, text)| (start, end, text.to_string()))
            .collect()
    }

    #[test]
    fn a_lone_space_token_makes_an_empty_word_instead_of_being_dropped() {
        assert_eq!(
            group_tokens_into_words(&[tok(" ", 1.0), tok(" hi", 2.0)]),
            owned(&[(1.0, 1.0, ""), (2.0, 2.0, "hi")])
        );
    }

    #[test]
    fn only_a_leading_ascii_space_starts_a_word_and_all_of_them_are_stripped() {
        assert_eq!(
            group_tokens_into_words(&[tok("  hi", 1.0), tok("\tthere", 2.0), tok("\u{a0}x", 3.0)]),
            owned(&[(1.0, 3.0, "hi\tthere\u{a0}x")])
        );
    }

    #[test]
    fn bracketed_tokens_are_special_only_at_the_start_and_angle_tokens_anywhere() {
        assert_eq!(
            group_tokens_into_words(&[
                tok(" a[b]", 1.0),
                tok("[_TT_150]", 1.1),
                tok(" he<|x|>llo", 1.2),
                tok(" c", 2.0),
            ]),
            owned(&[(1.0, 1.0, "a[b]"), (2.0, 2.0, "c")])
        );
    }

    #[test]
    fn a_word_borrows_the_previous_end_when_none_of_its_tokens_has_a_valid_time() {
        assert_eq!(
            group_tokens_into_words(&[
                tok(" first", 0.0),
                tok(" nan", f64::NAN),
                tok(" inf", f64::INFINITY),
                tok(" negative", -0.5),
                tok(" last", 4.0),
            ]),
            owned(&[
                (0.0, 0.0, "first"),
                (0.0, 0.0, "nan"),
                (0.0, 0.0, "inf"),
                (0.0, 0.0, "negative"),
                (4.0, 4.0, "last"),
            ])
        );
        // Nothing valid before it: the borrowed time is zero.
        assert_eq!(
            group_tokens_into_words(&[tok(" early", -1.0)]),
            owned(&[(0.0, 0.0, "early")])
        );
    }

    #[test]
    fn invalid_tokens_inside_a_word_do_not_affect_its_span() {
        assert_eq!(
            group_tokens_into_words(&[
                tok(" ab", -1.0),
                tok("c", 2.5),
                tok("d", f64::NAN),
                tok("e", 2.0)
            ]),
            owned(&[(2.0, 2.5, "abcde")])
        );
    }

    #[test]
    fn grouping_keeps_the_input_order_even_when_times_run_backwards() {
        assert_eq!(
            group_tokens_into_words(&[tok(" late", 5.0), tok(" early", 1.0)]),
            owned(&[(5.0, 5.0, "late"), (1.0, 1.0, "early")])
        );
    }

    #[test]
    fn segments_without_tokens_or_without_content_give_no_words() {
        assert!(words_from_segments(&[]).is_empty());
        assert!(words_from_segments(&[Vec::new(), Vec::new()]).is_empty());
        // A valid time on a special token proves DTW ran, but there is no word.
        assert!(words_from_segments(&[vec![tok("[_BEG_]", 0.0)]]).is_empty());
    }

    #[test]
    fn a_leading_special_token_does_not_use_up_the_segment_boundary() {
        let words = words_from_segments(&[
            vec![tok(" one", 1.0)],
            vec![tok("[_BEG_]", 0.0), tok("two", 2.0), tok("s", 2.1)],
        ]);

        assert_eq!(words, owned(&[(1.0, 1.0, "one"), (2.0, 2.1, "twos")]));
    }

    #[test]
    fn coverage_text_strips_timestamps_inside_the_text_and_trims_only_the_ends() {
        assert_eq!(meaningful_coverage_text(b""), None);
        assert_eq!(
            meaningful_coverage_text(b" a <|0.00|> b "),
            Some("a  b".to_string())
        );
        assert_eq!(
            meaningful_coverage_text(b"caf\xc3"),
            Some("caf\u{FFFD}".to_string())
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
        let res = lock_state_slot_bounded(&backend.state, Duration::from_millis(50));
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

        let res = lock_state_slot_bounded(&backend.state, Duration::from_secs(2));
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
        let res = lock_state_slot_bounded(&backend.state, Duration::from_millis(0));
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

/// Characterisation of the chunk-stitching rules (#371), pinned before the
/// module split. The rules are deliberately conservative: an overlap is only
/// removed when it is long, distinctive and close to the chunk edges.
#[cfg(test)]
mod stitch_characterisation_tests {
    use super::*;

    fn segment(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.to_string(),
            avg_logprob: None,
            no_speech_prob: 0.0,
        }
    }

    fn joined(chunks: Vec<Vec<TranscriptSegment>>, duration: f64) -> String {
        assemble_transcript(&stitch_chunk_results(chunks, duration))
    }

    fn words(count: usize, prefix: &str) -> String {
        (0..count)
            .map(|index| format!(" {prefix}{index}"))
            .collect::<String>()
    }

    #[test]
    fn nothing_to_stitch_gives_nothing() {
        assert!(stitch_chunk_results(Vec::new(), 10.0).is_empty());
        assert!(stitch_chunk_results(vec![Vec::new(), Vec::new()], 10.0).is_empty());
    }

    #[test]
    fn one_chunk_keeps_its_segments_with_bounded_times_and_drops_blank_ones() {
        let stitched = stitch_chunk_results(
            vec![vec![
                segment(-5.0, 2.0, " a"),
                segment(1.0, 3.0, "   "),
                segment(2.5, 99.0, " b"),
            ]],
            10.0,
        );

        let timed: Vec<_> = stitched
            .iter()
            .map(|s| (s.start, s.end, s.text.as_str()))
            .collect();
        assert_eq!(timed, [(0.0, 2.0, " a"), (2.5, 10.0, " b")]);
    }

    #[test]
    fn a_later_chunk_cannot_start_before_the_previous_chunk_ended() {
        let stitched = stitch_chunk_results(
            vec![
                vec![segment(0.0, 50.0, " first part")],
                vec![segment(40.0, 80.0, " second part")],
            ],
            100.0,
        );

        assert_eq!((stitched[1].start, stitched[1].end), (50.0, 80.0));
        assert_eq!(stitched[1].text, " second part");
    }

    #[test]
    fn unrelated_chunks_are_joined_without_trimming() {
        let text = joined(
            vec![
                vec![segment(0.0, 60.0, " the weather was clear")],
                vec![segment(55.0, 120.0, " dinner is at seven")],
            ],
            120.0,
        );

        assert_eq!(text.trim(), "the weather was clear dinner is at seven");
    }

    #[test]
    fn a_three_word_overlap_is_too_short_to_remove() {
        let text = joined(
            vec![
                vec![segment(0.0, 60.0, " we went to the park today")],
                vec![segment(55.0, 120.0, " the park today and then home")],
            ],
            120.0,
        );

        assert_eq!(
            text.trim(),
            "we went to the park today the park today and then home"
        );
    }

    #[test]
    fn the_anchor_needs_twelve_characters_not_just_four_words() {
        let eleven = joined(
            vec![
                vec![segment(0.0, 60.0, " x ab def ghi jkl")],
                vec![segment(55.0, 120.0, " ab def ghi jkl y")],
            ],
            120.0,
        );
        let twelve = joined(
            vec![
                vec![segment(0.0, 60.0, " x abc def ghi jkl")],
                vec![segment(55.0, 120.0, " abc def ghi jkl y")],
            ],
            120.0,
        );

        assert_eq!(eleven.trim(), "x ab def ghi jkl ab def ghi jkl y");
        assert_eq!(twelve.trim(), "x abc def ghi jkl y");
    }

    #[test]
    fn the_previous_tail_after_the_anchor_is_replaced_by_the_next_chunk() {
        let text = joined(
            vec![
                vec![segment(0.0, 60.0, " intro alpha beta gamma delta old tail")],
                vec![segment(55.0, 120.0, " alpha beta gamma delta new tail")],
            ],
            120.0,
        );

        assert_eq!(text.trim(), "intro alpha beta gamma delta new tail");
    }

    #[test]
    fn an_overlap_is_only_accepted_within_thirty_two_unanchored_words_of_each_edge() {
        let anchor = " alpha beta gamma delta";
        let trailing = |count| {
            vec![
                vec![segment(
                    0.0,
                    60.0,
                    &format!("{anchor}{}", words(count, "t")),
                )],
                vec![segment(55.0, 120.0, &format!("{anchor} next"))],
            ]
        };
        let leading = |count| {
            vec![
                vec![segment(
                    0.0,
                    60.0,
                    &format!("{}{anchor} prev", words(33, "p")),
                )],
                vec![segment(
                    55.0,
                    120.0,
                    &format!("{}{anchor}", words(count, "l")),
                )],
            ]
        };

        // 32 words after the anchor in the previous chunk: still replaced.
        assert_eq!(
            joined(trailing(32), 120.0).trim(),
            "alpha beta gamma delta next"
        );
        // 33: the overlap is not trusted and both chunks are kept whole.
        let kept = joined(trailing(33), 120.0);
        assert!(kept.contains("t32") && kept.ends_with("alpha beta gamma delta next"));

        // The same limit applies to words before the anchor in the next chunk:
        // 32 are accepted (the previous chunk is cut where the next one begins).
        let expected = format!("p0{}{anchor}", words(32, "l"));
        assert_eq!(joined(leading(32), 120.0).trim(), expected);
        // 33 are not.
        let kept = joined(leading(33), 120.0);
        assert!(kept.contains("p32 alpha beta gamma delta prev"));
        assert!(kept.trim_end().ends_with("l32 alpha beta gamma delta"));
    }

    #[test]
    fn the_longest_anchor_at_the_earliest_position_in_the_next_chunk_wins() {
        let text = joined(
            vec![
                vec![segment(
                    0.0,
                    60.0,
                    " one two three four five six one two three four",
                )],
                vec![segment(55.0, 120.0, " one two three four five six seven")],
            ],
            120.0,
        );

        // Both alignments start at the first word of the next chunk; the
        // six-word anchor beats the later four-word one, so the whole
        // previous chunk is the overlap.
        assert_eq!(text.trim(), "one two three four five six seven");
    }

    #[test]
    fn an_overlap_is_trimmed_across_segment_boundaries_of_the_previous_chunk() {
        let text = joined(
            vec![
                vec![
                    segment(0.0, 30.0, " start of speech"),
                    segment(30.0, 60.0, " alpha beta"),
                    segment(60.0, 65.0, " gamma delta"),
                ],
                vec![segment(55.0, 120.0, " alpha beta gamma delta end")],
            ],
            120.0,
        );

        assert_eq!(text.trim(), "start of speech alpha beta gamma delta end");
    }

    #[test]
    fn suffix_trimming_keeps_whole_segments_before_the_cut_and_cuts_inside_one() {
        let make = || vec![segment(0.0, 1.0, "abc def "), segment(1.0, 2.0, "ghi jkl")];

        let mut at_boundary = make();
        trim_segment_suffix(&mut at_boundary, 8);
        assert_eq!(texts(&at_boundary), ["abc def"]);

        let mut inside_second = make();
        trim_segment_suffix(&mut inside_second, 9);
        assert_eq!(texts(&inside_second), ["abc def ", "g"]);

        let mut at_start = make();
        trim_segment_suffix(&mut at_start, 0);
        assert!(at_start.is_empty());

        let mut past_the_end = make();
        trim_segment_suffix(&mut past_the_end, 99);
        assert_eq!(texts(&past_the_end), ["abc def ", "ghi jkl"]);
    }

    fn texts(segments: &[TranscriptSegment]) -> Vec<&str> {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    #[test]
    fn stitch_words_lowercases_and_keeps_inner_apostrophes_and_hyphens() {
        let found = stitch_words("Don't stop-me, ÅÄÖ  “quoted” 'x' it’s- ok");

        let normalized: Vec<_> = found.iter().map(|w| w.normalized.as_str()).collect();
        assert_eq!(
            normalized,
            ["don't", "stop-me", "åäö", "quoted", "x", "it’s", "ok"]
        );
        assert_eq!(found[0].start, 0);
        assert_eq!(found[1].start, 6);
        assert!(stitch_words("... ,, --").is_empty());
    }

    #[test]
    fn overlap_start_is_the_byte_offset_of_the_first_repeated_word() {
        let previous = "alpha beta gamma delta epsilon zeta";

        assert_eq!(
            overlapping_suffix_start(previous, "gamma delta epsilon zeta eta"),
            Some(11)
        );
        assert_eq!(
            overlapping_suffix_start(previous, "GAMMA, Delta; epsilon. zeta"),
            Some(11),
            "matching ignores case and punctuation"
        );
        assert_eq!(
            overlapping_suffix_start(previous, "nothing in common here"),
            None
        );
        assert_eq!(
            overlapping_suffix_start("", "gamma delta epsilon zeta"),
            None
        );
    }

    #[test]
    fn plain_transcript_joins_segments_exactly_as_whisper_spaced_them() {
        assert_eq!(assemble_transcript(&[]), "");
        let glued = [segment(0.0, 1.0, "Hello"), segment(1.0, 2.0, "world")];
        assert_eq!(assemble_transcript(&glued), "Helloworld");
        let spaced = [segment(0.0, 1.0, "Hello"), segment(1.0, 2.0, " world")];
        assert_eq!(assemble_transcript(&spaced), "Hello world");
        assert!(contains_no_speech_marker("x<|nospeech|>y"));
        assert!(!contains_no_speech_marker("<|nospeech"));
    }

    #[test]
    fn segment_bounds_never_leave_the_audio_or_run_backwards() {
        assert_eq!(sanitize_segment_bounds(1.0, 2.0, -3.0, 0.0), (0.0, 0.0));
        assert_eq!(sanitize_segment_bounds(1.0, 2.0, f64::NAN, 0.0), (0.0, 0.0));
        assert_eq!(sanitize_segment_bounds(1.0, 2.0, 10.0, 99.0), (10.0, 10.0));
        assert_eq!(sanitize_segment_bounds(5.0, 3.0, 10.0, 0.0), (5.0, 5.0));
        assert_eq!(
            sanitize_segment_bounds(-4.0, f64::NAN, 10.0, 2.0),
            (2.0, 2.0)
        );
    }
}

/// Characterisation of the parallel-chunk limits (#371). Chunk planning itself
/// is tested in `chunking.rs`; this pins the numbers this module feeds it and
/// the progress weights it derives from the plan.
#[cfg(test)]
mod parallel_limit_tests {
    use super::*;
    use crate::transcription::chunking::plan_chunks;

    const SECOND: usize = 16_000;

    fn planned(seconds: usize, requested: usize) -> Vec<AudioChunk> {
        let audio = vec![0.25; seconds * SECOND];
        plan_chunks(
            &audio,
            requested.clamp(1, MAX_PARALLEL_CHUNKS),
            MIN_PARALLEL_CHUNK_SAMPLES,
        )
    }

    #[test]
    fn the_limits_are_four_states_and_one_minute_pieces() {
        assert_eq!(MAX_PARALLEL_CHUNKS, 4);
        assert_eq!(MIN_PARALLEL_CHUNK_SAMPLES, 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MIN_SAMPLES, 10 * 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MAX_MODEL_MB, 500);
    }

    #[test]
    fn a_recording_is_never_split_into_pieces_shorter_than_a_minute() {
        assert_eq!(planned(59, 4).len(), 1);
        assert_eq!(planned(119, 4).len(), 1);
        assert_eq!(planned(120, 4).len(), 2);
        assert_eq!(planned(300, 4).len(), 4);
    }

    #[test]
    fn requests_of_zero_or_above_the_maximum_are_clamped() {
        assert_eq!(planned(600, 0).len(), 1);
        assert_eq!(planned(600, 1).len(), 1);
        assert_eq!(planned(600, 99).len(), MAX_PARALLEL_CHUNKS);
    }

    #[test]
    fn automatic_parallelism_starts_with_beam_two_and_never_asks_for_more_than_two() {
        let long = AUTO_PARALLEL_MIN_SAMPLES;
        let small = WhisperModel::KbWhisperSmall;
        assert_eq!(recommended_parallel_chunks(long, small, 1), 1);
        assert_eq!(recommended_parallel_chunks(long, small, 2), 2);
        assert_eq!(recommended_parallel_chunks(usize::MAX, small, 8), 2);
        assert!(recommended_parallel_chunks(usize::MAX, small, u32::MAX) <= MAX_PARALLEL_CHUNKS);
    }
}

/// Characterisation of the load lock and the abort and cancel flags (#371),
/// pinned before `whisper_backend.rs` is split. Nothing here needs model
/// weights: a runtime key can be published without a context, and every
/// assertion stops at the lock, flag or error that comes before FFI.
#[cfg(test)]
mod lock_protocol_tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    const MODEL: WhisperModel = WhisperModel::Base;

    fn key(profile: ContextProfile) -> RuntimeKey {
        RuntimeKey {
            model: MODEL,
            profile,
        }
    }

    /// A backend whose active runtime is published without a context, so
    /// `ensure_model*` sees the model as already active and never reads a
    /// model file.
    fn backend_with_active_runtime() -> Arc<WhisperBackend> {
        let backend = Arc::new(WhisperBackend::new());
        *backend.loaded_runtime.lock().unwrap() = Some(key(ContextProfile::FlashAttention));
        backend
    }

    /// Hold `load_lock` on a background thread, as a transcription in flight
    /// does, until the returned sender fires or is dropped. The lock is
    /// confirmed held before this returns.
    fn hold_load_lock(backend: &Arc<WhisperBackend>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let holder = Arc::clone(backend);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _load = holder.load_lock.lock().unwrap();
            locked_tx.send(()).expect("signal the lock is held");
            let _ = release_rx.recv();
        });
        locked_rx.recv().expect("holder should acquire load_lock");
        (release_tx, handle)
    }

    fn poison_lock<T: Send + 'static>(
        backend: &Arc<WhisperBackend>,
        pick: fn(&WhisperBackend) -> &Mutex<T>,
    ) {
        let holder = Arc::clone(backend);
        let panicked = thread::spawn(move || {
            let _guard = pick(&holder).lock().unwrap();
            panic!("injected panic to poison a whisper lock");
        })
        .join();
        assert!(panicked.is_err());
    }

    #[test]
    fn the_bounded_wait_budget_is_three_seconds_polled_in_short_steps() {
        assert_eq!(WARM_STATE_GRACE, Duration::from_secs(3));
        assert_eq!(WARM_STATE_POLL_INTERVAL, Duration::from_millis(25));
    }

    #[test]
    fn abort_request_flips_the_flag_the_ffi_trampoline_reads() {
        let backend = WhisperBackend::new();
        let data = Arc::as_ptr(&backend.abort_flag) as *mut c_void;

        // SAFETY: `data` points at the backend's own flag, which outlives these calls.
        assert!(!unsafe { WhisperBackend::abort_trampoline(data) });
        backend.request_abort();
        assert!(unsafe { WhisperBackend::abort_trampoline(data) });
        assert!(
            !backend.is_cancel_requested(),
            "an abort alone is not a sticky cancel"
        );
    }

    #[test]
    fn cancel_request_aborts_now_and_stays_requested() {
        let backend = WhisperBackend::new();
        assert!(!backend.is_cancel_requested());

        backend.request_cancel();

        assert!(backend.is_cancel_requested());
        assert!(backend.abort_flag.load(Ordering::SeqCst));
        // Reading the flag never clears either of them.
        assert!(backend.is_cancel_requested());
        assert!(backend.abort_flag.load(Ordering::SeqCst));
    }

    /// `ensure_model*`, `load_model*`, `with_model` and `transcribe_dictation`
    /// all wait on `load_lock` for the same bounded budget and then report
    /// `ModelBusy`. They run in parallel (one backend each), so the three
    /// budget-length tests below cost about one budget in wall time.
    #[test]
    fn ensure_model_reports_model_busy_once_the_budget_expires_while_a_load_is_in_flight() {
        let backend = backend_with_active_runtime();
        let (release, holder) = hold_load_lock(&backend);

        let started = Instant::now();
        let result = backend.ensure_model_with_profile(MODEL, ContextProfile::FlashAttention);
        let waited = started.elapsed();

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert!(waited >= WARM_STATE_GRACE, "gave up early after {waited:?}");
        release.send(()).unwrap();
        holder.join().unwrap();

        // Once the holder is gone the already-active runtime is accepted
        // without loading anything.
        backend
            .ensure_model_with_profile(MODEL, ContextProfile::FlashAttention)
            .expect("an active runtime needs no load");
    }

    #[test]
    fn load_model_reports_model_busy_before_touching_any_model_file() {
        let backend = Arc::new(WhisperBackend::new());
        let (release, holder) = hold_load_lock(&backend);

        let result = backend.load_model_with_profile(MODEL, ContextProfile::FlashAttention);

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert_eq!(
            backend.loaded_model(),
            None,
            "a refused load changes nothing"
        );
        release.send(()).unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn dictation_reports_model_busy_and_marks_only_model_acquisition_as_started() {
        let backend = backend_with_active_runtime();
        let (release, holder) = hold_load_lock(&backend);
        let mut timings = DictationTimings::default();

        let result = backend.transcribe_dictation(
            MODEL,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert!(timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert!(timings.model_ms >= WARM_STATE_GRACE.as_secs_f64() * 1000.0);
        assert_eq!(timings.inference_ms, 0.0);
        release.send(()).unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn with_model_keeps_the_load_lock_for_the_callback_and_releases_it_afterwards() {
        let backend = backend_with_active_runtime();

        let value = backend
            .with_model(MODEL, ContextProfile::FlashAttention, |pinned| {
                assert!(
                    matches!(pinned.load_lock.try_lock(), Err(TryLockError::WouldBlock)),
                    "the model selection stays pinned while the callback runs"
                );
                Ok(42)
            })
            .unwrap();
        assert_eq!(value, 42);
        assert!(
            backend.load_lock.try_lock().is_ok(),
            "released after the callback"
        );

        let failed: Result<(), _> =
            backend.with_model(MODEL, ContextProfile::FlashAttention, |_| {
                Err(DictationError::NoAudioCaptured)
            });
        assert!(matches!(failed, Err(DictationError::NoAudioCaptured)));
        assert!(
            backend.load_lock.try_lock().is_ok(),
            "released after an error"
        );
    }

    #[test]
    fn a_poisoned_load_lock_refuses_every_entry_that_selects_a_model() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.load_lock);
        let mut callback_ran = false;

        let results = [
            backend.ensure_model_with_profile(MODEL, ContextProfile::FlashAttention),
            backend.load_model_with_profile(MODEL, ContextProfile::FlashAttention),
            backend.with_model(MODEL, ContextProfile::FlashAttention, |_| {
                callback_ran = true;
                Ok(())
            }),
        ];

        assert!(
            !callback_ran,
            "no callback may run on an interrupted engine"
        );
        for result in results {
            assert!(
                matches!(&result, Err(DictationError::TranscriptionFailed(message))
                    if message.contains("restart Sagascript")),
                "expected the fail-closed restart error, got {result:?}"
            );
        }
        assert!(backend.load_lock.is_poisoned(), "fail-closed is sticky");
    }

    #[test]
    fn the_active_runtime_answers_the_lookups_without_a_context() {
        let backend = backend_with_active_runtime();

        assert_eq!(backend.loaded_model(), Some(MODEL));
        assert_eq!(
            backend.loaded_context_profile(),
            Some(ContextProfile::FlashAttention)
        );
        assert_eq!(backend.resident_models(), vec![MODEL]);
        assert!(!backend.needs_reload(MODEL));
        assert!(backend.needs_reload_with_profile(MODEL, ContextProfile::TokenAlignment));
        assert!(backend.needs_reload(WhisperModel::BaseEn));
    }

    #[test]
    fn an_empty_backend_has_nothing_loaded_and_everything_needs_a_load() {
        let backend = WhisperBackend::new();

        assert_eq!(backend.loaded_model(), None);
        assert_eq!(backend.loaded_context_profile(), None);
        assert!(backend.resident_models().is_empty());
        assert!(backend.needs_reload(MODEL));
    }

    #[test]
    fn dictation_with_an_active_runtime_marks_the_model_cached_before_it_fails_without_a_context() {
        let backend = backend_with_active_runtime();
        let mut timings = DictationTimings::default();

        let result = backend.transcribe_dictation(
            MODEL,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(timings.model_cached, "the active runtime counts as cached");
        assert!(timings.model_acquisition_started);
        assert!(timings.inference_started);
        assert!(timings.model_ready_at.is_some());
        assert!(timings.model_ms.is_finite() && timings.model_ms >= 0.0);
        assert!(timings.inference_ms.is_finite() && timings.inference_ms >= 0.0);
    }

    #[test]
    fn language_detection_checks_audio_before_the_context() {
        let backend = backend_with_active_runtime();

        assert!(matches!(
            backend.detect_language(&[]),
            Err(DictationError::NoAudioCaptured)
        ));
        assert!(matches!(
            backend.detect_language(&[0.1; 16_000]),
            Err(DictationError::ModelNotLoaded)
        ));
    }

    /// ADR 0005: `loaded_runtime` holds one `Option` that is only replaced
    /// whole, so a poisoned lock is recovered as it is, not cleared.
    #[test]
    fn a_poisoned_runtime_identity_is_recovered_as_it_is() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.loaded_runtime);
        assert!(backend.loaded_runtime.is_poisoned());

        assert_eq!(backend.loaded_model(), Some(MODEL));
        assert!(
            !backend.loaded_runtime.is_poisoned(),
            "reported once, then cleared"
        );
        assert!(!backend.needs_reload(MODEL));
    }

    #[test]
    fn a_poisoned_context_is_recovered_and_still_reports_no_model() {
        let backend = Arc::new(WhisperBackend::new());
        poison_lock(&backend, |backend| &backend.context);

        assert!(matches!(
            backend.detect_language(&[0.1; 16_000]),
            Err(DictationError::ModelNotLoaded)
        ));
        assert!(!backend.context.is_poisoned());
    }

    #[test]
    fn a_poisoned_secondary_runtime_slot_is_recovered_as_empty() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.cached_runtime);

        assert_eq!(backend.resident_models(), vec![MODEL]);
        assert!(!backend.cached_runtime.is_poisoned());
    }
}

/// Characterisation of the warm-state protocol with a fake state (#371): abort
/// during inference, the timeout path, a sticky cancel, a busy caller and a
/// panic, all through `with_state_slot`, the code `with_warm_state` runs on
/// the native `WhisperState`. The fake inference polls the same trampoline
/// whisper.cpp polls before every compute step.
#[cfg(test)]
mod warm_state_protocol_tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::thread;

    #[derive(Debug, PartialEq)]
    struct FakeState(u32);

    /// A backend, a state slot and a counter of how many fake states were built.
    struct Rig {
        backend: WhisperBackend,
        slot: Mutex<Option<FakeState>>,
        created: AtomicU32,
    }

    impl Rig {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                backend: WhisperBackend::new(),
                slot: Mutex::new(None),
                created: AtomicU32::new(0),
            })
        }

        fn create(&self) -> Result<FakeState, DictationError> {
            Ok(FakeState(self.created.fetch_add(1, Ordering::SeqCst) + 1))
        }

        fn created(&self) -> u32 {
            self.created.load(Ordering::SeqCst)
        }

        fn run<R>(
            &self,
            grace: Duration,
            f: impl FnOnce(&mut FakeState) -> Result<R, DictationError>,
        ) -> Result<R, DictationError> {
            self.backend
                .with_state_slot(&self.slot, grace, || self.create(), f)
        }

        fn state(&self) -> Option<u32> {
            self.slot.lock().unwrap().as_ref().map(|state| state.0)
        }

        fn abort_pending(&self) -> bool {
            self.backend.abort_flag.load(Ordering::SeqCst)
        }

        /// An inference that, like `full()`, runs until the abort callback says
        /// stop. Signals `started` once it is running.
        fn inference_until_aborted(
            &self,
            started: &mpsc::Sender<()>,
        ) -> Result<(), DictationError> {
            let flag = Arc::as_ptr(&self.backend.abort_flag) as *mut c_void;
            started.send(()).expect("signal inference started");
            let failsafe = Instant::now() + PATIENT;
            loop {
                // SAFETY: `flag` points at the backend's own flag, which outlives this call.
                if unsafe { WhisperBackend::abort_trampoline(flag) } {
                    return Err(DictationError::TranscriptionFailed(
                        "Whisper inference failed: aborted".to_string(),
                    ));
                }
                assert!(Instant::now() < failsafe, "the abort never arrived");
                thread::yield_now();
            }
        }

        /// Run `inference_until_aborted` on a background thread and return once
        /// it is running and holds the state lock.
        fn start_inference(self: &Arc<Self>) -> thread::JoinHandle<Result<(), DictationError>> {
            let rig = Arc::clone(self);
            let (started_tx, started_rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                rig.run(PATIENT, |_state| rig.inference_until_aborted(&started_tx))
            });
            started_rx.recv().expect("inference should start");
            handle
        }
    }

    fn failed_inference(result: &Result<(), DictationError>) -> bool {
        matches!(result, Err(DictationError::TranscriptionFailed(message))
            if message.contains("Whisper inference failed"))
    }

    #[test]
    fn state_is_built_on_first_use_and_reused_by_later_calls() {
        let rig = Rig::new();

        let first = rig.run(PATIENT, |state| Ok(state.0)).unwrap();
        let second = rig.run(PATIENT, |state| Ok(state.0)).unwrap();

        assert_eq!((first, second), (1, 1));
        assert_eq!(rig.created(), 1);
        assert_eq!(rig.state(), Some(1));
    }

    #[test]
    fn a_failed_state_build_skips_the_closure_and_leaves_the_slot_empty() {
        let rig = Rig::new();
        let mut ran = false;

        let result = rig.backend.with_state_slot(
            &rig.slot,
            PATIENT,
            || Err::<FakeState, _>(DictationError::ModelNotLoaded),
            |_| {
                ran = true;
                Ok(())
            },
        );

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(!ran);
        assert_eq!(rig.state(), None);
    }

    #[test]
    fn an_abort_during_inference_ends_it_and_discards_the_warm_state() {
        let rig = Rig::new();
        let inference = rig.start_inference();

        rig.backend.request_abort();
        let result = inference.join().unwrap();

        assert!(failed_inference(&result), "got {result:?}");
        assert_eq!(
            rig.state(),
            None,
            "an aborted compute leaves state unusable"
        );
        assert!(!rig.abort_pending(), "the lock holder clears its own abort");
        // The next transcription starts from a clean slate.
        assert_eq!(rig.run(PATIENT, |state| Ok(state.0)).unwrap(), 2);
        assert_eq!(rig.created(), 2);
    }

    #[test]
    fn a_stale_abort_does_not_stop_the_next_inference_or_cost_it_its_state() {
        let rig = Rig::new();
        rig.run(PATIENT, |_| Ok(())).unwrap();
        rig.backend.request_abort();
        let flag = Arc::as_ptr(&rig.backend.abort_flag) as *mut c_void;

        let aborted_inside = rig
            .run(PATIENT, |_| {
                // SAFETY: `flag` points at the backend's own flag.
                Ok(unsafe { WhisperBackend::abort_trampoline(flag) })
            })
            .unwrap();

        assert!(
            !aborted_inside,
            "the previous call's abort is stale once the lock is owned"
        );
        assert_eq!(rig.state(), Some(1), "so the warm state is kept");
        assert_eq!(rig.created(), 1);
    }

    /// The timeout path (WP2b): a caller that gives up with `ModelBusy` must not
    /// disturb the stuck inference; the timeout handler's abort then frees the
    /// lock, and the next call runs on a rebuilt state with a clear flag.
    #[test]
    fn a_timeout_abort_unwedges_the_pipeline() {
        let rig = Rig::new();
        let stuck = rig.start_inference();

        let mut ran = false;
        let busy = rig.run(Duration::from_millis(30), |_| {
            ran = true;
            Ok(())
        });
        assert!(
            matches!(busy, Err(DictationError::ModelBusy)),
            "got {busy:?}"
        );
        assert!(!ran);
        assert!(!rig.abort_pending(), "a busy caller touches nothing");
        assert_eq!(rig.created(), 1, "nor does it build a second state");

        // What the timeout handler does.
        rig.backend.request_abort();
        let rebuilt = rig
            .run(PATIENT, |state| Ok((state.0, rig.abort_pending())))
            .unwrap();

        assert_eq!(rebuilt, (2, false), "fresh state, clear flag");
        assert!(failed_inference(&stuck.join().unwrap()));
    }

    #[test]
    fn a_cancel_during_inference_aborts_it_and_every_later_call_is_refused() {
        let rig = Rig::new();
        let inference = rig.start_inference();

        rig.backend.request_cancel();
        let result = inference.join().unwrap();

        assert!(failed_inference(&result), "got {result:?}");
        assert_eq!(rig.state(), None);
        let mut ran = false;
        let later = rig.run(PATIENT, |_| {
            ran = true;
            Ok(())
        });
        assert!(
            matches!(later, Err(DictationError::Cancelled)),
            "got {later:?}"
        );
        assert!(!ran);
        assert_eq!(rig.created(), 1, "nothing is rebuilt for a cancelled run");
        assert!(rig.backend.is_cancel_requested());
    }

    #[test]
    fn a_sticky_cancel_refuses_new_inference_but_keeps_the_warm_state() {
        let rig = Rig::new();
        rig.run(PATIENT, |_| Ok(())).unwrap();
        rig.backend.request_cancel();

        let later = rig.run(PATIENT, |_| Ok(()));

        assert!(
            matches!(later, Err(DictationError::Cancelled)),
            "got {later:?}"
        );
        assert_eq!(
            rig.state(),
            Some(1),
            "a refused call does not discard the state"
        );
        assert!(
            !rig.abort_pending(),
            "the lock owner still cleared the flag"
        );
        assert!(rig.backend.is_cancel_requested());
    }

    #[test]
    fn a_panic_during_inference_poisons_the_slot_and_the_next_call_gets_a_fresh_state() {
        let rig = Rig::new();
        let panicking = Arc::clone(&rig);
        let outcome = thread::spawn(move || {
            panicking.run(PATIENT, |_| -> Result<(), DictationError> {
                panic!("injected panic mid-inference (#359)")
            })
        })
        .join();
        assert!(outcome.is_err());
        assert!(rig.slot.is_poisoned());
        assert_eq!(rig.created(), 1);

        let next = rig.run(PATIENT, |state| Ok(state.0)).unwrap();

        assert_eq!(next, 2, "the interrupted state is not reused");
        assert!(!rig.slot.is_poisoned());
    }

    /// ARCHITECTURE.md lock order: state, then context. Creating the state
    /// takes the context while the state lock is held.
    #[test]
    fn the_backend_takes_the_context_while_holding_the_state_lock() {
        let backend = Arc::new(WhisperBackend::new());
        let context = backend.context.lock().unwrap();
        let worker = {
            let backend = Arc::clone(&backend);
            thread::spawn(move || backend.with_warm_state_grace(PATIENT, |_| Ok(())))
        };

        wait_until_held(&backend.state);
        drop(context);

        let result = worker.join().unwrap();
        assert!(
            matches!(result, Err(DictationError::ModelNotLoaded)),
            "got {result:?}"
        );
        assert!(backend.state.try_lock().is_ok(), "released afterwards");
    }
}

/// Characterisation of the runtime swap with fake contexts and states (#371):
/// what a model switch keeps, releases and refuses while a transcription runs,
/// and the order in which it takes the four locks.
#[cfg(test)]
mod runtime_swap_tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::thread;

    const SHORT: Duration = Duration::from_millis(30);

    /// Counts how many fake contexts and states have been released.
    struct Probe {
        name: &'static str,
        released: Arc<AtomicU32>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn key(model: WhisperModel) -> RuntimeKey {
        RuntimeKey {
            model,
            profile: ContextProfile::FlashAttention,
        }
    }

    fn swedish() -> RuntimeKey {
        key(WhisperModel::KbWhisperBase)
    }

    fn english() -> RuntimeKey {
        key(WhisperModel::BaseEn)
    }

    struct Fakes {
        state: Mutex<Option<Probe>>,
        context: Mutex<Option<Probe>>,
        loaded: Mutex<Option<RuntimeKey>>,
        cached: Mutex<Option<CachedWhisperRuntime<Probe, Probe>>>,
        released: Arc<AtomicU32>,
    }

    /// What the four slots hold, by fake name.
    #[derive(Debug, PartialEq)]
    struct Snapshot {
        active: Option<RuntimeKey>,
        context: Option<&'static str>,
        state: Option<&'static str>,
        cached: Option<(RuntimeKey, &'static str, Option<&'static str>)>,
    }

    impl Fakes {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(None),
                context: Mutex::new(None),
                loaded: Mutex::new(None),
                cached: Mutex::new(None),
                released: Arc::new(AtomicU32::new(0)),
            })
        }

        fn probe(&self, name: &'static str) -> Probe {
            Probe {
                name,
                released: Arc::clone(&self.released),
            }
        }

        fn slots(&self) -> RuntimeSlots<'_, Probe, Probe> {
            RuntimeSlots {
                state: &self.state,
                context: &self.context,
                loaded_runtime: &self.loaded,
                cached_runtime: &self.cached,
            }
        }

        fn publish(&self, key: RuntimeKey, context: &'static str) -> Result<(), DictationError> {
            publish_runtime(&self.slots(), self.probe(context), key, SHORT)
        }

        fn warm(&self, state: &'static str) {
            *self.state.lock().unwrap() = Some(self.probe(state));
        }

        fn released(&self) -> u32 {
            self.released.load(Ordering::SeqCst)
        }

        fn snapshot(&self) -> Snapshot {
            Snapshot {
                active: *self.loaded.lock().unwrap(),
                context: self.context.lock().unwrap().as_ref().map(|p| p.name),
                state: self.state.lock().unwrap().as_ref().map(|p| p.name),
                cached: self.cached.lock().unwrap().as_ref().map(|runtime| {
                    (
                        runtime.key,
                        runtime.context.name,
                        runtime.state.as_ref().map(|p| p.name),
                    )
                }),
            }
        }
    }

    /// An inference in flight: holds the state lock until `release` fires.
    fn hold_state(fakes: &Arc<Fakes>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let holder = Arc::clone(fakes);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _inference = holder.state.lock().unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        held_rx.recv().unwrap();
        (release_tx, handle)
    }

    #[test]
    fn the_first_runtime_becomes_active_without_a_warm_state_or_a_secondary() {
        let fakes = Fakes::new();

        fakes.publish(swedish(), "swedish-ctx").unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: None,
            }
        );
        assert_eq!(fakes.released(), 0);
    }

    #[test]
    fn a_second_runtime_inside_the_budget_moves_the_first_and_its_warm_state_to_the_cache() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.warm("swedish-state");

        fakes.publish(english(), "english-ctx").unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(english()),
                context: Some("english-ctx"),
                state: None,
                cached: Some((swedish(), "swedish-ctx", Some("swedish-state"))),
            }
        );
        assert_eq!(
            fakes.released(),
            0,
            "nothing is released while it can be reused"
        );
    }

    #[test]
    fn a_leftover_warm_state_never_survives_into_a_newly_published_runtime() {
        let fakes = Fakes::new();
        fakes.warm("orphan-state");

        fakes.publish(swedish(), "swedish-ctx").unwrap();

        assert_eq!(fakes.snapshot().state, None);
        assert_eq!(fakes.released(), 1, "the orphan is released");
    }

    #[test]
    fn a_runtime_outside_the_budget_is_released_instead_of_cached() {
        let fakes = Fakes::new();
        fakes
            .publish(key(WhisperModel::KbWhisperMedium), "medium-ctx")
            .unwrap();
        fakes.warm("medium-state");

        fakes.publish(english(), "english-ctx").unwrap();

        assert_eq!(fakes.snapshot().cached, None);
        assert_eq!(fakes.released(), 2, "its context and its warm state");
    }

    #[test]
    fn publishing_the_same_runtime_again_releases_the_old_copy() {
        let fakes = Fakes::new();
        fakes.publish(english(), "first-ctx").unwrap();

        fakes.publish(english(), "second-ctx").unwrap();

        assert_eq!(fakes.snapshot().context, Some("second-ctx"));
        assert_eq!(
            fakes.snapshot().cached,
            None,
            "a runtime is never its own secondary"
        );
        assert_eq!(fakes.released(), 1);
    }

    #[test]
    fn a_third_runtime_replaces_the_secondary_with_the_previous_active_one() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();

        fakes
            .publish(key(WhisperModel::NbWhisperBase), "norwegian-ctx")
            .unwrap();

        let snapshot = fakes.snapshot();
        assert_eq!(snapshot.active, Some(key(WhisperModel::NbWhisperBase)));
        assert_eq!(snapshot.cached, Some((english(), "english-ctx", None)));
        assert_eq!(fakes.released(), 1, "the old secondary's context");
    }

    #[test]
    fn a_model_switch_during_inference_is_refused_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        let (release, inference) = hold_state(&fakes);

        let result = fakes.publish(english(), "english-ctx");

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "got {result:?}"
        );
        release.send(()).unwrap();
        inference.join().unwrap();
        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: None,
            }
        );
        assert_eq!(
            fakes.released(),
            1,
            "only the freshly loaded context is dropped"
        );
    }

    #[test]
    fn activating_the_secondary_during_inference_is_refused_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();
        let before = fakes.snapshot();
        let (release, inference) = hold_state(&fakes);

        let result = activate_cached_slots(&fakes.slots(), swedish(), SHORT);

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "got {result:?}"
        );
        release.send(()).unwrap();
        inference.join().unwrap();
        assert_eq!(fakes.snapshot(), before);
    }

    #[test]
    fn activating_the_secondary_swaps_the_runtimes_with_their_warm_states() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();
        fakes.warm("english-state");

        activate_cached_slots(&fakes.slots(), swedish(), SHORT).unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: Some((english(), "english-ctx", Some("english-state"))),
            }
        );
        assert_eq!(fakes.released(), 0);
    }

    #[test]
    fn activating_a_runtime_that_is_not_cached_fails_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.warm("swedish-state");
        let before = fakes.snapshot();

        let result = activate_cached_slots(&fakes.slots(), english(), SHORT);

        assert!(
            matches!(result, Err(DictationError::ModelNotLoaded)),
            "got {result:?}"
        );
        assert_eq!(fakes.snapshot(), before);
    }

    type Swap = fn(&RuntimeSlots<'_, Probe, Probe>) -> Result<(), DictationError>;

    /// Both transactions that take all four locks. `activate` finds nothing
    /// cached and reports `ModelNotLoaded` once it has the locks.
    fn each_swap() -> [(&'static str, Swap); 2] {
        [
            ("publish", |slots| {
                let ctx = Probe {
                    name: "new-ctx",
                    released: Arc::new(AtomicU32::new(0)),
                };
                publish_runtime(slots, ctx, english(), PATIENT)
            }),
            ("activate", |slots| {
                activate_cached_slots(slots, english(), PATIENT)
            }),
        ]
    }

    fn spawn_swap(fakes: &Arc<Fakes>, swap: Swap) -> thread::JoinHandle<()> {
        let swapper = Arc::clone(fakes);
        thread::spawn(move || {
            let _ = swap(&swapper.slots());
        })
    }

    // The documented lock order is state -> context -> loaded_runtime -> cache.
    // Seen from outside: while a swap waits on one lock it holds every earlier
    // lock and none of the later ones.

    #[test]
    fn a_swap_takes_the_state_lock_before_the_context() {
        for (name, swap) in each_swap() {
            let fakes = Fakes::new();
            let blocker = fakes.context.lock().unwrap();
            let worker = spawn_swap(&fakes, swap);

            wait_until_held(&fakes.state);
            assert!(
                !is_held(&fakes.loaded),
                "{name}: identity taken before the context"
            );
            assert!(
                !is_held(&fakes.cached),
                "{name}: cache taken before the context"
            );

            drop(blocker);
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_swap_takes_the_context_before_the_runtime_identity() {
        for (name, swap) in each_swap() {
            let fakes = Fakes::new();
            let blocker = fakes.loaded.lock().unwrap();
            let worker = spawn_swap(&fakes, swap);

            wait_until_held(&fakes.state);
            wait_until_held(&fakes.context);
            assert!(
                !is_held(&fakes.cached),
                "{name}: cache taken before the identity"
            );

            drop(blocker);
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_swap_takes_the_runtime_identity_before_the_cache() {
        for (_name, swap) in each_swap() {
            let fakes = Fakes::new();
            let blocker = fakes.cached.lock().unwrap();
            let worker = spawn_swap(&fakes, swap);

            wait_until_held(&fakes.state);
            wait_until_held(&fakes.context);
            wait_until_held(&fakes.loaded);

            drop(blocker);
            worker.join().unwrap();
        }
    }
}

/// The DTW alignment-head preset per model. Moved here from `WhisperModel`
/// (#372) so the models module names no `whisper_rs` type; this table pins the
/// mapping it had there: fine-tunes share the preset of their base architecture
/// and the large fine-tunes are large-v3.
#[cfg(feature = "diarization")]
#[test]
fn dtw_preset_follows_the_base_architecture_of_every_model() {
    let expected = [
        (WhisperModel::TinyEn, "TinyEn"),
        (WhisperModel::Tiny, "Tiny"),
        (WhisperModel::BaseEn, "BaseEn"),
        (WhisperModel::Base, "Base"),
        (WhisperModel::FinnishWhisperTiny, "Tiny"),
        (WhisperModel::KbWhisperTiny, "Tiny"),
        (WhisperModel::KbWhisperBase, "Base"),
        (WhisperModel::KbWhisperSmall, "Small"),
        (WhisperModel::KbWhisperMedium, "Medium"),
        (WhisperModel::KbWhisperLarge, "LargeV3"),
        (WhisperModel::NbWhisperTiny, "Tiny"),
        (WhisperModel::NbWhisperBase, "Base"),
        (WhisperModel::NbWhisperSmall, "Small"),
        (WhisperModel::NbWhisperMedium, "Medium"),
        (WhisperModel::NbWhisperLarge, "LargeV3"),
        (WhisperModel::SmallEn, "SmallEn"),
        (WhisperModel::Small, "Small"),
        (WhisperModel::MediumEn, "MediumEn"),
        (WhisperModel::Medium, "Medium"),
        (WhisperModel::LargeV3Turbo, "LargeV3Turbo"),
        (WhisperModel::LargeV3TurboQ8, "LargeV3Turbo"),
    ];
    for (model, preset) in expected {
        assert_eq!(format!("{:?}", dtw_preset(model)), preset, "{model:?}");
    }
}
