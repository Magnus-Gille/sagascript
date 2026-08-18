use clap::{Args, Subcommand};

use sagascript_core::error::DictationError;
use sagascript_core::settings;
use sagascript_core::transcription::Glossary;

#[derive(Args)]
pub struct GlossaryArgs {
    #[command(subcommand)]
    pub action: GlossaryAction,
}

#[derive(Subcommand)]
pub enum GlossaryAction {
    /// List canonical terms and their explicit aliases
    List,
    /// Add a term or merge aliases into an existing term
    Add {
        /// Preferred spelling written to the transcript
        term: String,
        /// Exact mishearing to replace (repeat for more aliases)
        #[arg(long = "alias", value_name = "TEXT")]
        aliases: Vec<String>,
    },
    /// Remove a canonical term and all of its aliases
    Remove { term: String },
    /// Remove every personal dictionary entry
    Clear {
        /// Confirm destructive removal of the complete dictionary
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(args: GlossaryArgs) -> Result<(), DictationError> {
    match args.action {
        GlossaryAction::List => list(),
        GlossaryAction::Add { term, aliases } => add(&term, &aliases),
        GlossaryAction::Remove { term } => remove(&term),
        GlossaryAction::Clear { yes } => clear(yes),
    }
}

fn list() -> Result<(), DictationError> {
    let stored = settings::store::load();
    let glossary = Glossary::parse(&stored.initial_prompt);
    for entry in glossary.entries() {
        if entry.aliases.is_empty() {
            println!("{}", entry.canonical);
        } else {
            println!("{} = {}", entry.canonical, entry.aliases.join(" | "));
        }
    }
    Ok(())
}

fn add(term: &str, aliases: &[String]) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let aliases = aliases
        .iter()
        .map(|alias| validate_component("alias", alias))
        .collect::<Result<Vec<_>, _>>()?;

    settings::store::update(|stored| {
        let mut glossary = Glossary::parse(&stored.initial_prompt);
        glossary.upsert(term.clone(), aliases.clone());
        stored.initial_prompt = glossary.render();
    })
    .map_err(DictationError::SettingsError)?;
    eprintln!("Saved personal dictionary term: {term}");
    Ok(())
}

fn remove(term: &str) -> Result<(), DictationError> {
    let term = validate_component("term", term)?;
    let mut removed = false;
    settings::store::update(|stored| {
        let mut glossary = Glossary::parse(&stored.initial_prompt);
        removed = glossary.remove(&term);
        if removed {
            stored.initial_prompt = glossary.render();
        }
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

fn clear(confirmed: bool) -> Result<(), DictationError> {
    if !confirmed {
        return Err(DictationError::SettingsError(
            "Refusing to clear the personal dictionary without --yes".to_string(),
        ));
    }
    settings::store::update(|stored| stored.initial_prompt.clear())
        .map_err(DictationError::SettingsError)?;
    eprintln!("Cleared personal dictionary");
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
}
