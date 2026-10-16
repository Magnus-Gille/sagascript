//! Which profile, language and dictionary a file or recording run uses (#346).
//!
//! `sagascript transcribe`, `sagascript record` and `sagascript meeting
//! reprocess` all start the same way: pick the profile (the one asked for or
//! the default), take its language unless the run overrides it, and compose
//! the dictionary for that language from the profile's glossary and a one-run
//! prompt. The three commands used to repeat these steps; this module is the
//! one place that owns them, so a change to profile or dictionary semantics
//! cannot make one command differ from another.
//!
//! The resolution is pure: it reads [`Settings`] and its arguments, and
//! touches no file, model or global. Reading a prompt file and parsing a
//! `--language` string stay with the caller, which is where their error text
//! belongs.
//!
//! What this module does not decide is the model. Each command has its own
//! rules for it (a file run may use Pianissimo, a reprocessing run needs
//! Whisper, a live recording follows the profile's live model), so the model
//! is chosen by the caller from [`RunSelection::language`], between the two
//! steps below:
//!
//! 1. [`RunSelection::resolve`]: profile and language.
//! 2. [`RunSelection::into_context`]: the dictionary, given the one-run prompt.

use crate::error::DictationError;
use crate::settings::{HotkeyProfile, Language, Settings};
use crate::transcription::Glossary;

/// What a run asked for, already parsed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunOverrides<'a> {
    /// The profile to use; the default profile when `None`.
    pub profile: Option<&'a str>,
    /// The language to use instead of the profile's.
    pub language: Option<Language>,
}

/// The profile and language a run resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSelection {
    pub profile: HotkeyProfile,
    pub language: Language,
}

/// Everything a run takes from its profile, language and dictionary.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub profile: HotkeyProfile,
    pub language: Language,
    pub glossary: Glossary,
}

impl RunSelection {
    /// Pick the profile (an unknown id is an error; no id is the default
    /// profile) and the language (the override, else the profile's).
    pub fn resolve(
        settings: &Settings,
        overrides: RunOverrides<'_>,
    ) -> Result<Self, DictationError> {
        let profile = match overrides.profile {
            Some(id) => settings
                .resolved_hotkey_profiles()
                .into_iter()
                .find(|profile| profile.id == id)
                .ok_or_else(|| {
                    DictationError::SettingsError(format!("Unknown dictation profile '{id}'"))
                })?,
            None => settings.default_profile(),
        };
        let language = overrides.language.unwrap_or(profile.language);
        Ok(Self { profile, language })
    }

    /// Whether the run asked for another language than its profile's.
    pub fn language_overridden(&self) -> bool {
        self.language != self.profile.language
    }

    /// Compose the dictionary. `one_run_prompt` is the caller's own hint,
    /// already read and trimmed (`None` or blank when there is none).
    ///
    /// A profile's dictionary is written in the profile's language, and its
    /// aliases are deterministic replacements, so it applies only while the
    /// run keeps that language. When the run overrides the language, nothing
    /// saved applies, neither the profile dictionary nor the legacy global
    /// hint: only what the caller supplied for this run
    /// (`docs/glossary-scoping-decision.md`).
    pub fn into_context(self, settings: &Settings, one_run_prompt: Option<&str>) -> RunContext {
        let source = if self.language_overridden() {
            one_run_prompt.unwrap_or("").to_string()
        } else {
            settings.effective_glossary_source_with_prompt(Some(&self.profile.id), one_run_prompt)
        };
        RunContext {
            glossary: Glossary::parse(&source),
            profile: self.profile,
            language: self.language,
        }
    }
}

#[cfg(test)]
#[path = "run_context_tests.rs"]
mod tests;
