use std::path::PathBuf;

use clap::{Args, Subcommand};

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::error::DictationError;
use sagascript_core::settings;
use sagascript_core::settings::Language;
use sagascript_core::transcription::model;
use sagascript_core::transcription::{
    suggest_glossary_candidates, Glossary, GlossarySuggestion, GlossarySuggestionKind,
    WhisperBackend,
};

use crate::transcribe::{effective_glossary, model_id_string};

#[derive(Args)]
pub struct GlossaryArgs {
    #[command(subcommand)]
    pub action: GlossaryAction,
}

#[derive(Subcommand)]
pub enum GlossaryAction {
    /// Print the external personal-dictionary file path. The global file is
    /// legacy storage and supplies hint terms; use `--profile ID` for aliases.
    Path {
        /// Print this profile's dictionary path instead of the global path
        #[arg(long)]
        profile: Option<String>,
    },
    /// List canonical terms and their explicit aliases. The legacy global
    /// dictionary is retained and displayed, but its aliases are hint-only;
    /// deterministic replacement requires `--profile ID`.
    List {
        /// Show entries saved only for this dictation profile
        #[arg(long)]
        profile: Option<String>,
    },
    /// Add a term or merge aliases into an existing term. Global aliases are
    /// retained for compatibility but are hint-only at transcription time.
    Add {
        /// Preferred spelling written to the transcript
        term: String,
        /// Exact mishearing to replace (repeat for more aliases)
        #[arg(long = "alias", value_name = "TEXT")]
        aliases: Vec<String>,
        /// Save to this dictation profile instead of the legacy global dictionary
        #[arg(long)]
        profile: Option<String>,
    },
    /// Remove a canonical term and all of its aliases from the selected scope
    Remove {
        term: String,
        /// Remove from this dictation profile instead of the legacy global dictionary
        #[arg(long)]
        profile: Option<String>,
    },
    /// Remove every entry from the selected personal dictionary. Clearing the
    /// global dictionary is optional migration cleanup; it is not required to
    /// disable its aliases.
    Clear {
        /// Confirm destructive removal of the selected dictionary
        #[arg(long)]
        yes: bool,
        /// Clear this profile instead of the legacy global dictionary
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
        /// Dictation profile that owns learned entries
        #[arg(long)]
        profile: String,
        /// Emit a stable machine-readable result
        #[arg(long)]
        json: bool,
        /// Atomically add every displayed candidate; dry-run is the default
        #[arg(long)]
        apply: bool,
    },
}

pub fn run(args: GlossaryArgs) -> Result<(), DictationError> {
    match args.action {
        GlossaryAction::Path { profile } => path(profile.as_deref()),
        GlossaryAction::List { profile } => list(profile.as_deref()),
        GlossaryAction::Add { term, aliases, profile } => {
            add(&term, &aliases, profile.as_deref())
        }
        GlossaryAction::Remove { term, profile } => remove(&term, profile.as_deref()),
        GlossaryAction::Clear { yes, profile } => clear(yes, profile.as_deref()),
        GlossaryAction::Suggest { heard, corrected, profile, json, apply } => {
            suggest(&heard, &corrected, &profile, json, apply)
        }
    }
}

fn path(profile: Option<&str>) -> Result<(), DictationError> {
    let path = match profile {
        Some(profile) => {
            let stored = settings::store::load();
            validate_profile(&stored, profile)?;
            settings::store::profile_glossary_path(profile)
                .map_err(DictationError::SettingsError)?
        }
        None => settings::store::global_glossary_path(),
    };
    println!("{}", path.display());
    Ok(())
}

fn list(profile: Option<&str>) -> Result<(), DictationError> {
    let stored = settings::store::load();
    let source = glossary_source(&stored, profile)?;
    let glossary = Glossary::parse(source);
    for entry in glossary.entries() {
        if entry.aliases.is_empty() {
            println!("{}", entry.canonical);
        } else {
            println!("{} = {}", entry.canonical, entry.aliases.join(" | "));
        }
    }
    Ok(())
}

fn add(term: &str, aliases: &[String], profile: Option<&str>) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let aliases = aliases
        .iter()
        .map(|alias| validate_component("alias", alias))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(profile) = profile {
        validate_learning_profile(&settings::store::load(), profile)?;
    }

    settings::store::try_update(|stored| {
        if let Some(profile) = profile {
            validate_learning_profile(stored, profile).map_err(|error| error.to_string())?;
        }
        let source = glossary_source_mut(stored, profile)?;
        let mut glossary = Glossary::parse(source);
        glossary.upsert(term.clone(), aliases.clone());
        *source = glossary.render();
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;
    eprintln!("Saved personal dictionary term: {term}");
    Ok(())
}

fn remove(term: &str, profile: Option<&str>) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let mut removed = false;
    settings::store::try_update(|stored| {
        let source = glossary_source_mut(stored, profile)?;
        let mut glossary = Glossary::parse(source);
        removed = glossary.remove(&term);
        if removed {
            *source = glossary.render();
        }
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;

    if removed {
        eprintln!("Removed personal dictionary term: {term}");
        Ok(())
    } else {
        Err(DictationError::SettingsError(format!(
            "Personal dictionary term '{term}' was not found"
        )))
    }
}

fn clear(confirmed: bool, profile: Option<&str>) -> Result<(), DictationError> {
    if !confirmed {
        return Err(DictationError::SettingsError(
            "Refusing to clear the personal dictionary without --yes".to_string(),
        ));
    }
    settings::store::try_update(|stored| {
        glossary_source_mut(stored, profile)?.clear();
        Ok(())
    })
        .map_err(DictationError::SettingsError)?;
    eprintln!("Cleared personal dictionary");
    Ok(())
}

fn suggest(
    heard_path: &PathBuf,
    corrected_path: &PathBuf,
    profile: &str,
    json: bool,
    apply: bool,
) -> Result<(), DictationError> {
    let corrected = std::fs::read_to_string(corrected_path).map_err(|error| {
        DictationError::SettingsError(format!(
            "Failed to read corrected transcript '{}': {error}",
            corrected_path.display()
        ))
    })?;

    let stored = settings::store::load();
    validate_learning_profile(&stored, profile)?;
    let glossary = effective_glossary(&stored, Some(profile), None, None)?;
    let heard = load_training_input(heard_path, &stored, profile, &glossary)?;
    let suggestions = suggest_glossary_candidates(&heard, &corrected, &glossary);

    if apply {
        if suggestions.is_empty() {
            return Err(DictationError::SettingsError(
                "No safe dictionary suggestions to apply".to_string(),
            ));
        }
        let reviewed = suggestions.clone();
        settings::store::try_update(|latest| {
            validate_learning_profile(latest, profile).map_err(|error| error.to_string())?;
            let effective = effective_glossary(latest, Some(profile), None, None)
                .map_err(|error| error.to_string())?;
            let current = suggest_glossary_candidates(&heard, &corrected, &effective);
            if current != reviewed {
                return Err(
                    "Dictionary changed since the dry run; review suggestions again".to_string(),
                );
            }

            let source = latest.profile_glossaries.entry(profile.to_string()).or_default();
            let mut scoped = Glossary::parse(source);
            apply_candidates(&mut scoped, &reviewed);
            *source = scoped.render();
            Ok(())
        })
        .map_err(DictationError::SettingsError)?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "profile": profile,
                "applied": apply,
                "suggestions": suggestions,
            })
        );
    } else if suggestions.is_empty() {
        println!("No safe personal dictionary suggestions.");
    } else {
        for candidate in &suggestions {
            match candidate.kind {
                GlossarySuggestionKind::Alias => {
                    println!("{} = {}", candidate.canonical, candidate.observed);
                }
                GlossarySuggestionKind::HintOnly => println!("{}", candidate.canonical),
            }
        }
        if !apply {
            eprintln!("Dry run only. Re-run with --apply to save these profile-scoped entries.");
        }
    }
    Ok(())
}

fn load_training_input(
    path: &PathBuf,
    stored: &settings::Settings,
    profile_id: &str,
    glossary: &Glossary,
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
    let model = stored.effective_model_for(profile.language);
    if !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }

    let audio = decode_audio_file(path)?;
    if audio.is_empty() {
        return Err(DictationError::FileDecodeError(format!(
            "No audio decoded from '{}'",
            path.display()
        )));
    }
    eprintln!("Transcribing training input locally with {}...", model.display_name());
    let backend = WhisperBackend::new();
    backend.load_model(model)?;
    let raw = backend.transcribe_sync_with_progress_and_prompt(
        &audio,
        profile.language,
        glossary.decoder_prompt().as_deref(),
        |_| {},
    )?;
    Ok(glossary.correct_text(&raw).0)
}

fn apply_candidates(glossary: &mut Glossary, candidates: &[GlossarySuggestion]) {
    for candidate in candidates {
        match candidate.kind {
            GlossarySuggestionKind::Alias => {
                glossary.upsert(candidate.canonical.clone(), vec![candidate.observed.clone()]);
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
    match profile {
        Some(profile) => {
            if let Some(source) = stored.profile_glossaries.get(profile) {
                return Ok(source);
            }
            validate_profile(stored, profile)?;
            Ok("")
        }
        None => Ok(&stored.initial_prompt),
    }
}

fn glossary_source_mut<'a>(
    stored: &'a mut settings::Settings,
    profile: Option<&str>,
) -> Result<&'a mut String, String> {
    match profile {
        Some(profile) => {
            if !stored.profile_glossaries.contains_key(profile) {
                validate_profile(stored, profile).map_err(|error| error.to_string())?;
            }
            Ok(stored.profile_glossaries.entry(profile.to_string()).or_default())
        }
        None => Ok(&mut stored.initial_prompt),
    }
}

fn validate_profile(stored: &settings::Settings, profile: &str) -> Result<(), DictationError> {
    stored
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|candidate| candidate.id == profile)
        .ok_or_else(|| {
            DictationError::SettingsError(format!("Unknown dictation profile '{profile}'"))
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
            DictationError::SettingsError(format!("Unknown dictation profile '{profile}'"))
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
    fn legacy_global_alias_remains_stored_but_is_hint_only() {
        let stored = settings::Settings {
            initial_prompt: "merge = merch".to_string(),
            ..Default::default()
        };

        assert_eq!(glossary_source(&stored, None).unwrap(), "merge = merch");
        let effective = effective_glossary(&stored, None, None, None).unwrap();
        assert_eq!(effective.correct_text("merch").0, "merch");
        assert!(effective.decoder_prompt().is_some());
    }
}
