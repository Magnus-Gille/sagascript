//! The command-line side of the run context (#346).
//!
//! `transcribe`, `record` and `meeting reprocess` take the same four flags to
//! say which profile, language and dictionary a run uses: `--profile`,
//! `--language`, `--prompt` and `--prompt-file`. [`RunRequest`] holds them as
//! typed on the command line and resolves them through
//! [`sagascript_core::run_context`], which owns the semantics. What belongs to
//! the command line stays here: parsing a language name, and reading the
//! prompt file (whose error text names the path as it was typed).
//!
//! The model is not resolved here. Each command has its own rules and its own
//! error order, so it chooses its model from [`RunSelection::language`]
//! itself, between the two steps:
//!
//! - `transcribe` resolves in one call ([`RunRequest::resolve`]); its model
//!   choice comes after the prompt file has been read.
//! - `record` and `meeting reprocess` choose the model first and read the
//!   prompt file afterwards ([`RunRequest::select`], then
//!   [`RunRequest::finish`]), so an unusable model is reported before an
//!   unreadable prompt file.

use std::path::Path;

use sagascript_core::error::DictationError;
use sagascript_core::run_context::{RunContext, RunOverrides, RunSelection};
use sagascript_core::settings::Settings;

use crate::transcribe::{parse_language, resolve_one_run_prompt};

/// The profile, language and one-run prompt flags of one invocation, as typed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RunRequest<'a> {
    pub profile: Option<&'a str>,
    pub language: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub prompt_file: Option<&'a Path>,
}

impl RunRequest<'_> {
    /// Step one: the profile (unknown ids are an error) and the language
    /// (`--language` is parsed here, an unknown name is an error). Reads no
    /// file. The CLI never accepts `--profile` together with `--language`, so
    /// which of the two is reported first cannot be observed.
    pub(crate) fn select(&self, settings: &Settings) -> Result<RunSelection, DictationError> {
        let language = self.language.map(parse_language).transpose()?;
        RunSelection::resolve(
            settings,
            RunOverrides {
                profile: self.profile,
                language,
            },
        )
    }

    /// Step two: read the one-run prompt (`--prompt-file` wins over
    /// `--prompt`; an unreadable file is an error) and compose the dictionary.
    pub(crate) fn finish(
        &self,
        settings: &Settings,
        selection: RunSelection,
    ) -> Result<RunContext, DictationError> {
        let prompt = resolve_one_run_prompt(self.prompt, self.prompt_file)?;
        Ok(selection.into_context(settings, prompt.as_deref()))
    }

    /// Both steps, for a command whose model choice comes after them.
    pub(crate) fn resolve(&self, settings: &Settings) -> Result<RunContext, DictationError> {
        let selection = self.select(settings)?;
        self.finish(settings, selection)
    }
}
