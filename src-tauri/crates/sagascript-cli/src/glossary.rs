use std::path::PathBuf;

use clap::{Args, Subcommand};

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::error::DictationError;
use sagascript_core::machine;
use sagascript_core::settings;
use sagascript_core::settings::{FileModel, Language, SettingsError};
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::{
    suggest_glossary_candidates, Glossary, GlossarySuggestion, GlossarySuggestionKind,
    TranscribeOptions, TranscribeRequest, WhisperBackend,
};

use crate::output::{errln, outln, Out};
use crate::transcribe::{effective_glossary, model_id_string};

#[derive(Args)]
pub struct GlossaryArgs {
    #[command(subcommand)]
    pub action: GlossaryAction,
}

#[derive(Subcommand)]
pub enum GlossaryAction {
    /// Print a profile's personal-dictionary path (default profile if omitted).
    Path {
        /// Choose a profile instead of the default profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// List the selected profile's canonical terms and aliases.
    List {
        /// Show entries saved only for this dictation profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// Add a term or merge aliases into a profile dictionary.
    Add {
        /// Preferred spelling written to the transcript
        term: String,
        /// Exact mishearing to replace (repeat for more aliases)
        #[arg(long = "alias", value_name = "TEXT")]
        aliases: Vec<String>,
        /// Choose a profile instead of the default profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// Remove a canonical term and all of its aliases from the selected scope
    Remove {
        term: String,
        /// Choose a profile instead of the default profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// Remove every entry from the selected profile dictionary.
    Clear {
        /// Confirm destructive removal of the selected dictionary
        #[arg(long)]
        yes: bool,
        /// Choose a profile instead of the default profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// Compare a transcript with its manual correction and propose safe entries
    Suggest {
        /// Audio/video file to transcribe, or a UTF-8 .txt/.md transcript
        heard: PathBuf,
        /// UTF-8 text file containing the final corrected transcript
        #[arg(long, value_name = "FILE")]
        corrected: PathBuf,
        /// Dictation profile that owns learned entries (default profile if omitted)
        #[arg(long)]
        profile: Option<String>,
        /// Emit a stable machine-readable result
        #[arg(long)]
        json: bool,
        /// Atomically add every displayed candidate; dry-run is the default
        #[arg(long)]
        apply: bool,
    },
}

pub fn run(args: GlossaryArgs, out: &Out) -> Result<(), DictationError> {
    match args.action {
        GlossaryAction::Path { profile } => path(profile.as_deref(), out),
        GlossaryAction::List { profile } => list(profile.as_deref(), out),
        GlossaryAction::Add {
            term,
            aliases,
            profile,
        } => add(&term, &aliases, profile.as_deref(), out),
        GlossaryAction::Remove { term, profile } => remove(&term, profile.as_deref(), out),
        GlossaryAction::Clear { yes, profile } => clear(yes, profile.as_deref(), out),
        GlossaryAction::Suggest {
            heard,
            corrected,
            profile,
            json,
            apply,
        } => suggest(&heard, &corrected, profile.as_deref(), json, apply, out),
    }
}

fn path(profile: Option<&str>, out: &Out) -> Result<(), DictationError> {
    let stored = machine::load();
    let profile = resolved_profile_id(&stored, profile)?;
    let path = settings::store::profile_glossary_path(&profile).map_err(DictationError::from)?;
    outln!(out, "{}", path.display())?;
    Ok(())
}

fn list(profile: Option<&str>, out: &Out) -> Result<(), DictationError> {
    let stored = machine::load();
    let source = glossary_source(&stored, profile)?;
    let glossary = Glossary::parse(source);
    for entry in glossary.entries() {
        if entry.aliases.is_empty() {
            outln!(out, "{}", entry.canonical)?;
        } else {
            outln!(out, "{} = {}", entry.canonical, entry.aliases.join(" | "))?;
        }
    }
    Ok(())
}

fn add(
    term: &str,
    aliases: &[String],
    profile: Option<&str>,
    out: &Out,
) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let aliases = aliases
        .iter()
        .map(|alias| validate_component("alias", alias))
        .collect::<Result<Vec<_>, _>>()?;
    let current = machine::load();
    let profile_id = resolved_profile_id(&current, profile)?;
    if profile.is_some() {
        validate_profile(&current, &profile_id)?;
    }
    if !aliases.is_empty() {
        validate_learning_profile(&current, &profile_id)?;
    }

    machine::try_update::<DictationError, _>(|stored| {
        let profile_id = resolved_profile_id(stored, profile)?;
        if profile.is_some() {
            validate_profile(stored, &profile_id)?;
        }
        if !aliases.is_empty() {
            validate_learning_profile(stored, &profile_id)?;
        }
        let source = glossary_source_mut(stored, profile)?;
        let mut glossary = Glossary::parse(source);
        glossary.upsert(term.clone(), aliases.clone());
        *source = glossary.render();
        Ok(())
    })?;
    errln!(out, "Saved personal dictionary term: {term}");
    Ok(())
}

fn remove(term: &str, profile: Option<&str>, out: &Out) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let mut removed = false;
    machine::try_update::<DictationError, _>(|stored| {
        let source = glossary_source_mut(stored, profile)?;
        let mut glossary = Glossary::parse(source);
        removed = glossary.remove(&term);
        if removed {
            *source = glossary.render();
        }
        Ok(())
    })?;

    if removed {
        errln!(out, "Removed personal dictionary term: {term}");
        Ok(())
    } else {
        Err(DictationError::SettingsError(format!(
            "Personal dictionary term '{term}' was not found"
        )))
    }
}

fn clear(confirmed: bool, profile: Option<&str>, out: &Out) -> Result<(), DictationError> {
    if !confirmed {
        return Err(DictationError::SettingsError(
            "Refusing to clear the personal dictionary without --yes".to_string(),
        ));
    }
    machine::try_update::<DictationError, _>(|stored| {
        glossary_source_mut(stored, profile)?.clear();
        Ok(())
    })?;
    errln!(out, "Cleared personal dictionary");
    Ok(())
}

fn suggest(
    heard_path: &PathBuf,
    corrected_path: &PathBuf,
    profile: Option<&str>,
    json: bool,
    apply: bool,
    out: &Out,
) -> Result<(), DictationError> {
    let corrected = std::fs::read_to_string(corrected_path).map_err(|error| {
        DictationError::SettingsError(format!(
            "Failed to read corrected transcript '{}': {error}",
            corrected_path.display()
        ))
    })?;

    let stored = machine::load();
    let profile_id = resolved_profile_id(&stored, profile)?;
    validate_learning_profile(&stored, &profile_id)?;
    let glossary = effective_glossary(&stored, Some(&profile_id), None, None)?;
    let heard = load_training_input(heard_path, &stored, &profile_id, &glossary, out)?;
    let suggestions = suggest_glossary_candidates(&heard, &corrected, &glossary);

    if apply {
        if suggestions.is_empty() {
            return Err(DictationError::SettingsError(
                "No safe dictionary suggestions to apply".to_string(),
            ));
        }
        let reviewed = suggestions.clone();
        machine::try_update::<DictationError, _>(|latest| {
            validate_learning_profile(latest, &profile_id)?;
            let effective = effective_glossary(latest, Some(&profile_id), None, None)?;
            let current = suggest_glossary_candidates(&heard, &corrected, &effective);
            if current != reviewed {
                return Err(DictationError::SettingsError(
                    "Dictionary changed since the dry run; review suggestions again".to_string(),
                ));
            }

            let source = latest
                .profile_glossaries
                .entry(profile_id.clone())
                .or_default();
            let mut scoped = Glossary::parse(source);
            apply_candidates(&mut scoped, &reviewed);
            *source = scoped.render();
            Ok(())
        })?;
    }

    if json {
        outln!(
            out,
            "{}",
            serde_json::json!({
                "profile": profile_id,
                "applied": apply,
                "suggestions": suggestions,
            })
        )?;
    } else if suggestions.is_empty() {
        outln!(out, "No safe personal dictionary suggestions.")?;
    } else {
        for candidate in &suggestions {
            match candidate.kind {
                GlossarySuggestionKind::Alias => {
                    outln!(out, "{} = {}", candidate.canonical, candidate.observed)?;
                }
                GlossarySuggestionKind::HintOnly => outln!(out, "{}", candidate.canonical)?,
            }
        }
        if !apply {
            errln!(
                out,
                "Dry run only. Re-run with --apply to save these profile-scoped entries."
            );
        }
    }
    Ok(())
}

fn load_training_input(
    path: &PathBuf,
    stored: &settings::Settings,
    profile_id: &str,
    glossary: &Glossary,
    out: &Out,
) -> Result<String, DictationError> {
    let is_text = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "txt" | "md"));
    if is_text {
        return std::fs::read_to_string(path).map_err(|error| {
            DictationError::SettingsError(format!(
                "Failed to read transcript '{}': {error}",
                path.display()
            ))
        });
    }

    let profile = stored
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .expect("validated profile must still exist");
    let choice = stored
        .dictation_model_for_profile(profile_id)
        .map_err(DictationError::from)?;
    match choice {
        FileModel::Whisper(model) if !model::is_model_downloaded(model) => {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' is not downloaded. Run: sagascript download-model {}",
                model.display_name(),
                model_id_string(model)
            )));
        }
        FileModel::PianissimoOriginal if !pianissimo_model::is_downloaded() => {
            return Err(DictationError::TranscriptionFailed(
                "Pianissimo is not downloaded. Run: sagascript download-model pianissimo-sv".into(),
            ));
        }
        _ => {}
    }

    let audio = decode_audio_file(path)?;
    if audio.is_empty() {
        return Err(DictationError::FileDecodeError(format!(
            "No audio decoded from '{}'",
            path.display()
        )));
    }
    let raw = match choice {
        FileModel::Whisper(model) => {
            errln!(
                out,
                "Transcribing training input locally with {}...",
                model.display_name()
            );
            let backend = WhisperBackend::new();
            backend.load_model(model)?;
            let options = TranscribeOptions {
                prompt: glossary.decoder_prompt(),
                ..TranscribeOptions::default()
            };
            backend.transcribe_text(TranscribeRequest::new(&audio, profile.language, &options))?
        }
        FileModel::PianissimoOriginal => {
            errln!(
                out,
                "Transcribing training input locally with Pianissimo..."
            );
            PianissimoBackend::start(&machine::load())?
                .transcribe(&audio, |_| {})?
                .text
        }
    };
    Ok(glossary.correct_text(&raw).0)
}

fn apply_candidates(glossary: &mut Glossary, candidates: &[GlossarySuggestion]) {
    for candidate in candidates {
        match candidate.kind {
            GlossarySuggestionKind::Alias => {
                glossary.upsert(
                    candidate.canonical.clone(),
                    vec![candidate.observed.clone()],
                );
            }
            GlossarySuggestionKind::HintOnly => {
                glossary.upsert(candidate.canonical.clone(), Vec::new());
            }
        }
    }
}

fn glossary_source<'a>(
    stored: &'a settings::Settings,
    profile: Option<&str>,
) -> Result<&'a str, DictationError> {
    let id = resolved_profile_id(stored, profile)?;
    Ok(stored
        .profile_glossaries
        .get(&id)
        .map(String::as_str)
        .unwrap_or(""))
}

fn glossary_source_mut<'a>(
    stored: &'a mut settings::Settings,
    profile: Option<&str>,
) -> Result<&'a mut String, DictationError> {
    let id = resolved_profile_id(stored, profile)?;
    Ok(stored.profile_glossaries.entry(id).or_default())
}

fn resolved_profile_id(
    stored: &settings::Settings,
    profile: Option<&str>,
) -> Result<String, DictationError> {
    if let Some(id) = profile {
        if stored.profile_glossaries.contains_key(id) {
            return Ok(id.to_string());
        }
    }
    let id = profile
        .map(str::to_string)
        .unwrap_or_else(|| stored.default_profile().id);
    validate_profile(stored, &id)?;
    Ok(id)
}

fn validate_profile(stored: &settings::Settings, profile: &str) -> Result<(), DictationError> {
    stored
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|candidate| candidate.id == profile)
        .ok_or_else(|| {
            DictationError::from(SettingsError::UnknownDictationProfile {
                id: profile.to_string(),
            })
        })?;
    Ok(())
}

fn validate_learning_profile(
    stored: &settings::Settings,
    profile: &str,
) -> Result<(), DictationError> {
    let profile = stored
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|candidate| candidate.id == profile)
        .ok_or_else(|| {
            DictationError::from(SettingsError::UnknownDictationProfile {
                id: profile.to_string(),
            })
        })?;
    if profile.language == Language::Auto {
        return Err(DictationError::SettingsError(
            "Glossary learning requires a profile with an explicit language".to_string(),
        ));
    }
    Ok(())
}

fn validate_component(label: &str, value: &str) -> Result<String, DictationError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DictationError::SettingsError(format!(
            "Glossary {label} cannot be empty"
        )));
    }
    if value
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | ',' | '=' | '|'))
    {
        return Err(DictationError::SettingsError(format!(
            "Glossary {label} cannot contain a newline, comma, '=' or '|'"
        )));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_grammar_delimiters_in_cli_values() {
        assert!(validate_component("term", "OpenRouter").is_ok());
        assert!(validate_component("alias", "open router").is_ok());
        assert!(validate_component("alias", "open router | router").is_err());
    }

    #[test]
    fn orphaned_profile_dictionary_remains_listable_and_clearable() {
        let mut stored = settings::Settings::default();
        stored
            .profile_glossaries
            .insert("removed".to_string(), "Lovable = love a ball".to_string());

        assert_eq!(
            glossary_source(&stored, Some("removed")).unwrap(),
            "Lovable = love a ball"
        );
        glossary_source_mut(&mut stored, Some("removed"))
            .unwrap()
            .clear();
        assert_eq!(stored.profile_glossaries.get("removed").unwrap(), "");
    }

    #[test]
    fn legacy_global_alias_migrates_as_hint_to_default_profile() {
        let mut stored = settings::Settings {
            initial_prompt: "merge = merch".to_string(),
            ..Default::default()
        };
        stored.materialize_profile_models();
        stored.migrate_global_glossary_to_profiles();
        assert_eq!(stored.initial_prompt, "merge = merch"); // rollback source is retained
        assert_eq!(glossary_source(&stored, None).unwrap(), "merge");
        let effective = effective_glossary(&stored, None, None, None).unwrap();
        assert_eq!(effective.correct_text("merch").0, "merch");
        assert!(effective.decoder_prompt().is_some());
    }
}
