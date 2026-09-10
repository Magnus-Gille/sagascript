//! Conservative, deterministic suggestions for teaching a personal glossary.

use crate::transcription::Glossary;

const MAX_REPLACEMENT_WORDS: usize = 4;
const MAX_REPLACEMENT_BYTES: usize = 96;
const MAX_TRANSCRIPT_WORDS: usize = 2_048;

/// The action a reviewed suggestion is safe to take in the glossary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlossarySuggestionKind {
    /// The observed text can be stored as an alias for the corrected text.
    Alias,
    /// The corrected text is useful decoder context, but has no safe alias.
    HintOnly,
}

/// A deterministic candidate produced from a raw/effective transcript and its
/// manually corrected version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GlossarySuggestion {
    /// Text as heard by the recognizer. Empty for an insertion hint.
    pub observed: String,
    /// Text the user supplied as the correction.
    pub canonical: String,
    pub kind: GlossarySuggestionKind,
    /// Small word window around the edit for human review.
    pub context: String,
}

/// Compare two transcripts and return only bounded, unambiguous glossary
/// candidates. Punctuation and case are deliberately ignored for alignment;
/// the returned values retain the user's original spelling.
pub fn suggest_glossary_candidates(
    heard: &str,
    corrected: &str,
    glossary: &Glossary,
) -> Vec<GlossarySuggestion> {
    let heard_tokens = tokenize(heard);
    let corrected_tokens = tokenize(corrected);
    if heard_tokens.is_empty()
        || corrected_tokens.is_empty()
        || heard_tokens.len() > MAX_TRANSCRIPT_WORDS
        || corrected_tokens.len() > MAX_TRANSCRIPT_WORDS
    {
        return Vec::new();
    }

    let mut suggestions = Vec::new();
    let mut heard_index = 0;
    let mut corrected_index = 0;
    let mut unchanged_words = 0;
    while heard_index < heard_tokens.len() || corrected_index < corrected_tokens.len() {
        if heard_index < heard_tokens.len()
            && corrected_index < corrected_tokens.len()
            && same_word(
                &heard_tokens[heard_index].text,
                &corrected_tokens[corrected_index].text,
            )
        {
            heard_index += 1;
            corrected_index += 1;
            unchanged_words += 1;
            continue;
        }

        let heard_start = heard_index;
        let corrected_start = corrected_index;
        while heard_index < heard_tokens.len()
            && corrected_index < corrected_tokens.len()
            && !same_word(
                &heard_tokens[heard_index].text,
                &corrected_tokens[corrected_index].text,
            )
        {
            let heard_reappears = corrected_tokens[corrected_index + 1..]
                .iter()
                .any(|token| same_word(&heard_tokens[heard_index].text, &token.text));
            let corrected_reappears = heard_tokens[heard_index + 1..]
                .iter()
                .any(|token| same_word(&corrected_tokens[corrected_index].text, &token.text));
            if heard_reappears && !corrected_reappears {
                corrected_index += 1;
            } else if corrected_reappears && !heard_reappears {
                heard_index += 1;
            } else {
                // Neither current token is a unique synchronization point:
                // consume both as one bounded replacement. This also handles
                // a multi-token observation such as "Love a ball" ->
                // "Lovable" without inventing an insertion/deletion alias.
                heard_index += 1;
                corrected_index += 1;
            }
        }

        let heard_end = if corrected_index == corrected_tokens.len() {
            heard_tokens.len()
        } else {
            heard_index
        };
        let corrected_end = if heard_index == heard_tokens.len() {
            corrected_tokens.len()
        } else {
            corrected_index
        };
        heard_index = heard_end;
        corrected_index = corrected_end;
        let heard_count = heard_end - heard_start;
        let corrected_count = corrected_end - corrected_start;

        if heard_count == 0 {
            let canonical = join_tokens(&corrected_tokens[corrected_start..corrected_end]);
            if is_bounded(corrected_count, canonical.len())
                && !canonical.is_empty()
                && !is_represented_hint(glossary, &canonical)
            {
                suggestions.push(GlossarySuggestion {
                    observed: String::new(),
                    canonical,
                    kind: GlossarySuggestionKind::HintOnly,
                    context: context_window(
                        &corrected_tokens,
                        corrected_start,
                        corrected_end,
                    ),
                });
            }
            continue;
        }

        if corrected_count == 0 {
            // A deletion has no corrected spelling from which to learn.
            continue;
        }

        let observed = join_tokens(&heard_tokens[heard_start..heard_end]);
        let canonical = join_tokens(&corrected_tokens[corrected_start..corrected_end]);
        if !is_bounded(heard_count, observed.len())
            || !is_bounded(corrected_count, canonical.len())
            || (heard_count > 1 && corrected_count > 1)
            || same_word(&observed, &canonical)
            || is_represented_alias(glossary, &observed, &canonical)
        {
            continue;
        }

        suggestions.push(GlossarySuggestion {
            observed,
            canonical,
            kind: GlossarySuggestionKind::Alias,
            context: context_window(&heard_tokens, heard_start, heard_end),
        });
    }

    let alias_count = suggestions
        .iter()
        .filter(|candidate| candidate.kind == GlossarySuggestionKind::Alias)
        .count();
    if alias_count > 1
        && unchanged_words * 2 < heard_tokens.len().max(corrected_tokens.len())
    {
        return Vec::new();
    }

    let batch = suggestions.clone();
    suggestions.retain(|candidate| {
        candidate.kind != GlossarySuggestionKind::Alias
            || !batch.iter().any(|other| {
                other.kind == GlossarySuggestionKind::Alias
                    && same_word(&candidate.observed, &other.observed)
                    && !same_word(&candidate.canonical, &other.canonical)
            })
    });
    suggestions
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if is_word_character(character)
            || (matches!(character, '-' | '.' | '/')
                && !current.is_empty()
                && characters.peek().is_some_and(|next| next.is_alphanumeric()))
        {
            current.push(character);
        } else if !current.is_empty() {
            tokens.push(Token {
                text: std::mem::take(&mut current),
            });
        }
    }
    if !current.is_empty() {
        tokens.push(Token { text: current });
    }
    tokens
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(
            character as u32,
            0x0300..=0x036f | 0x1ab0..=0x1aff | 0x1dc0..=0x1dff | 0x20d0..=0x20ff
        )
}

fn join_tokens(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn context_window(tokens: &[Token], start: usize, end: usize) -> String {
    let window_start = start.saturating_sub(4);
    let window_end = (end + 4).min(tokens.len());
    join_tokens(&tokens[window_start..window_end])
}

fn normalized(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

fn same_word(left: &str, right: &str) -> bool {
    normalized(left) == normalized(right)
}

fn is_bounded(word_count: usize, byte_count: usize) -> bool {
    word_count > 0 && word_count <= MAX_REPLACEMENT_WORDS && byte_count <= MAX_REPLACEMENT_BYTES
}

fn is_represented_hint(glossary: &Glossary, canonical: &str) -> bool {
    glossary
        .entries()
        .iter()
        .any(|entry| same_word(&entry.canonical, canonical))
}

fn is_represented_alias(glossary: &Glossary, observed: &str, canonical: &str) -> bool {
    glossary.entries().iter().any(|entry| {
        same_word(&entry.canonical, observed)
            || entry
                .aliases
                .iter()
                .any(|alias| same_word(alias, canonical))
            || entry.aliases.iter().any(|alias| same_word(alias, observed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::Glossary;

    #[test]
    fn suggests_a_single_word_correction() {
        let suggestions = suggest_glossary_candidates(
            "Magnus Jille arbetar här.",
            "Magnus Gille arbetar här.",
            &Glossary::default(),
        );

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "Jille");
        assert_eq!(suggestions[0].canonical, "Gille");
        assert_eq!(suggestions[0].kind, GlossarySuggestionKind::Alias);
    }

    #[test]
    fn suggests_a_phrase_correction() {
        let suggestions = suggest_glossary_candidates(
            "Vi använder Love a ball i dag.",
            "Vi använder Lovable i dag.",
            &Glossary::default(),
        );

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "Love a ball");
        assert_eq!(suggestions[0].canonical, "Lovable");
        assert_eq!(suggestions[0].kind, GlossarySuggestionKind::Alias);
    }

    #[test]
    fn preserves_hyphenated_and_dotted_canonical_terms() {
        let suggestions = suggest_glossary_candidates(
            "Vi använder GPT 4o i dag.",
            "Vi använder GPT-4o i dag.",
            &Glossary::default(),
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "GPT 4o");
        assert_eq!(suggestions[0].canonical, "GPT-4o");

        let suggestions = suggest_glossary_candidates(
            "Vi använder version 4 1 nu.",
            "Vi använder version 4.1 nu.",
            &Glossary::default(),
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].canonical, "4.1");
    }

    #[test]
    fn suggests_swedish_unicode_correction() {
        let suggestions = suggest_glossary_candidates(
            "Vi använder klåd i dag.",
            "Vi använder Claude i dag.",
            &Glossary::default(),
        );

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "klåd");
        assert_eq!(suggestions[0].canonical, "Claude");
    }

    #[test]
    fn suggests_multiple_independent_replacements() {
        let suggestions = suggest_glossary_candidates(
            "mördsa till branch.",
            "mergea till branch.",
            &Glossary::default(),
        );

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "mördsa");
        assert_eq!(suggestions[0].canonical, "mergea");

        let suggestions = suggest_glossary_candidates(
            "Jag vill mördsa den här branschen med klåd i dag.",
            "Jag vill mergea den här branschen med Claude i dag.",
            &Glossary::default(),
        );
        assert_eq!(
            suggestions
                .iter()
                .map(|candidate| (&candidate.observed, &candidate.canonical))
                .collect::<Vec<_>>(),
            vec![
                (&"mördsa".to_string(), &"mergea".to_string()),
                (&"klåd".to_string(), &"Claude".to_string()),
            ]
        );
    }

    #[test]
    fn insertion_is_hint_only() {
        let suggestions = suggest_glossary_candidates(
            "Vi arbetar dag.",
            "Vi arbetar Sagascript dag.",
            &Glossary::default(),
        );

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "");
        assert_eq!(suggestions[0].canonical, "Sagascript");
        assert_eq!(suggestions[0].kind, GlossarySuggestionKind::HintOnly);
    }

    #[test]
    fn deletions_do_not_create_candidates() {
        let suggestions = suggest_glossary_candidates(
            "Vi arbetar i Sagascript i dag.",
            "Vi arbetar i dag.",
            &Glossary::default(),
        );

        assert!(suggestions.is_empty());
    }

    #[test]
    fn end_of_input_insertions_and_deletions_terminate() {
        let suggestions = suggest_glossary_candidates(
            "Vi arbetar.",
            "Vi arbetar Sagascript.",
            &Glossary::default(),
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].kind, GlossarySuggestionKind::HintOnly);
        assert_eq!(suggestions[0].canonical, "Sagascript");

        assert!(suggest_glossary_candidates(
            "Vi arbetar Sagascript.",
            "Vi arbetar.",
            &Glossary::default(),
        )
        .is_empty());
    }

    #[test]
    fn punctuation_and_case_only_changes_do_not_create_candidates() {
        assert!(
            suggest_glossary_candidates("Hej, Magnus.", "hej Magnus!", &Glossary::default(),)
                .is_empty()
        );
    }

    #[test]
    fn broad_rewrites_and_empty_text_fail_closed() {
        assert!(suggest_glossary_candidates(
            "Det här är en helt ny mening.",
            "Nu skriver vi något totalt annorlunda.",
            &Glossary::default(),
        )
        .is_empty());
        assert!(suggest_glossary_candidates("", "Lovable", &Glossary::default()).is_empty());
        assert!(suggest_glossary_candidates("Lovable", "", &Glossary::default()).is_empty());

        assert!(suggest_glossary_candidates(
            "alpha Paris beta",
            "gamma Paris delta",
            &Glossary::default(),
        )
        .is_empty());
    }

    #[test]
    fn oversized_training_passages_fail_closed() {
        let heard = std::iter::repeat_n("ord", MAX_TRANSCRIPT_WORDS + 1)
            .collect::<Vec<_>>()
            .join(" ");
        let corrected = heard.replacen("ord", "term", 1);
        assert!(suggest_glossary_candidates(&heard, &corrected, &Glossary::default()).is_empty());
    }

    #[test]
    fn repeated_token_ambiguity_fails_closed() {
        let suggestions =
            suggest_glossary_candidates("gå till gå hem.", "gå hem.", &Glossary::default());
        assert!(suggestions.is_empty());
    }

    #[test]
    fn repeated_unchanged_words_do_not_hide_a_bounded_correction() {
        let suggestions = suggest_glossary_candidates(
            "Jag använder Codex och jag använder klåd.",
            "Jag använder Codex och jag använder Claude.",
            &Glossary::default(),
        );
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].observed, "klåd");
        assert_eq!(suggestions[0].canonical, "Claude");
    }

    #[test]
    fn existing_aliases_and_hints_are_suppressed() {
        let glossary = Glossary::parse("mergea = mördsa\nSagascript");
        assert!(suggest_glossary_candidates("mördsa", "mergea", &glossary).is_empty());
        assert!(suggest_glossary_candidates("vi dag", "vi Sagascript dag", &glossary).is_empty());
    }

    #[test]
    fn existing_canonical_and_conflicting_aliases_fail_closed() {
        let glossary = Glossary::parse("merchandise = merch\nmerge");
        assert!(suggest_glossary_candidates("merch", "merge", &glossary).is_empty());
        assert!(suggest_glossary_candidates("merge", "mergea", &glossary).is_empty());

        let glossary = Glossary::parse("product = mergea");
        assert!(suggest_glossary_candidates("mördsa", "mergea", &glossary).is_empty());
    }

    #[test]
    fn merge_merch_collision_is_safe_across_swedish_and_english_contexts() {
        let glossary = Glossary::parse("merge = merch");

        let swedish = suggest_glossary_candidates(
            "Vi kan merge den här ändringen.",
            "Vi kan merch den här ändringen.",
            &glossary,
        );
        let english = suggest_glossary_candidates(
            "We can merch this merge.",
            "We can merge this merge.",
            &glossary,
        );

        assert!(swedish
            .iter()
            .all(|candidate| candidate.kind != GlossarySuggestionKind::Alias));
        assert!(english
            .iter()
            .all(|candidate| candidate.kind != GlossarySuggestionKind::Alias));
    }

    #[test]
    fn deterministic_evaluation_reports_recall_safety_and_latency() {
        struct EvaluationCase<'a> {
            heard: &'a str,
            corrected: &'a str,
            expected_aliases: &'a [(&'a str, &'a str)],
            glossary: &'a Glossary,
            unsafe_case: bool,
        }

        let empty_glossary = Glossary::default();
        let merge_glossary = Glossary::parse("merge = merch");
        let magnus = [("Jille", "Gille")];
        let lovable = [("Love a ball", "Lovable")];
        let tooling = [("mördsa", "mergea"), ("klåd", "Claude")];
        let unicode = [("Åmål", "Älmhult")];
        let cases = [
            EvaluationCase {
                heard: "Magnus Jille arbetar här.",
                corrected: "Magnus Gille arbetar här.",
                expected_aliases: &magnus,
                glossary: &empty_glossary,
                unsafe_case: false,
            },
            EvaluationCase {
                heard: "Vi använder Love a ball i dag.",
                corrected: "Vi använder Lovable i dag.",
                expected_aliases: &lovable,
                glossary: &empty_glossary,
                unsafe_case: false,
            },
            EvaluationCase {
                heard: "Jag vill mördsa den här branschen med klåd i dag.",
                corrected: "Jag vill mergea den här branschen med Claude i dag.",
                expected_aliases: &tooling,
                glossary: &empty_glossary,
                unsafe_case: false,
            },
            EvaluationCase {
                heard: "Vi såg räksmörgås nära Åmål.",
                corrected: "Vi såg räksmörgås nära Älmhult.",
                expected_aliases: &unicode,
                glossary: &empty_glossary,
                unsafe_case: false,
            },
            EvaluationCase {
                heard: "Vi arbetar dag.",
                corrected: "Vi arbetar Sagascript dag.",
                expected_aliases: &[],
                glossary: &empty_glossary,
                unsafe_case: true,
            },
            EvaluationCase {
                heard: "Vi arbetar Sagascript.",
                corrected: "Vi arbetar.",
                expected_aliases: &[],
                glossary: &empty_glossary,
                unsafe_case: true,
            },
            EvaluationCase {
                heard: "Det här är en helt ny mening.",
                corrected: "Nu skriver vi något totalt annorlunda.",
                expected_aliases: &[],
                glossary: &empty_glossary,
                unsafe_case: true,
            },
            EvaluationCase {
                heard: "Vi kan merge den här ändringen.",
                corrected: "Vi kan merch den här ändringen.",
                expected_aliases: &[],
                glossary: &merge_glossary,
                unsafe_case: true,
            },
        ];

        let mut expected_count = 0usize;
        let mut recovered_count = 0usize;
        let mut unsafe_case_count = 0usize;
        let mut unsafe_alias_count = 0usize;

        for case in &cases {
            let suggestions =
                suggest_glossary_candidates(case.heard, case.corrected, case.glossary);
            let aliases: Vec<_> = suggestions
                .iter()
                .filter(|candidate| candidate.kind == GlossarySuggestionKind::Alias)
                .map(|candidate| (candidate.observed.as_str(), candidate.canonical.as_str()))
                .collect();

            expected_count += case.expected_aliases.len();
            recovered_count += case
                .expected_aliases
                .iter()
                .filter(|(observed, canonical)| aliases.contains(&(*observed, *canonical)))
                .count();
            if !case.unsafe_case {
                assert_eq!(aliases, case.expected_aliases);
            } else {
                unsafe_case_count += 1;
                unsafe_alias_count += aliases.len();
                assert!(aliases.is_empty(), "unsafe case proposed {aliases:?}");
            }
        }

        let recall = recovered_count as f64 / expected_count as f64;
        let unsafe_alias_proposal_rate = unsafe_alias_count as f64 / unsafe_case_count as f64;
        assert_eq!(recovered_count, expected_count);
        assert_eq!(unsafe_alias_count, 0);

        let mut timings = Vec::with_capacity(cases.len() * 128);
        for _ in 0..128 {
            for case in &cases {
                let started = std::time::Instant::now();
                std::hint::black_box(suggest_glossary_candidates(
                    case.heard,
                    case.corrected,
                    case.glossary,
                ));
                timings.push(started.elapsed().as_nanos());
            }
        }
        timings.sort_unstable();
        let p50 = timings[(timings.len() - 1) * 50 / 100];
        let p95 = timings[(timings.len() - 1) * 95 / 100];
        assert!(p50 <= p95);
        println!(
            "glossary evaluation: recall={recall:.3}, unsafe_alias_rate={unsafe_alias_proposal_rate:.3}, p50={p50}ns, p95={p95}ns"
        );
    }
}
