use super::*;

#[cfg(test)]
mod decode_flags_tests {
    use super::*;

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

/// Moved from the former `segment_confidence_tests` (#371); the module name is kept so
/// the test ids stay stable.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

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
    fn display_transcript_is_the_assembled_text_trimmed_and_normalized() {
        use crate::models::Language;

        // Whisper pads its text with a leading space; none of it survives at the edges.
        let padded = [segment(0.0, 1.0, " Hello"), segment(1.0, 2.0, " world. ")];
        assert_eq!(
            display_transcript(&padded, Language::English),
            "Hello world."
        );

        // A hallucination Whisper marked as no-speech is dropped from the text.
        let marked = [
            segment(0.0, 1.0, " Hi"),
            segment(1.0, 2.0, "<|nospeech|> thanks for watching"),
        ];
        assert_eq!(display_transcript(&marked, Language::English), "Hi");

        // A run of music words collapses to the language's marker, for display
        // only: the raw segment keeps what Whisper emitted.
        let music = [segment(0.0, 3.0, " Music Music Music")];
        assert_eq!(display_transcript(&music, Language::English), "[MUSIC]");
        assert_eq!(music[0].text, " Music Music Music");
        // The marker is the configured language's.
        let swedish = [segment(0.0, 3.0, " Musik Musik Musik")];
        assert_eq!(display_transcript(&swedish, Language::Swedish), "[MUSIK]");
    }

    #[test]
    fn display_transcript_of_nothing_is_empty() {
        use crate::models::Language;

        assert_eq!(display_transcript(&[], Language::Swedish), "");
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
