//! Language and model resolution of a file run, and the one-run glossary
//! (#343): the IDs `--language` and `--model` accept, and which model a run
//! gets when none is named.

use std::path::Path;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{FileModel, FileModelPreference, Language, Settings, WhisperModel};
use sagascript_core::transcription::Glossary;

pub fn parse_language(s: &str) -> Result<Language, DictationError> {
    match s {
        "en" | "english" => Ok(Language::English),
        "sv" | "swedish" => Ok(Language::Swedish),
        "no" | "norwegian" => Ok(Language::Norwegian),
        "fi" | "finnish" => Ok(Language::Finnish),
        "auto" => Ok(Language::Auto),
        other => Err(DictationError::SettingsError(format!(
            "Unknown language '{other}'. Valid: en, sv, no, fi, auto"
        ))),
    }
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
            FileModelPreference::parse_id(id).map_err(DictationError::SettingsError)?;
        if preference == FileModelPreference::Auto {
            return Err(DictationError::SettingsError(
                "--model needs a concrete model ID; omit it to use the file default".into(),
            ));
        }
        let mut run = settings.clone();
        run.file_transcription_model = preference;
        return run
            .effective_file_model_for(language)
            .map_err(DictationError::SettingsError);
    }
    if needs_whisper {
        return settings
            .effective_whisper_file_model_for(language)
            .map_err(DictationError::SettingsError);
    }
    settings
        .effective_file_model_for(language)
        .map_err(DictationError::SettingsError)
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

pub fn parse_model(s: &str) -> Result<WhisperModel, DictationError> {
    match s {
        "tiny.en" => Ok(WhisperModel::TinyEn),
        "tiny" => Ok(WhisperModel::Tiny),
        "base.en" => Ok(WhisperModel::BaseEn),
        "base" => Ok(WhisperModel::Base),
        "fi-whisper-tiny" => Ok(WhisperModel::FinnishWhisperTiny),
        "kb-whisper-tiny" => Ok(WhisperModel::KbWhisperTiny),
        "kb-whisper-base" => Ok(WhisperModel::KbWhisperBase),
        "kb-whisper-small" => Ok(WhisperModel::KbWhisperSmall),
        "kb-whisper-medium" => Ok(WhisperModel::KbWhisperMedium),
        "kb-whisper-large" => Ok(WhisperModel::KbWhisperLarge),
        "nb-whisper-tiny" => Ok(WhisperModel::NbWhisperTiny),
        "nb-whisper-base" => Ok(WhisperModel::NbWhisperBase),
        "nb-whisper-small" => Ok(WhisperModel::NbWhisperSmall),
        "nb-whisper-medium" => Ok(WhisperModel::NbWhisperMedium),
        "nb-whisper-large" => Ok(WhisperModel::NbWhisperLarge),
        "small.en" => Ok(WhisperModel::SmallEn),
        "small" => Ok(WhisperModel::Small),
        "medium.en" => Ok(WhisperModel::MediumEn),
        "medium" => Ok(WhisperModel::Medium),
        "large-v3-turbo" => Ok(WhisperModel::LargeV3Turbo),
        "large-v3-turbo-q8_0" => Ok(WhisperModel::LargeV3TurboQ8),
        other => Err(DictationError::SettingsError(format!(
            "Unknown model '{other}'. Run 'sagascript list-models' to see available models."
        ))),
    }
}

pub fn model_id_string(model: WhisperModel) -> &'static str {
    match model {
        WhisperModel::TinyEn => "tiny.en",
        WhisperModel::Tiny => "tiny",
        WhisperModel::BaseEn => "base.en",
        WhisperModel::Base => "base",
        WhisperModel::FinnishWhisperTiny => "fi-whisper-tiny",
        WhisperModel::KbWhisperTiny => "kb-whisper-tiny",
        WhisperModel::KbWhisperBase => "kb-whisper-base",
        WhisperModel::KbWhisperSmall => "kb-whisper-small",
        WhisperModel::KbWhisperMedium => "kb-whisper-medium",
        WhisperModel::KbWhisperLarge => "kb-whisper-large",
        WhisperModel::NbWhisperTiny => "nb-whisper-tiny",
        WhisperModel::NbWhisperBase => "nb-whisper-base",
        WhisperModel::NbWhisperSmall => "nb-whisper-small",
        WhisperModel::NbWhisperMedium => "nb-whisper-medium",
        WhisperModel::NbWhisperLarge => "nb-whisper-large",
        WhisperModel::SmallEn => "small.en",
        WhisperModel::Small => "small",
        WhisperModel::MediumEn => "medium.en",
        WhisperModel::Medium => "medium",
        WhisperModel::LargeV3Turbo => "large-v3-turbo",
        WhisperModel::LargeV3TurboQ8 => "large-v3-turbo-q8_0",
    }
}
