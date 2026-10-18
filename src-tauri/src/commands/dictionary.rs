//! The personal dictionary: per-profile glossaries and the training suggestions
//! that extend them.

use sagascript_core::settings::{Language, Settings};
use sagascript_core::transcription::{
    suggest_glossary_candidates, Glossary, GlossarySuggestion, GlossarySuggestionKind,
};
use tauri::State;
use tracing::info;

use super::{
    correct_in_memory, ensure_known_profile, lock_controller, try_persist_and_mirror,
    SharedController,
};
use crate::command_error::CommandError;

#[tauri::command]
pub async fn set_initial_prompt(
    controller: State<'_, SharedController>,
    prompt: String,
    expected_source: Option<String>,
) -> Result<(), CommandError> {
    let profile_id = lock_controller(&controller).settings().default_profile().id;
    set_profile_glossary(controller, profile_id, prompt, expected_source).await
}

/// Compare-and-set guard of a dictionary save: the stored source must still be
/// the one the editor read. The frontend branches on this error's code
/// (`dictionary_changed_elsewhere`), never on its text.
fn ensure_expected_dictionary_source(
    current_source: &str,
    expected_source: Option<&str>,
) -> Result<(), CommandError> {
    if expected_source.is_some_and(|expected| {
        !sagascript_core::settings::store::glossary_sources_match(expected, current_source)
    }) {
        return Err(CommandError::DictionaryChangedElsewhere);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DictionarySourceSnapshot {
    present: bool,
    source: String,
}

fn profile_dictionary_source_snapshot(
    settings: &Settings,
    profile_id: &str,
) -> DictionarySourceSnapshot {
    settings
        .profile_glossaries
        .get(profile_id)
        .map(|source| DictionarySourceSnapshot {
            present: true,
            source: source.clone(),
        })
        .unwrap_or_else(|| DictionarySourceSnapshot {
            present: false,
            source: String::new(),
        })
}

fn reconcile_profile_dictionary_after_conflict(
    settings: &mut Settings,
    profile_id: &str,
    expected_source: &str,
    persisted_source: &DictionarySourceSnapshot,
) {
    let current_source = settings
        .profile_glossaries
        .get(profile_id)
        .map(String::as_str)
        .unwrap_or_default();
    if !sagascript_core::settings::store::glossary_sources_match(current_source, expected_source) {
        return;
    }

    if persisted_source.present {
        settings
            .profile_glossaries
            .insert(profile_id.to_string(), persisted_source.source.clone());
    } else {
        settings.profile_glossaries.remove(profile_id);
    }
}

fn ensure_learning_profile(settings: &Settings, profile_id: &str) -> Result<(), String> {
    ensure_known_profile(settings, profile_id)?;
    if settings
        .resolved_hotkey_profiles()
        .iter()
        .any(|profile| profile.id == profile_id && profile.language == Language::Auto)
    {
        return Err(
            "Deterministic dictionary training requires a profile with an explicit language".into(),
        );
    }
    Ok(())
}

/// The fields a dictionary save writes: the controller takes the written
/// `profile_glossaries` and nothing else, so a saved dictionary reaches the
/// controller without the shortcuts or the demotion the file differs in.
fn mirror_profile_glossaries(memory: &mut Settings, written: &Settings) {
    memory.profile_glossaries = written.profile_glossaries.clone();
}

fn set_profile_glossary_in(
    settings: &mut Settings,
    profile_id: &str,
    source: String,
    expected_source: Option<&str>,
) -> Result<(), CommandError> {
    ensure_known_profile(settings, profile_id)?;
    let auto_language = settings
        .resolved_hotkey_profiles()
        .iter()
        .any(|profile| profile.id == profile_id && profile.language == Language::Auto);
    if auto_language
        && Glossary::parse(&source)
            .entries()
            .iter()
            .any(|entry| !entry.aliases.is_empty())
    {
        return Err(
            "Auto-detect profiles support dictionary hints, not deterministic aliases".into(),
        );
    }
    let current_source = settings
        .profile_glossaries
        .get(profile_id)
        .map(String::as_str)
        .unwrap_or_default();
    ensure_expected_dictionary_source(current_source, expected_source)?;
    settings
        .profile_glossaries
        .insert(profile_id.to_owned(), source);
    Ok(())
}

#[tauri::command]
pub async fn set_profile_glossary(
    controller: State<'_, SharedController>,
    profile_id: String,
    source: String,
    expected_source: Option<String>,
) -> Result<(), CommandError> {
    let source_len = source.chars().count();
    let mut conflict_source = None;
    // Only the dictionaries, like `apply_training_glossary`: the controller
    // takes the written `profile_glossaries` and nothing else.
    let saved = try_persist_and_mirror(
        &controller,
        |settings| {
            let current_source = expected_source
                .as_ref()
                .map(|_| profile_dictionary_source_snapshot(settings, &profile_id));
            set_profile_glossary_in(settings, &profile_id, source, expected_source.as_deref())
                .inspect_err(|error| {
                    if matches!(error, CommandError::DictionaryChangedElsewhere) {
                        conflict_source = current_source;
                    }
                })
        },
        mirror_profile_glossaries,
    );
    match saved {
        Ok(_) => {
            info!(source_len, "Profile personal dictionary updated");
            Ok(())
        }
        Err(error) => {
            if let (Some(expected), Some(persisted_source)) =
                (expected_source.as_deref(), conflict_source.as_ref())
            {
                correct_in_memory(&controller, |settings| {
                    reconcile_profile_dictionary_after_conflict(
                        settings,
                        &profile_id,
                        expected,
                        persisted_source,
                    );
                });
            }
            Err(error)
        }
    }
}

fn apply_reviewed_training_candidates(
    settings: &mut Settings,
    heard: &str,
    corrected: &str,
    profile_id: &str,
    accepted: &[GlossarySuggestion],
) -> Result<(), String> {
    ensure_learning_profile(settings, profile_id)?;
    let effective = Glossary::parse(&settings.effective_glossary_source(Some(profile_id)));
    let allowed = suggest_glossary_candidates(heard, corrected, &effective);
    let mut matched = vec![false; allowed.len()];
    for candidate in accepted {
        validate_edited_training_candidate(candidate)?;
        let canonical_key = training_term_key(&candidate.canonical);
        let conflicts_with_existing_alias = effective.entries().iter().any(|entry| {
            entry
                .aliases
                .iter()
                .any(|alias| training_term_key(alias) == canonical_key)
        });
        let conflicts_with_reviewed_alias = accepted.iter().any(|other| {
            other.kind == GlossarySuggestionKind::Alias
                && training_term_key(&other.observed) == canonical_key
        });
        if conflicts_with_existing_alias || conflicts_with_reviewed_alias {
            return Err(
                "Edited preferred spelling conflicts with a personal dictionary alias".to_string(),
            );
        }
        let Some((index, _)) = allowed.iter().enumerate().find(|(index, original)| {
            !matched[*index]
                && original.observed == candidate.observed
                && original.context == candidate.context
                && match (original.kind, candidate.kind) {
                    (GlossarySuggestionKind::Alias, GlossarySuggestionKind::Alias)
                    | (GlossarySuggestionKind::Alias, GlossarySuggestionKind::HintOnly)
                    | (GlossarySuggestionKind::HintOnly, GlossarySuggestionKind::HintOnly) => true,
                    (GlossarySuggestionKind::HintOnly, GlossarySuggestionKind::Alias) => false,
                }
        }) else {
            return Err(
                "Dictionary suggestions changed; review the corrected transcript again".to_string(),
            );
        };
        matched[index] = true;
    }

    let source = settings
        .profile_glossaries
        .entry(profile_id.to_string())
        .or_default();
    let mut scoped = Glossary::parse(source);
    for candidate in accepted {
        match candidate.kind {
            GlossarySuggestionKind::Alias => {
                scoped.upsert(
                    candidate.canonical.clone(),
                    vec![candidate.observed.clone()],
                );
            }
            GlossarySuggestionKind::HintOnly => {
                scoped.upsert(candidate.canonical.clone(), Vec::new());
            }
        }
    }
    *source = scoped.render();
    Ok(())
}

fn training_term_key(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn validate_edited_training_candidate(candidate: &GlossarySuggestion) -> Result<(), String> {
    let canonical = candidate.canonical.trim();
    if canonical.is_empty()
        || canonical != candidate.canonical
        || canonical.len() > 96
        || canonical.split_whitespace().count() > 4
        || canonical
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | ',' | '=' | '|'))
    {
        return Err(
            "Edited dictionary terms must be 1-4 words without glossary delimiters".to_string(),
        );
    }
    Ok(())
}

#[tauri::command]
pub async fn suggest_training_glossary(
    heard: String,
    corrected: String,
    profile_id: String,
) -> Result<Vec<GlossarySuggestion>, CommandError> {
    let settings = sagascript_core::machine::load();
    ensure_learning_profile(&settings, &profile_id)?;
    let glossary = Glossary::parse(&settings.effective_glossary_source(Some(&profile_id)));
    Ok(suggest_glossary_candidates(&heard, &corrected, &glossary))
}

#[tauri::command]
pub async fn apply_training_glossary(
    controller: State<'_, SharedController>,
    heard: String,
    corrected: String,
    profile_id: String,
    accepted: Vec<GlossarySuggestion>,
) -> Result<(), CommandError> {
    if accepted.is_empty() {
        return Err("Select at least one dictionary suggestion".into());
    }

    let accepted_count = accepted.len();
    try_persist_and_mirror(
        &controller,
        |settings| {
            apply_reviewed_training_candidates(settings, &heard, &corrected, &profile_id, &accepted)
        },
        mirror_profile_glossaries,
    )?;
    info!(accepted_count, profile_id = %profile_id, "Applied reviewed training glossary suggestions");
    Ok(())
}

#[cfg(test)]
#[path = "dictionary_tests.rs"]
mod tests;
