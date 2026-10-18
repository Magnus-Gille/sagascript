use super::*;

#[cfg(test)]
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

#[cfg(test)]
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
#[cfg(test)]
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

/// The DTW alignment-head preset per model. Moved here from `WhisperModel`
/// (#372) so the models module names no `whisper_rs` type; this table pins the
/// mapping it had there: fine-tunes share the preset of their base architecture
/// and the large fine-tunes are large-v3.
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
