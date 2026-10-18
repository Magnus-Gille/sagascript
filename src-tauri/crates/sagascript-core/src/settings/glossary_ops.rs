//! The personal-dictionary rules both surfaces share (#373).
//!
//! A profile's dictionary (`profile_glossaries`) is edited by two entry points:
//! `sagascript glossary` and `config set initial_prompt` in the CLI, and the
//! dictionary editor and the training review in the app (`set_profile_glossary`,
//! `apply_training_glossary`). Each used to carry its own copy of the rules that
//! say whether an edit is allowed: the profile has to exist, and a profile that
//! detects its language (`Language::Auto`) keeps dictionary *hints* but no
//! *aliases*, because an exact replacement is only safe in a known language.
//! The rules are decided here; what a refusal is called to the user is the
//! surface's, because the wording depends on what the user was doing
//! ("Glossary learning requires...", "Deterministic dictionary training
//! requires...") and the codes and texts of each surface do not change.
//!
//! Like the operations of `ops`, these are methods of [`Settings`] that either
//! change nothing or the whole of what they say.

use super::{Language, Settings, SettingsError};
use crate::glossary::Glossary;

/// Why an edit of a profile's dictionary was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictionaryRefusal {
    /// A refusal of the settings themselves: no profile has this id
    /// ([`SettingsError::UnknownDictationProfile`]).
    Settings(SettingsError),
    /// The profile detects its language, so its dictionary keeps hints only.
    AutoDetectAliases,
    /// Merging nothing into a dictionary that has entries would not remove them;
    /// removing is a command of its own.
    WouldEraseEntries,
}

impl From<SettingsError> for DictionaryRefusal {
    fn from(error: SettingsError) -> Self {
        Self::Settings(error)
    }
}

/// The text both surfaces print for an unknown profile, and the app's text for
/// an alias in an Auto-detect profile; a surface with other wording for a
/// refusal matches on the variant instead.
impl std::fmt::Display for DictionaryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settings(error) => error.fmt(f),
            Self::AutoDetectAliases => f.write_str(
                "Auto-detect profiles support dictionary hints, not deterministic aliases",
            ),
            Self::WouldEraseEntries => {
                f.write_str("An empty text does not remove the entries of a dictionary")
            }
        }
    }
}

impl std::error::Error for DictionaryRefusal {}

impl Settings {
    /// Whether profile `id` may keep aliases (exact replacements): it exists and
    /// does not detect its language.
    pub fn ensure_alias_profile(&self, id: &str) -> Result<(), DictionaryRefusal> {
        match self.dictation_profile(id)?.language {
            Language::Auto => Err(DictionaryRefusal::AutoDetectAliases),
            _ => Ok(()),
        }
    }

    /// Whether `source` may become the dictionary of profile `id`: the profile
    /// exists, and if it detects its language the text has hints only.
    pub fn check_dictionary_source(&self, id: &str, source: &str) -> Result<(), DictionaryRefusal> {
        if self.dictation_profile(id)?.language == Language::Auto
            && Glossary::parse(source).has_aliases()
        {
            return Err(DictionaryRefusal::AutoDetectAliases);
        }
        Ok(())
    }

    /// Replace the dictionary of profile `id` with `source` (the dictionary
    /// editor's save). Refused, with nothing changed, as
    /// [`check_dictionary_source`](Self::check_dictionary_source) says.
    pub fn set_profile_dictionary(
        &mut self,
        id: &str,
        source: String,
    ) -> Result<(), DictionaryRefusal> {
        self.check_dictionary_source(id, &source)?;
        self.profile_glossaries.insert(id.to_owned(), source);
        Ok(())
    }

    /// Add `term` to the dictionary of profile `id`, or merge `aliases` into the
    /// entry it already has (`sagascript glossary add`); the dictionary is
    /// written back in its canonical form. Refused, with nothing changed, when
    /// the profile does not exist, or when there are aliases and the profile
    /// detects its language. The caller has validated the text of `term` and of
    /// the aliases.
    pub fn add_dictionary_entry(
        &mut self,
        id: &str,
        term: &str,
        aliases: &[String],
    ) -> Result<(), DictionaryRefusal> {
        if aliases.is_empty() {
            self.dictation_profile(id)?;
        } else {
            self.ensure_alias_profile(id)?;
        }
        let source = self.profile_glossaries.entry(id.to_owned()).or_default();
        let mut glossary = Glossary::parse(source);
        glossary.upsert(term.to_owned(), aliases.to_vec());
        *source = glossary.render();
        Ok(())
    }

    /// Merge the entries of `text` into the dictionary of the default profile
    /// (`sagascript config set initial_prompt`, kept as an alias of adding
    /// entries): each term is added, each alias merged, and nothing is ever
    /// replaced or dropped, because the dictionary may hold reviewed aliases and
    /// has no rollback file of its own. The app's `set_initial_prompt` replaces
    /// the dictionary instead (`set_profile_dictionary`): a kept difference, and
    /// the only one of the dictionary rules (#373 part 2).
    ///
    /// Refused, with nothing changed, when `text` is empty and the dictionary is
    /// not. Unlike [`add_dictionary_entry`](Self::add_dictionary_entry) it does
    /// not look at the profile's language.
    pub fn merge_default_dictionary(&mut self, text: &str) -> Result<(), DictionaryRefusal> {
        let id = self.default_profile().id;
        let source = self.profile_glossaries.entry(id).or_default();
        if text.trim().is_empty() && !source.trim().is_empty() {
            return Err(DictionaryRefusal::WouldEraseEntries);
        }
        let mut glossary = Glossary::parse(source);
        for entry in Glossary::parse(text).entries() {
            glossary.upsert(entry.canonical.clone(), entry.aliases.clone());
        }
        *source = glossary.render();
        Ok(())
    }
}

#[cfg(test)]
#[path = "glossary_ops_tests.rs"]
mod tests;
