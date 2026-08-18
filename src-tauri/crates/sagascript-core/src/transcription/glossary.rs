//! Personal glossary parsing and deterministic post-transcription correction.
//!
//! The persisted `initial_prompt` remains the source of truth. Plain entries
//! continue to act only as Whisper hints; `canonical = alias | alias` entries
//! additionally authorize exact, whole-word/phrase replacements.

use std::collections::{HashMap, HashSet};

use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryEntry {
    pub canonical: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GlossaryCorrection {
    pub original: String,
    pub replacement: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Glossary {
    source: String,
    entries: Vec<GlossaryEntry>,
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
                .find(|entry| entry.canonical.eq_ignore_ascii_case(canonical))
            {
                for alias in aliases {
                    if !existing
                        .aliases
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(&alias))
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

        Self { source, entries }
    }

    pub fn entries(&self) -> &[GlossaryEntry] {
        &self.entries
    }

    pub fn upsert(&mut self, canonical: String, aliases: Vec<String>) {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.canonical.eq_ignore_ascii_case(&canonical))
        {
            for alias in aliases {
                if !existing
                    .aliases
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&alias))
                {
                    existing.aliases.push(alias);
                }
            }
        } else {
            self.entries.push(GlossaryEntry { canonical, aliases });
        }
        self.source = self.render();
    }

    pub fn remove(&mut self, canonical: &str) -> bool {
        let previous_len = self.entries.len();
        self.entries
            .retain(|entry| !entry.canonical.eq_ignore_ascii_case(canonical));
        let removed = self.entries.len() != previous_len;
        if removed {
            self.source = self.render();
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
        struct Binding<'a> {
            alias: String,
            canonical: &'a str,
        }

        #[derive(Debug)]
        struct Candidate<'a> {
            start: usize,
            end: usize,
            canonical: &'a str,
        }

        // One alias may appear only once in the effective map. If it points to
        // different canonical terms, fail closed instead of guessing.
        let mut bindings_by_key: HashMap<String, Vec<Binding<'_>>> = HashMap::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| !entry.aliases.is_empty())
        {
            for alias in std::iter::once(entry.canonical.as_str())
                .chain(entry.aliases.iter().map(String::as_str))
            {
                if !has_word_edges(alias) {
                    continue;
                }
                let key = alias_key(alias);
                let bindings = bindings_by_key.entry(key).or_default();
                if !bindings.iter().any(|binding| {
                    binding.canonical.eq_ignore_ascii_case(&entry.canonical)
                        && binding.alias.eq_ignore_ascii_case(alias)
                }) {
                    bindings.push(Binding {
                        alias: alias.to_string(),
                        canonical: &entry.canonical,
                    });
                }
            }
        }

        let mut candidates = Vec::new();
        for bindings in bindings_by_key.values() {
            let canonical_keys: HashSet<String> = bindings
                .iter()
                .map(|binding| binding.canonical.to_lowercase())
                .collect();
            if canonical_keys.len() != 1 {
                continue;
            }
            let binding = &bindings[0];
            let Some(pattern) = whole_alias_regex(&binding.alias) else {
                continue;
            };
            for matched in pattern.find_iter(text) {
                if &text[matched.start()..matched.end()] != binding.canonical {
                    candidates.push(Candidate {
                        start: matched.start(),
                        end: matched.end(),
                        canonical: binding.canonical,
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
        let mut selected = Vec::new();
        let mut cursor = 0;
        for candidate in candidates {
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
            });
            last = candidate.end;
        }
        corrected.push_str(&text[last..]);
        (corrected, corrections)
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
}

fn alias_key(alias: &str) -> String {
    alias
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
    Regex::new(&format!(r"(?iu)\b{escaped}\b")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
