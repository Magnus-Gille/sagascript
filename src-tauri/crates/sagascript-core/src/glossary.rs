//! Personal glossary parsing and deterministic post-transcription correction.
//!
//! The persisted `initial_prompt` remains the source of truth. Plain entries
//! continue to act only as Whisper hints; `canonical = alias | alias` entries
//! additionally authorize exact, whole-word/phrase replacements.

use regex::Regex;

/// Whether `text` holds a character the dictionary text reserves for its own
/// grammar: entries end at a newline or a comma, `=` separates a term from its
/// aliases and `|` the aliases from each other. A term or an alias that holds
/// one would be read back as something else.
pub fn contains_delimiter(text: &str) -> bool {
    text.chars()
        .any(|character| matches!(character, '\n' | '\r' | ',' | '=' | '|'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub canonical: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GlossaryCorrection {
    pub original: String,
    pub replacement: String,
    #[serde(skip)]
    pub start: usize,
    #[serde(skip)]
    pub end: usize,
}

/// Shortest squashed form an explicit alias may repair by spacing alone;
/// shorter keys would match ordinary phrases such as "to get her".
const MIN_SQUASH_ALIAS_LEN: usize = 5;

/// A run of 1–3 adjacent words whose letters, with the spacing and hyphens
/// between them removed, may spell a dictionary term (#580).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SquashWindow {
    pub start: usize,
    pub end: usize,
    /// Lowercased letters and digits of the window, without separators.
    pub key: String,
    /// Byte ranges of the words inside the window.
    pub words: Vec<(usize, usize)>,
}

/// Letters, digits and combining marks: a combining accent never ends a word,
/// so a window cannot cut a decomposed word in two.
fn is_word_char(character: char) -> bool {
    character.is_alphanumeric()
        || matches!(character as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// The squashed comparison form: lowercase letters and digits only.
pub fn squash_key(text: &str) -> String {
    text.chars()
        .filter(|character| is_word_char(*character))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Every window of `1..=max_words` adjacent words in `text`. Words are maximal
/// alphanumeric runs and only whitespace or a hyphen may separate the words of
/// a window, so punctuation (a clause boundary) never lets a window merge
/// words. A window spans exactly its first word's start to its last word's end.
pub fn squash_windows(text: &str, max_words: usize) -> Vec<SquashWindow> {
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut current: Option<usize> = None;
    for (index, character) in text.char_indices() {
        if is_word_char(character) {
            current.get_or_insert(index);
        } else if let Some(start) = current.take() {
            words.push((start, index));
        }
    }
    if let Some(start) = current {
        words.push((start, text.len()));
    }
    let mut windows = Vec::new();
    for first in 0..words.len() {
        let mut key = String::new();
        for last in first..words.len().min(first + max_words) {
            if last > first {
                let gap = &text[words[last - 1].1..words[last].0];
                if !(gap == "-" || (!gap.is_empty() && gap.chars().all(char::is_whitespace))) {
                    break;
                }
            }
            key.push_str(&squash_key(&text[words[last].0..words[last].1]));
            windows.push(SquashWindow {
                start: words[first].0,
                end: words[last].1,
                key: key.clone(),
                words: words[first..=last].to_vec(),
            });
        }
    }
    windows
}

#[derive(Debug, Clone)]
struct AliasMatcher {
    canonical: String,
    pattern: Regex,
}

#[derive(Debug, Clone, Default)]
pub struct Glossary {
    source: String,
    entries: Vec<GlossaryEntry>,
    matchers: Vec<AliasMatcher>,
    /// Squashed explicit aliases (never the canonical term) and the term each
    /// repairs to.
    squash_aliases: Vec<(String, String)>,
}

impl Glossary {
    pub fn parse(source: &str) -> Self {
        let source = source.trim().to_string();
        let mut entries: Vec<GlossaryEntry> = Vec::new();

        for item in source
            .split([',', '\n'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let (canonical, aliases) = match item.split_once('=') {
                Some((canonical, aliases)) => {
                    let canonical = canonical.trim();
                    if canonical.is_empty() {
                        continue;
                    }
                    let aliases = aliases
                        .split('|')
                        .map(str::trim)
                        .filter(|alias| !alias.is_empty())
                        .map(str::to_string)
                        .collect();
                    (canonical, aliases)
                }
                None => (item, Vec::new()),
            };

            if let Some(existing) = entries
                .iter_mut()
                .find(|entry| same_term(&entry.canonical, canonical))
            {
                for alias in aliases {
                    if !existing
                        .aliases
                        .iter()
                        .any(|known| same_term(known, &alias))
                    {
                        existing.aliases.push(alias);
                    }
                }
            } else {
                entries.push(GlossaryEntry {
                    canonical: canonical.to_string(),
                    aliases,
                });
            }
        }

        let mut glossary = Self {
            source,
            entries,
            matchers: Vec::new(),
            squash_aliases: Vec::new(),
        };
        glossary.rebuild_matchers();
        glossary
    }

    /// Squashed explicit aliases (at least 5 characters, never the canonical
    /// term) with the term each repairs to, for the confidence-gated
    /// spacing repair of `transcribe --correct-hints` (#580).
    pub fn squash_aliases(&self) -> &[(String, String)] {
        &self.squash_aliases
    }

    pub fn entries(&self) -> &[GlossaryEntry] {
        &self.entries
    }

    /// Whether any entry has an alias: the exact replacements that only a
    /// profile with a known language may keep (a hint needs no language).
    pub fn has_aliases(&self) -> bool {
        self.entries.iter().any(|entry| !entry.aliases.is_empty())
    }

    pub fn upsert(&mut self, canonical: String, aliases: Vec<String>) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| same_term(&entry.canonical, &canonical))
        {
            for alias in aliases {
                if !existing
                    .aliases
                    .iter()
                    .any(|known| same_term(known, &alias))
                {
                    existing.aliases.push(alias);
                }
            }
        } else {
            self.entries.push(GlossaryEntry { canonical, aliases });
        }
        self.source = self.render();
        self.rebuild_matchers();
    }

    pub fn remove(&mut self, canonical: &str) -> bool {
        let previous_len = self.entries.len();
        self.entries
            .retain(|entry| !same_term(&entry.canonical, canonical));
        let removed = self.entries.len() != previous_len;
        if removed {
            self.source = self.render();
            self.rebuild_matchers();
        }
        removed
    }

    pub fn decoder_prompt(&self) -> Option<String> {
        if self.source.is_empty() {
            None
        } else if self.entries.iter().any(|entry| !entry.aliases.is_empty()) {
            Some(
                self.entries
                    .iter()
                    .map(|entry| entry.canonical.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        } else {
            Some(self.source.clone())
        }
    }

    pub fn correct_text(&self, text: &str) -> (String, Vec<GlossaryCorrection>) {
        #[derive(Debug)]
        struct Candidate<'a> {
            start: usize,
            end: usize,
            canonical: &'a str,
        }

        let mut candidates = Vec::new();
        for matcher in &self.matchers {
            for matched in matcher.pattern.find_iter(text) {
                if text[matched.start()..matched.end()] != matcher.canonical {
                    candidates.push(Candidate {
                        start: matched.start(),
                        end: matched.end(),
                        canonical: &matcher.canonical,
                    });
                }
            }
        }

        // Select against the original transcript in one pass. Longest-first at
        // each position prevents phrase aliases from being split by shorter
        // aliases, and avoiding cascading replacements makes behavior stable.
        candidates.sort_by(|left, right| {
            left.start
                .cmp(&right.start)
                .then_with(|| (right.end - right.start).cmp(&(left.end - left.start)))
                .then_with(|| left.canonical.cmp(right.canonical))
        });
        // Regex Unicode case folding has equivalence classes that are not
        // reproduced by lowercasing (for example s/long-s and k/Kelvin sign).
        // Detect ambiguity by the actual matched byte span so those aliases
        // fail closed just like identical spellings do.
        let mut unambiguous = Vec::new();
        let mut candidates = candidates.into_iter().peekable();
        while let Some(candidate) = candidates.next() {
            let mut ambiguous = false;
            while candidates
                .peek()
                .is_some_and(|next| next.start == candidate.start && next.end == candidate.end)
            {
                let duplicate = candidates.next().expect("peeked candidate");
                ambiguous |= duplicate.canonical != candidate.canonical;
            }
            if !ambiguous {
                unambiguous.push(candidate);
            }
        }
        let mut selected = Vec::new();
        let mut cursor = 0;
        for candidate in unambiguous {
            if candidate.start >= cursor {
                cursor = candidate.end;
                selected.push(candidate);
            }
        }

        if selected.is_empty() {
            return (text.to_string(), Vec::new());
        }

        let mut corrected = String::with_capacity(text.len());
        let mut corrections = Vec::with_capacity(selected.len());
        let mut last = 0;
        for candidate in selected {
            corrected.push_str(&text[last..candidate.start]);
            let original = &text[candidate.start..candidate.end];
            corrected.push_str(candidate.canonical);
            corrections.push(GlossaryCorrection {
                original: original.to_string(),
                replacement: candidate.canonical.to_string(),
                start: candidate.start,
                end: candidate.end,
            });
            last = candidate.end;
        }
        corrected.push_str(&text[last..]);
        (corrected, corrections)
    }

    /// Correct one logical transcript while preserving its original fragment
    /// boundaries. This lets timestamped and diarized callers match phrases
    /// that Whisper split across adjacent segments.
    pub fn correct_fragments(
        &self,
        fragments: &[&str],
    ) -> (Vec<String>, Vec<(usize, GlossaryCorrection)>) {
        let original = fragments.concat();
        let (corrected, corrections) = self.correct_text(&original);
        if corrections.is_empty() {
            return (
                fragments
                    .iter()
                    .map(|fragment| (*fragment).to_string())
                    .collect(),
                Vec::new(),
            );
        }

        let mut ranges = Vec::with_capacity(fragments.len());
        let mut offset = 0;
        for fragment in fragments {
            let end = offset + fragment.len();
            ranges.push((offset, end));
            offset = end;
        }

        let mut rebuilt = vec![String::new(); fragments.len()];
        let mut projected = Vec::with_capacity(corrections.len());
        let mut cursor = 0;
        for correction in corrections {
            append_original_range(&original, cursor, correction.start, &ranges, &mut rebuilt);
            let fragment_index = ranges
                .iter()
                .position(|(start, end)| *start <= correction.start && correction.start < *end)
                .unwrap_or_else(|| fragments.len().saturating_sub(1));
            rebuilt[fragment_index].push_str(&correction.replacement);
            cursor = correction.end;
            projected.push((fragment_index, correction));
        }
        append_original_range(&original, cursor, original.len(), &ranges, &mut rebuilt);
        debug_assert_eq!(rebuilt.concat(), corrected);
        (rebuilt, projected)
    }

    pub fn render(&self) -> String {
        self.entries
            .iter()
            .map(|entry| {
                if entry.aliases.is_empty() {
                    entry.canonical.clone()
                } else {
                    format!("{} = {}", entry.canonical, entry.aliases.join(" | "))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn single_word_terms(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|entry| entry.canonical.chars().all(char::is_alphabetic))
            .map(|entry| entry.canonical.clone())
            .collect()
    }

    fn rebuild_matchers(&mut self) {
        self.matchers = self
            .entries
            .iter()
            .filter(|entry| !entry.aliases.is_empty())
            .flat_map(|entry| {
                std::iter::once(entry.canonical.as_str())
                    .chain(entry.aliases.iter().map(String::as_str))
                    .filter(|alias| has_word_edges(alias))
                    .filter_map(|alias| {
                        whole_alias_regex(alias).map(|pattern| AliasMatcher {
                            canonical: entry.canonical.clone(),
                            pattern,
                        })
                    })
            })
            .collect();
        self.squash_aliases = self
            .entries
            .iter()
            .flat_map(|entry| {
                entry
                    .aliases
                    .iter()
                    .filter(|alias| has_word_edges(alias))
                    .map(|alias| (squash_key(alias), entry.canonical.clone()))
            })
            .filter(|(key, _)| key.chars().count() >= MIN_SQUASH_ALIAS_LEN)
            .collect();
    }
}

fn same_term(left: &str, right: &str) -> bool {
    Regex::new(&format!(r"(?iu)^{}$", escaped_alias_pattern(left)))
        .is_ok_and(|pattern| pattern.is_match(right))
}

fn append_original_range(
    original: &str,
    start: usize,
    end: usize,
    fragment_ranges: &[(usize, usize)],
    output: &mut [String],
) {
    for (fragment_index, (fragment_start, fragment_end)) in fragment_ranges.iter().enumerate() {
        let slice_start = start.max(*fragment_start);
        let slice_end = end.min(*fragment_end);
        if slice_start < slice_end {
            output[fragment_index].push_str(&original[slice_start..slice_end]);
        }
    }
}

fn has_word_edges(alias: &str) -> bool {
    let mut characters = alias.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let last = characters.last().unwrap_or(first);
    first.is_alphanumeric() && last.is_alphanumeric()
}

fn whole_alias_regex(alias: &str) -> Option<Regex> {
    Regex::new(&format!(r"(?iu)\b{}\b", escaped_alias_pattern(alias))).ok()
}

fn escaped_alias_pattern(alias: &str) -> String {
    let mut escaped = String::new();
    let mut in_whitespace = false;
    for character in alias.chars() {
        if character.is_whitespace() {
            if !in_whitespace {
                escaped.push_str(r"\s+");
                in_whitespace = true;
            }
        } else {
            escaped.push_str(&regex::escape(&character.to_string()));
            in_whitespace = false;
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn squash_windows_stay_within_three_words() {
        let keys: Vec<String> = squash_windows("a b c d", 3)
            .into_iter()
            .map(|w| w.key)
            .collect();
        assert_eq!(keys, ["a", "ab", "abc", "b", "bc", "bcd", "c", "cd", "d"]);
    }

    #[test]
    fn squash_windows_merge_only_across_plain_spacing_or_one_hyphen() {
        let keys = |text| -> Vec<usize> {
            squash_windows(text, 3)
                .iter()
                .map(|w| w.words.len())
                .collect()
        };
        assert_eq!(keys("a-b"), [1, 2, 1]);
        assert_eq!(keys("a - b"), [1, 1]);
        assert_eq!(keys("a. b"), [1, 1]);
        assert_eq!(keys("e\u{301}x y"), [1, 2, 1]);
    }

    #[test]
    fn squash_aliases_hold_only_long_explicit_aliases() {
        let glossary = Glossary::parse("worktree = work tree\nOdin = o din\nTogether");
        assert_eq!(
            glossary.squash_aliases(),
            [("worktree".to_string(), "worktree".to_string())]
        );
    }

    #[test]
    fn legacy_prompt_remains_byte_for_byte_decoder_context() {
        let glossary = Glossary::parse("OpenRouter, merge, pull request");
        assert_eq!(
            glossary.decoder_prompt().as_deref(),
            Some("OpenRouter, merge, pull request")
        );
        assert_eq!(glossary.entries().len(), 3);
    }

    #[test]
    fn mapped_prompt_hides_mishearings_from_whisper() {
        let glossary =
            Glossary::parse("OpenRouter = open router | open vrouter\nmerge = merch\nGrimnir");
        assert_eq!(
            glossary.decoder_prompt().as_deref(),
            Some("OpenRouter, merge, Grimnir")
        );
    }

    #[test]
    fn corrects_observed_aliases_and_preserves_punctuation() {
        let glossary = Glossary::parse(
            "OpenRouter = open router | open vrouter\nmerge = merch\nGrimnir = grimminer | skrimnir\nBroker = brocker",
        );
        let (text, corrections) =
            glossary.correct_text("Open router, Merch, Grimminer, Skrimnir och Brocker.");
        assert_eq!(text, "OpenRouter, merge, Grimnir, Grimnir och Broker.");
        assert_eq!(corrections.len(), 5);
        assert_eq!(corrections[0].original, "Open router");
        assert_eq!(corrections[0].replacement, "OpenRouter");
    }

    #[test]
    fn never_replaces_inside_an_unrelated_word() {
        let glossary = Glossary::parse("merge = merch");
        let (text, corrections) = glossary.correct_text("Merch merchandise merchant");
        assert_eq!(text, "merge merchandise merchant");
        assert_eq!(corrections.len(), 1);
    }

    #[test]
    fn longest_alias_wins_without_cascading_replacements() {
        let glossary = Glossary::parse("OpenRouter = open router\nRouter = router");
        let (text, corrections) = glossary.correct_text("open router router");
        assert_eq!(text, "OpenRouter Router");
        assert_eq!(corrections.len(), 2);
    }

    #[test]
    fn ambiguous_aliases_fail_closed() {
        let glossary = Glossary::parse("Grimnir = grimner\nGrimmer = grimner");
        let (text, corrections) = glossary.correct_text("grimner");
        assert_eq!(text, "grimner");
        assert!(corrections.is_empty());
    }

    #[test]
    fn unicode_case_equivalents_fail_closed_by_actual_match_span() {
        let glossary = Glossary::parse("LetterS = s\nLongS = ſ\nLetterK = k\nKelvin = K");
        let (text, corrections) = glossary.correct_text("s ſ k K");
        assert_eq!(text, "s ſ k K");
        assert!(corrections.is_empty());
    }

    #[test]
    fn corrections_report_original_byte_spans() {
        let glossary = Glossary::parse("OpenRouter = open router");
        let (_, corrections) = glossary.correct_text("Hej, open router!");
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].start, 5);
        assert_eq!(corrections[0].end, 16);
    }

    #[test]
    fn corrects_phrases_across_fragment_boundaries() {
        let glossary = Glossary::parse("OpenRouter = open router");
        let (fragments, corrections) = glossary.correct_fragments(&["open", " router!"]);
        assert_eq!(fragments, vec!["OpenRouter", "!"]);
        assert_eq!(corrections.len(), 1);
        assert_eq!(corrections[0].0, 0);
    }

    #[test]
    fn unicode_case_is_consistent_for_parse_upsert_and_remove() {
        let mut glossary = Glossary::parse("Ångström = ångstrom\nåNGSTRÖM = angstrom");
        assert_eq!(glossary.entries().len(), 1);
        glossary.upsert("ångström".to_string(), vec!["ång stroem".to_string()]);
        assert_eq!(glossary.entries().len(), 1);
        assert!(glossary.remove("åNGSTRÖM"));
        assert!(glossary.entries().is_empty());
    }

    #[test]
    fn management_uses_the_matchers_unicode_case_equivalents() {
        let mut glossary = Glossary::parse("s, ſ, k, K");
        assert_eq!(glossary.entries().len(), 2);
        glossary.upsert("ſ".to_string(), vec!["long ess".to_string()]);
        assert_eq!(glossary.entries().len(), 2);
        assert!(glossary.remove("K"));
        assert_eq!(glossary.entries().len(), 1);
        assert!(glossary.remove("ſ"));
        assert!(glossary.entries().is_empty());
    }

    #[test]
    fn render_and_single_word_terms_support_cli_and_batch() {
        let glossary = Glossary::parse(
            "OpenRouter = open router | open vrouter, merge = merch, pull request, Grimnir",
        );
        assert_eq!(
            glossary.render(),
            "OpenRouter = open router | open vrouter\nmerge = merch\npull request\nGrimnir"
        );
        assert_eq!(
            glossary.single_word_terms(),
            vec!["OpenRouter", "merge", "Grimnir"]
        );
    }

    #[test]
    fn upsert_and_remove_preserve_other_entries() {
        let mut glossary = Glossary::parse("OpenRouter, Grimnir = grimminer");
        glossary.upsert(
            "OpenRouter".to_string(),
            vec!["open router".to_string(), "open vrouter".to_string()],
        );
        glossary.upsert("merge".to_string(), vec!["merch".to_string()]);
        assert!(glossary.remove("grimnir"));
        assert!(!glossary.remove("missing"));
        assert_eq!(
            glossary.render(),
            "OpenRouter = open router | open vrouter\nmerge = merch"
        );
    }

    #[test]
    fn the_grammar_characters_are_the_delimiters() {
        for text in ["a\nb", "a\rb", "a,b", "a=b", "a|b", "=", "|"] {
            assert!(contains_delimiter(text), "{text:?}");
        }
        for text in [
            "",
            "OpenRouter",
            "open router",
            "Åsa-Lena",
            "a.b",
            "a;b",
            "a\tb",
        ] {
            assert!(!contains_delimiter(text), "{text:?}");
        }
        // What the delimiters mean to the parser: one of them splits or joins.
        assert_eq!(Glossary::parse("a,b").entries().len(), 2);
        assert_eq!(Glossary::parse("a=b").entries()[0].aliases, ["b"]);
        assert_eq!(Glossary::parse("a=b|c").entries()[0].aliases, ["b", "c"]);
    }

    #[test]
    fn a_glossary_has_aliases_only_if_an_entry_has_one() {
        assert!(!Glossary::parse("").has_aliases());
        assert!(!Glossary::parse("Alpha\nBeta, Gamma").has_aliases());
        assert!(Glossary::parse("Alpha\nBeta = b").has_aliases());
        assert!(!Glossary::parse("Alpha = ").has_aliases());
    }
}
