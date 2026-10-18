//! Language and model resolution of a file run, and the one-run glossary
//! (#343): the IDs `--language` and `--model` accept, and which model a run
//! gets when none is named.

use std::path::Path;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{FileModel, FileModelPreference, Language, Settings, WhisperModel};
use sagascript_core::transcription::Glossary;

/// A language as typed on the command line: the id or the English name
/// (`sv`, `swedish`), the spellings of `Language`'s `FromStr`, which
/// `sagascript config` takes as well (#373).
pub fn parse_language(s: &str) -> Result<Language, DictationError> {
    s.parse::<Language>()
        .map_err(|error| DictationError::SettingsError(error.to_string()))
}

/// Resolves the whisper model to use for a run: an explicit `--model` argument
/// always wins; otherwise, if `auto_select_model` is set, the model
/// recommended for `language` is used; otherwise `fallback` (the stored
/// `whisper_model` setting) is used, ignoring `language`.
///
/// This is the single source of truth for the branch shared by
/// `transcribe::run()` and `record::run()` — keep their call sites in sync
/// with this function rather than re-deriving the logic inline.
///
/// Note: this intentionally does not delegate to
/// `Settings::effective_model()` (core/settings/manager.rs), because that
/// method always uses the *stored* `Settings::language`, while callers here
/// need to honor a `--language` override that differs from the stored value
/// (e.g. `--language sv` with auto-select on should recommend the Swedish
/// model even if the stored language is English).
pub fn resolve_effective_model(
    model_arg: Option<&str>,
    language: Language,
    auto_select_model: bool,
    fallback: WhisperModel,
) -> Result<WhisperModel, DictationError> {
    match model_arg {
        Some(s) => parse_model(s),
        None => Ok(if auto_select_model {
            WhisperModel::recommended(language)
        } else {
            fallback
        }),
    }
}

/// Resolve a file run separately from the live `record` model. `Auto` keeps
/// the CLI's existing fallback semantics for installations without a file
/// preference; an explicit file choice is checked against the requested
/// language rather than silently changing engines.
pub fn resolve_file_model(
    model_arg: Option<&str>,
    language: Language,
    settings: &Settings,
) -> Result<FileModel, DictationError> {
    resolve_file_model_for_run(model_arg, language, settings, false)
}

/// [`resolve_file_model`] for a run that may need Whisper-only features
/// (diarization, meeting output, decoder hints, beam/parallel/VAD options).
/// An implicit choice (no `--model`, file preference `Auto`) then resolves to
/// the recommended Whisper model instead of Pianissimo.
pub fn resolve_file_model_for_run(
    model_arg: Option<&str>,
    language: Language,
    settings: &Settings,
    needs_whisper: bool,
) -> Result<FileModel, DictationError> {
    if let Some(id) = model_arg {
        let preference =
            FileModelPreference::parse_id(id).map_err(DictationError::from)?;
        if preference == FileModelPreference::Auto {
            return Err(DictationError::SettingsError(
                "--model needs a concrete model ID; omit it to use the file default".into(),
            ));
        }
        let mut run = settings.clone();
        run.file_transcription_model = preference;
        return run
            .effective_file_model_for(language)
            .map_err(DictationError::from);
    }
    if needs_whisper {
        return settings
            .effective_whisper_file_model_for(language)
            .map_err(DictationError::from);
    }
    settings
        .effective_file_model_for(language)
        .map_err(DictationError::from)
}

/// Resolve a non-empty one-run hint and compose it through Settings' scoped
/// glossary helper. The helper owns legacy-global hint-only behavior and
/// selected explicit-profile aliases; CLI never parses those sources directly.
pub(crate) fn effective_glossary(
    stored: &Settings,
    profile_id: Option<&str>,
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&Path>,
) -> Result<Glossary, DictationError> {
    let one_run_prompt = resolve_one_run_prompt(cli_prompt, cli_prompt_file)?;
    let source =
        stored.effective_glossary_source_with_prompt(profile_id, one_run_prompt.as_deref());
    Ok(Glossary::parse(&source))
}

pub(crate) fn resolve_one_run_prompt(
    cli_prompt: Option<&str>,
    cli_prompt_file: Option<&Path>,
) -> Result<Option<String>, DictationError> {
    if let Some(path) = cli_prompt_file {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            DictationError::FileDecodeError(format!(
                "Failed to read prompt file '{}': {e}",
                path.display()
            ))
        })?;
        let trimmed = contents.trim();
        return Ok((!trimmed.is_empty()).then(|| trimmed.to_string()));
    }
    let Some(prompt) = cli_prompt else {
        return Ok(None);
    };
    let trimmed = prompt.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

/// A Whisper model as typed on the command line: its catalog id
/// (`WhisperModel::from_id`), which `sagascript config` takes as well (#373).
pub fn parse_model(s: &str) -> Result<WhisperModel, DictationError> {
    WhisperModel::from_id(s).ok_or_else(|| {
        DictationError::SettingsError(format!(
            "Unknown model '{s}'. Run 'sagascript list-models' to see available models."
        ))
    })
}

pub fn model_id_string(model: WhisperModel) -> &'static str {
    model.id()
}
