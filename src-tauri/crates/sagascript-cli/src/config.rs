use clap::{Args, Subcommand};

use sagascript_core::error::DictationError;
use sagascript_core::machine;
use sagascript_core::settings::{
    self, FileModelPreference, HotkeyProfile, Language, ModelResetNotice, Settings,
};

use crate::output::{errln, outln, Out};

mod keys;
use keys::{lookup, parse_value, KEYS};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show all settings with current and default values
    #[command(long_about = list_long_about())]
    List,

    /// Get a single setting value
    #[command(
        long_about = get_long_about(),
        after_long_help = "\
EXAMPLES:
  sagascript config get language
  sagascript config get hotkey
  sagascript config get initial_prompt"
    )]
    Get {
        #[arg(help = key_help())]
        key: String,
    },

    /// Set a setting value
    #[command(
        long_about = set_long_about(),
        after_long_help = "\
EXAMPLES:
  sagascript config set language sv
  sagascript config set whisper_model kb-whisper-base
  sagascript config set hotkey 'Option+Space'
  sagascript config set hotkey F13
  sagascript config set auto_paste false
  sagascript config set pianissimo_dictation true
  sagascript config set initial_prompt $'OpenRouter = open router | open vrouter\\nmerge = merch'"
    )]
    Set {
        #[arg(help = key_help())]
        key: String,
        /// New value for the setting
        value: String,
    },

    /// Reset one or all settings to defaults
    #[command(
        long_about = "\
Reset a single application setting or all application settings to their default values.

If KEY is provided, only that setting is reset. \
If KEY is omitted, all application settings are reset. External personal \
 dictionaries are preserved. `sagascript glossary clear --yes` clears the \
 default profile; add `--profile ID` for another dictionary.",
        after_long_help = "\
EXAMPLES:
  # Reset just the language
  sagascript config reset language
  sagascript config reset pianissimo_dictation

  # Reset everything
  sagascript config reset"
    )]
    Reset {
        /// Setting key to reset (omit to reset all)
        key: Option<String>,
    },

    /// Print the settings file path
    #[command(long_about = "\
Print the absolute path to the settings JSON file. Use `sagascript glossary \
path` for the separate personal dictionary.")]
    Path,

    /// Manage per-shortcut dictation profiles with language and model
    #[command(long_about = "\
Manage dictation profiles. Each profile owns its language, shortcut(s), \
dictionary and model. The model is `auto` (the recommended model for the \
profile's language, the default), a Whisper model compatible with that \
language, or `pianissimo-sv` (Swedish only). File transcription has its own \
separate model choice (`config set file_transcription_model`).

If a profile's language changes to one its model does not support, that \
profile's model is reset to `auto` and a notice naming the new recommended \
model is printed to stderr; the command still succeeds and other profiles \
are never changed. A compatible explicit model is kept.")]
    Profiles {
        #[command(subcommand)]
        action: ProfileAction,
    },
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// List all dictation profiles
    List,
    /// Create a dictation profile
    #[command(alias = "add")]
    Create {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(
            long = "push-to-talk-shortcut",
            conflicts_with = "clear_push_to_talk_shortcut"
        )]
        push_to_talk_shortcut: Option<String>,
        #[arg(long = "toggle-shortcut", conflicts_with = "clear_toggle_shortcut")]
        toggle_shortcut: Option<String>,
        #[arg(long = "clear-push-to-talk-shortcut")]
        clear_push_to_talk_shortcut: bool,
        #[arg(long = "clear-toggle-shortcut")]
        clear_toggle_shortcut: bool,
        #[arg(long)]
        language: String,
        /// Live dictation model ID (auto, compatible Whisper model, or Swedish pianissimo-sv)
        #[arg(long)]
        model: Option<String>,
    },
    /// Update a dictation profile
    #[command(alias = "set")]
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(
            long = "push-to-talk-shortcut",
            conflicts_with = "clear_push_to_talk_shortcut"
        )]
        push_to_talk_shortcut: Option<String>,
        #[arg(long = "toggle-shortcut", conflicts_with = "clear_toggle_shortcut")]
        toggle_shortcut: Option<String>,
        #[arg(long = "clear-push-to-talk-shortcut")]
        clear_push_to_talk_shortcut: bool,
        #[arg(long = "clear-toggle-shortcut")]
        clear_toggle_shortcut: bool,
        #[arg(long)]
        language: Option<String>,
        /// Live dictation model ID (auto, compatible Whisper model, or Swedish pianissimo-sv)
        #[arg(long)]
        model: Option<String>,
    },
    /// Remove a dictation profile (at least one must remain)
    Remove { id: String },
}

/// The part of `config set --help` before the table of values.
const SET_INTRO: &str = "\
Update a setting. The new value takes effect immediately — the GUI \
 hot-reloads changes made via CLI. Legacy language, whisper_model, auto_select_model, \
 pianissimo_dictation, and initial_prompt keys edit the default profile. The \
 initial_prompt alias only merges entries and never erases saved aliases; prefer \
 `config profiles update ID --language ... --model ...`.";

// The key lists and the table of values in the help come from the key table
// (`keys::KEYS`), so a key cannot be missing from one of them.

fn list_long_about() -> String {
    format!(
        "Show all settings in a table with their current values and defaults.\n\nValid keys: {}.",
        keys::key_list_help()
    )
}

fn get_long_about() -> String {
    format!(
        "Print the current value of a single setting to stdout.\n\nValid keys: {}",
        keys::key_list_help()
    )
}

fn set_long_about() -> String {
    format!(
        "{SET_INTRO}\n\nValid values per key:\n{}",
        keys::valid_values_help()
    )
}

fn key_help() -> String {
    format!("Setting key [possible values: {}]", keys::possible_values())
}

pub fn run(args: ConfigArgs, out: &Out) -> Result<(), DictationError> {
    match args.action {
        ConfigAction::List => cmd_list(out),
        ConfigAction::Get { key } => cmd_get(&key, out),
        ConfigAction::Set { key, value } => cmd_set(&key, &value, out),
        ConfigAction::Reset { key } => cmd_reset(key.as_deref(), out),
        ConfigAction::Path => cmd_path(out),
        ConfigAction::Profiles { action } => cmd_profiles(action, out),
    }
}

fn cmd_profiles(action: ProfileAction, out: &Out) -> Result<(), DictationError> {
    match action {
        ProfileAction::List => {
            outln!(
                out,
                "{:<16} {:<20} {:<28} {:<28} {:<28} {:<12} MODEL",
                "ID",
                "NAME",
                "LEGACY",
                "PUSH-TO-TALK",
                "TOGGLE",
                "LANGUAGE"
            )?;
            let settings = machine::load();
            for profile in settings.resolved_hotkey_profiles() {
                let model = settings
                    .profile_models
                    .get(&profile.id)
                    .copied()
                    .unwrap_or(FileModelPreference::Auto);
                outln!(
                    out,
                    "{:<16} {:<20} {:<28} {:<28} {:<28} {:<12} {}",
                    profile.id,
                    profile.name,
                    profile.shortcut,
                    profile.push_to_talk_shortcut.as_deref().unwrap_or("-"),
                    profile.toggle_shortcut.as_deref().unwrap_or("-"),
                    profile.language.id(),
                    model.id(),
                )?;
            }
            Ok(())
        }
        ProfileAction::Create {
            id,
            name,
            hotkey,
            push_to_talk_shortcut,
            toggle_shortcut,
            clear_push_to_talk_shortcut,
            clear_toggle_shortcut,
            language,
            model,
        } => {
            let language = parse_value::<Language>(&language, "language")?;
            let model = model
                .as_deref()
                .map(FileModelPreference::parse_id)
                .transpose()
                .map_err(DictationError::SettingsError)?;
            if clear_push_to_talk_shortcut || clear_toggle_shortcut {
                return Err(DictationError::SettingsError(
                    "Clear shortcut options are only valid when updating a profile".to_string(),
                ));
            }
            let shortcut = hotkey
                .or_else(|| push_to_talk_shortcut.clone())
                .or_else(|| toggle_shortcut.clone())
                .ok_or_else(|| {
                    DictationError::SettingsError(
                        "Specify --hotkey or at least one explicit shortcut".to_string(),
                    )
                })?;
            let hotkey_warnings = [
                shortcut_warning(&shortcut),
                push_to_talk_shortcut.as_deref().and_then(shortcut_warning),
                toggle_shortcut.as_deref().and_then(shortcut_warning),
            ];
            let mut profiles = machine::load().resolved_hotkey_profiles();
            if profiles.iter().any(|profile| profile.id == id) {
                return Err(DictationError::SettingsError(format!(
                    "Profile '{id}' already exists"
                )));
            }
            profiles.push(HotkeyProfile {
                id: id.clone(),
                name,
                shortcut,
                language,
                push_to_talk_shortcut,
                toggle_shortcut,
            });
            print_notices(
                &persist_profiles(profiles, model.map(|choice| (id.as_str(), choice)))?,
                out,
            );
            errln!(out, "Created profile {id}");
            for warning in hotkey_warnings
                .into_iter()
                .flatten()
                .collect::<std::collections::HashSet<_>>()
            {
                errln!(out, "Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Update {
            id,
            name,
            hotkey,
            push_to_talk_shortcut,
            toggle_shortcut,
            clear_push_to_talk_shortcut,
            clear_toggle_shortcut,
            language,
            model,
        } => {
            if name.is_none()
                && hotkey.is_none()
                && push_to_talk_shortcut.is_none()
                && toggle_shortcut.is_none()
                && !clear_push_to_talk_shortcut
                && !clear_toggle_shortcut
                && language.is_none()
                && model.is_none()
            {
                return Err(DictationError::SettingsError(
                    "Specify at least one profile field or shortcut option".to_string(),
                ));
            }
            let hotkey_warnings = [
                hotkey.as_deref().and_then(shortcut_warning),
                push_to_talk_shortcut.as_deref().and_then(shortcut_warning),
                toggle_shortcut.as_deref().and_then(shortcut_warning),
            ];
            let language = language
                .as_deref()
                .map(|value| parse_value::<Language>(value, "language"))
                .transpose()?;
            let model = model
                .as_deref()
                .map(FileModelPreference::parse_id)
                .transpose()
                .map_err(DictationError::SettingsError)?;
            let mut profiles = machine::load().resolved_hotkey_profiles();
            let profile = profiles
                .iter_mut()
                .find(|profile| profile.id == id)
                .ok_or_else(|| DictationError::SettingsError(format!("Unknown profile '{id}'")))?;
            if let Some(name) = name {
                profile.name = name;
            }
            if let Some(hotkey) = hotkey {
                profile.set_primary_shortcut(hotkey);
            }
            if clear_push_to_talk_shortcut {
                profile.push_to_talk_shortcut = None;
            }
            if clear_toggle_shortcut {
                profile.toggle_shortcut = None;
            }
            if let Some(shortcut) = push_to_talk_shortcut {
                profile.push_to_talk_shortcut = Some(shortcut);
            }
            if let Some(shortcut) = toggle_shortcut {
                profile.toggle_shortcut = Some(shortcut);
            }
            if let Some(language) = language {
                profile.language = language;
            }
            print_notices(
                &persist_profiles(profiles, model.map(|choice| (id.as_str(), choice)))?,
                out,
            );
            errln!(out, "Updated profile {id}");
            for warning in hotkey_warnings
                .into_iter()
                .flatten()
                .collect::<std::collections::HashSet<_>>()
            {
                errln!(out, "Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Remove { id } => {
            let stored = machine::load();
            let dictionary_kept = stored
                .profile_glossaries
                .get(&id)
                .is_some_and(|source| !source.trim().is_empty());
            let mut profiles = stored.resolved_hotkey_profiles();
            let original_len = profiles.len();
            profiles.retain(|profile| profile.id != id);
            if profiles.len() == original_len {
                return Err(DictationError::SettingsError(format!(
                    "Unknown profile '{id}'"
                )));
            }
            persist_profiles(profiles, None)?;
            errln!(out, "Removed profile {id}");
            if dictionary_kept {
                errln!(
                    out,
                    "Its personal dictionary was kept. Inspect it with `sagascript glossary list --profile {id}` or remove it with `sagascript glossary clear --profile {id}`."
                );
            }
            Ok(())
        }
    }
}

/// Persist the profile list atomically; the change itself is
/// [`Settings::apply_profiles_change`], which the app's shortcut profiles use too.
fn persist_profiles(
    profiles: Vec<HotkeyProfile>,
    model: Option<(&str, FileModelPreference)>,
) -> Result<Vec<ModelResetNotice>, DictationError> {
    Settings::validate_hotkey_profiles(&profiles).map_err(DictationError::SettingsError)?;
    let mut notices = Vec::new();
    machine::try_update(|settings| {
        notices = settings.apply_profiles_change(profiles.clone(), model)?;
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;
    Ok(notices)
}

fn print_notices(notices: &[ModelResetNotice], out: &Out) {
    for notice in notices {
        errln!(out, "Notice: {}", notice.message);
    }
}

fn cmd_list(out: &Out) -> Result<(), DictationError> {
    let current = machine::load();
    let defaults = Settings::default();

    outln!(out, "{:<20} {:<24} DEFAULT", "KEY", "CURRENT")?;
    outln!(out, "{:<20} {:<24} -------", "---", "-------")?;
    for key in KEYS {
        outln!(
            out,
            "{:<20} {:<24} {}",
            key.name,
            (key.get)(&current),
            (key.get)(&defaults)
        )?;
    }
    Ok(())
}

fn cmd_get(key: &str, out: &Out) -> Result<(), DictationError> {
    let key = lookup(key)?;
    let settings = machine::load();
    let value = (key.get)(&settings);
    outln!(out, "{value}")?;
    Ok(())
}

fn cmd_set(name: &str, value: &str, out: &Out) -> Result<(), DictationError> {
    let key = lookup(name)?;
    // Parse before acquiring the settings lock so invalid input never writes.
    let mut validation_target = machine::load();
    (key.set)(&mut validation_target, value)?;
    let mut notices = Vec::new();
    let settings = machine::try_update(|settings| {
        notices = (key.set)(settings, value).map_err(|error| error.to_string())?;
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;

    errln!(out, "Set {name} = {}", (key.get)(&settings));
    print_notices(&notices, out);
    if let Some(warning) = (key.warning)(&settings) {
        errln!(out, "Warning: {warning}");
    }
    Ok(())
}

/// Warning for a shortcut: the brightness-key hint takes precedence because a
/// key that never reaches the app makes the Accessibility note moot.
fn shortcut_warning(shortcut: &str) -> Option<&'static str> {
    brightness_key_hotkey_warning(shortcut).or_else(|| bare_extended_hotkey_warning(shortcut))
}

/// macOS maps bare F14/F15 to the screen-brightness keys, so they usually never
/// reach the app. Mirrors the hint in src/lib/hotkey.ts.
fn brightness_key_hotkey_warning(shortcut: &str) -> Option<&'static str> {
    let is_brightness_key = matches!(shortcut.trim().to_ascii_uppercase().as_str(), "F14" | "F15");
    (cfg!(target_os = "macos") && is_brightness_key).then_some(
        "F14 and F15 control screen brightness on Macs and usually don't reach Sagascript. \
         F13 and F16-F19 work",
    )
}

fn bare_extended_hotkey_warning(shortcut: &str) -> Option<&'static str> {
    // Strict parity with src/lib/hotkey.ts (/^F(\d{1,2})$/i): at most two
    // ASCII digits, so "F013" never warns as a bare extended key.
    let normalized = shortcut.trim().to_ascii_lowercase();
    let digits = normalized.strip_prefix('f')?;
    let is_bare_extended = !digits.is_empty()
        && digits.len() <= 2
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits
            .parse::<u8>()
            .is_ok_and(|number| (13..=24).contains(&number));
    (cfg!(target_os = "macos") && is_bare_extended)
        .then_some("bare F13-F24 requires Accessibility approval for the installed Sagascript app")
}

fn cmd_reset(name: Option<&str>, out: &Out) -> Result<(), DictationError> {
    if let Some(name) = name {
        let key = lookup(name)?;
        let defaults = Settings::default();
        let mut notices = Vec::new();
        let settings = machine::try_update(|settings| {
            notices = (key.reset)(settings, &defaults)?;
            Ok(())
        })
        .map_err(DictationError::SettingsError)?;
        errln!(out, "Reset {name} to {}", (key.get)(&settings));
        print_notices(&notices, out);
    } else {
        machine::try_update(|current| {
            reset_all_settings(current)?;
            Ok(())
        })
        .map_err(DictationError::SettingsError)?;
        errln!(
            out,
            "All application settings reset to defaults; personal dictionaries preserved"
        );
    }
    Ok(())
}

fn reset_all_settings(current: &mut Settings) -> Result<(), String> {
    let defaults = Settings::default();
    let mut validation = current.clone();
    validation.replace_hotkey_profiles(vec![HotkeyProfile::legacy_default(
        defaults.hotkey.clone(),
        defaults.language,
    )])?;

    let initial_prompt = std::mem::take(&mut current.initial_prompt);
    let profile_glossaries = std::mem::take(&mut current.profile_glossaries);
    let profile_glossary_migrated = current.profile_glossary_migrated;
    let default_profile = HotkeyProfile::legacy_default(defaults.hotkey.clone(), defaults.language);
    let mut profile_models = std::collections::BTreeMap::new();
    profile_models.insert(default_profile.id.clone(), FileModelPreference::Auto);
    *current = Settings {
        initial_prompt,
        profile_glossaries,
        profile_glossary_migrated,
        hotkey_profiles: vec![default_profile],
        profile_models,
        ..defaults
    };
    Ok(())
}

fn cmd_path(out: &Out) -> Result<(), DictationError> {
    outln!(out, "{}", settings::store::settings_path().display())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::keys::{parse_bool, VALID_KEYS};
    use super::*;
    use sagascript_core::settings::{validate_hotkey, WhisperModel};

    // The tests name a key as `config` is given it; these run the key's entry
    // in the table (`keys::KEYS`).

    fn apply_setting_value_with_notices(
        settings: &mut Settings,
        key: &str,
        value: &str,
    ) -> Result<Vec<ModelResetNotice>, DictationError> {
        (lookup(key)?.set)(settings, value)
    }

    fn apply_setting_value(
        settings: &mut Settings,
        key: &str,
        value: &str,
    ) -> Result<(), DictationError> {
        apply_setting_value_with_notices(settings, key, value).map(|_| ())
    }

    fn reset_setting_value(
        settings: &mut Settings,
        key: &str,
        defaults: &Settings,
    ) -> Result<(), String> {
        let key = lookup(key).map_err(|error| error.to_string())?;
        (key.reset)(settings, defaults).map(|_| ())
    }

    fn get_setting_value(settings: &Settings, key: &str) -> String {
        lookup(key).map_or_else(|_| "unknown".to_string(), |key| (key.get)(settings))
    }

    fn setting_warning(key: &str, settings: &Settings) -> Option<&'static str> {
        lookup(key).ok().and_then(|key| (key.warning)(settings))
    }

    fn validate_key(key: &str) -> Result<(), DictationError> {
        lookup(key).map(|_| ())
    }

    #[test]
    fn validate_hotkey_valid_shortcuts() {
        let valid = [
            "Control+Shift+Space",
            "Option+Space",
            "Alt+Space",
            "Command+A",
            "CmdOrCtrl+Space",
            "Ctrl+Shift+Alt+F1",
            "Super+Shift+KeyX",
            "Shift+Enter",
            "Control+Tab",
            "CommandOrControl+Z",
        ];
        for s in valid {
            assert!(validate_hotkey(s).is_ok(), "should be valid: {s}");
        }
    }

    #[test]
    fn validate_hotkey_case_insensitive() {
        assert!(validate_hotkey("control+shift+space").is_ok());
        assert!(validate_hotkey("CONTROL+SHIFT+SPACE").is_ok());
        assert!(validate_hotkey("Control+SHIFT+Space").is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_hotkey_rejects_macos_quit_shortcut_aliases() {
        for shortcut in [
            "Command+Q",
            "Cmd+KeyQ",
            "Super+Q",
            "CmdOrCtrl+Q",
            "Control+Super+Shift+Q",
        ] {
            let error = validate_hotkey(shortcut).unwrap_err();
            assert!(
                error.contains("reserved for Quit on macOS"),
                "unexpected error for {shortcut}: {error}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_hotkey_rejects_macos_cut_shortcut_before_settings_mutation() {
        let mut settings = Settings::default();
        let original = settings.hotkey.clone();

        let error = apply_setting_value(&mut settings, "hotkey", "Super+X").unwrap_err();

        assert!(error.to_string().contains("reserved for Cut on macOS"));
        assert_eq!(settings.hotkey, original);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reserved_hotkey_is_rejected_before_settings_mutation() {
        let mut settings = Settings::default();
        let original = settings.hotkey.clone();

        let error = apply_setting_value(&mut settings, "hotkey", "Super+Q").unwrap_err();

        assert!(error.to_string().contains("reserved for Quit on macOS"));
        assert_eq!(settings.hotkey, original);
    }

    #[test]
    fn validate_hotkey_rejects_bare_key() {
        let err = validate_hotkey("Space").unwrap_err();
        assert!(err.to_string().contains("modifier is required"));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn apply_setting_value_accepts_bare_extended_function_key() {
        let mut settings = Settings::default();
        apply_setting_value(&mut settings, "hotkey", "F13").unwrap();
        assert_eq!(settings.hotkey, "F13");
        assert_eq!(settings.resolved_hotkey_profiles()[0].shortcut, "F13");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apply_setting_value_accepts_bare_f24_on_macos() {
        let mut settings = Settings::default();
        apply_setting_value(&mut settings, "hotkey", "F24").unwrap();
        assert_eq!(settings.hotkey, "F24");
    }

    #[test]
    fn validate_hotkey_rejects_unknown_key() {
        let err = validate_hotkey("Control+FooBar").unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn validate_hotkey_rejects_modifier_as_key() {
        let err = validate_hotkey("Control+Shift").unwrap_err();
        assert!(err.to_string().contains("is a modifier"));
    }

    #[test]
    fn validate_hotkey_rejects_empty() {
        assert!(validate_hotkey("").is_err());
    }

    #[test]
    fn validate_hotkey_rejects_double_plus() {
        assert!(validate_hotkey("Control++Space").is_err());
    }

    #[test]
    fn validate_hotkey_rejects_unknown_modifier() {
        let err = validate_hotkey("Hyper+Space").unwrap_err();
        assert!(err.to_string().contains("unknown modifier"));
    }

    // -- validate_key --

    #[test]
    fn validate_key_accepts_all_valid_keys() {
        for key in VALID_KEYS {
            assert!(validate_key(key).is_ok(), "should accept: {key}");
        }
    }

    #[test]
    fn validate_key_rejects_unknown() {
        assert!(validate_key("nonexistent").is_err());
        assert!(validate_key("").is_err());
        assert!(validate_key("Language").is_err()); // case-sensitive
    }

    #[test]
    fn validate_key_error_lists_valid_keys() {
        let err = validate_key("bogus").unwrap_err();
        let msg = err.to_string();
        for key in VALID_KEYS {
            assert!(
                msg.contains(key),
                "error should list valid key '{key}': {msg}"
            );
        }
    }

    // -- VALID_KEYS exhaustiveness --

    #[test]
    fn valid_keys_matches_settings_fields() {
        // Ensure every VALID_KEY is handled in get_setting_value (not returning "unknown")
        let settings = Settings::default();
        for key in VALID_KEYS {
            let value = get_setting_value(&settings, key);
            assert_ne!(
                value, "unknown",
                "VALID_KEYS contains '{key}' but get_setting_value doesn't handle it"
            );
        }
    }

    #[test]
    fn valid_keys_count_matches_settings_struct() {
        // Internal fields that are serialized but not user-configurable via `config`.
        // These have dedicated CLI commands instead (e.g. `reset-onboarding`).
        const INTERNAL_FIELDS: &[&str] = &[
            "has_completed_onboarding",
            "hotkey_profiles",
            "profile_glossaries",
            "profile_models",
            "profile_glossary_migrated",
            "model_lineup_version",
            "engine_defaults_version",
            "schema_version",
        ];

        let settings = Settings::default();
        let json = serde_json::to_value(&settings).unwrap();
        let field_count = json.as_object().unwrap().len() - INTERNAL_FIELDS.len();
        assert_eq!(
            VALID_KEYS.len(),
            field_count,
            "VALID_KEYS has {} entries but Settings has {} user-facing fields — did you forget to add a new setting?",
            VALID_KEYS.len(),
            field_count
        );
    }

    // -- parse_enum_value --

    #[test]
    fn parse_enum_value_all_valid_languages() {
        let valid = ["en", "sv", "no", "fi", "auto"];
        for v in valid {
            let result = parse_value::<Language>(v, "language");
            assert!(result.is_ok(), "should parse language '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_language() {
        let result = parse_value::<Language>("de", "language");
        assert!(result.is_err());
    }

    #[test]
    fn cli_language_change_resets_incompatible_model_with_notice_and_keeps_others() {
        let profile = |id: &str, shortcut: &str, language| HotkeyProfile {
            id: id.to_string(),
            name: id.to_string(),
            shortcut: shortcut.to_string(),
            language,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        };
        let mut settings = Settings {
            hotkey_profiles: vec![
                profile("default", "Super+S", Language::Swedish),
                profile("other", "Super+O", Language::Swedish),
            ],
            ..Default::default()
        };
        settings
            .set_profile_model_gated("default", FileModelPreference::PianissimoOriginal, true)
            .unwrap();
        settings
            .set_profile_model_gated("other", FileModelPreference::PianissimoOriginal, true)
            .unwrap();

        // `config profiles update default --language en` path.
        let mut profiles = settings.resolved_hotkey_profiles();
        profiles[0].language = Language::English;
        let notices = settings.replace_hotkey_profiles(profiles).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0]
                .message
                .contains("Pianissimo only supports Swedish"),
            "{}",
            notices[0].message
        );
        assert!(
            notices[0]
                .message
                .contains("Recommended (Whisper Base (EN))"),
            "{}",
            notices[0].message
        );
        assert_eq!(
            settings.profile_models["default"],
            FileModelPreference::Auto
        );
        assert_eq!(
            settings.profile_models["other"],
            FileModelPreference::PianissimoOriginal
        );

        // `config set language` path: compatible explicit model is kept silently.
        settings
            .set_profile_model("default", FileModelPreference::Whisper(WhisperModel::Base))
            .unwrap();
        let notices = apply_setting_value_with_notices(&mut settings, "language", "no").unwrap();
        assert!(notices.is_empty());
        assert_eq!(
            settings.profile_models["default"],
            FileModelPreference::Whisper(WhisperModel::Base)
        );
    }

    fn pianissimo_sv_settings() -> Settings {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "default".into(),
                name: "Svenska".into(),
                shortcut: "Super+S".into(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            }],
            ..Default::default()
        };
        settings
            .set_profile_model_gated("default", FileModelPreference::PianissimoOriginal, true)
            .unwrap();
        settings
    }

    fn english_default(settings: &Settings) -> Vec<HotkeyProfile> {
        let mut profiles = settings.resolved_hotkey_profiles();
        profiles[0].language = Language::English;
        profiles
    }

    #[test]
    fn profiles_update_language_only_resets_model_with_notice() {
        let mut settings = pianissimo_sv_settings();
        let profiles = english_default(&settings);
        let notices = settings.apply_profiles_change(profiles, None).unwrap();
        assert_eq!(notices.len(), 1);
        assert!(
            notices[0]
                .message
                .contains("Recommended (Whisper Base (EN))"),
            "{}",
            notices[0].message
        );
        assert_eq!(
            settings.profile_models["default"],
            FileModelPreference::Auto
        );
    }

    #[test]
    fn profiles_update_language_with_compatible_explicit_model_has_no_notice() {
        let mut settings = pianissimo_sv_settings();
        let profiles = english_default(&settings);
        let choice = FileModelPreference::Whisper(WhisperModel::BaseEn);
        let notices = settings
            .apply_profiles_change(profiles, Some(("default", choice)))
            .unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(settings.profile_models["default"], choice);
    }

    #[test]
    fn profiles_update_language_with_incompatible_explicit_model_errors_without_change() {
        let mut settings = pianissimo_sv_settings();
        let before = settings.clone();
        let mut candidate = settings.clone();
        let profiles = english_default(&settings);
        let error = candidate
            .apply_profiles_change(
                profiles,
                Some(("default", FileModelPreference::PianissimoOriginal)),
            )
            .unwrap_err();
        assert!(error.contains("Pianissimo"), "{error}");
        // try_update only persists on Ok, so the stored settings stay `before`.
        assert_eq!(settings.profile_models, before.profile_models);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        settings = before;
        assert_eq!(settings.default_profile().language, Language::Swedish);
    }

    #[test]
    fn explicit_model_drops_only_its_own_profiles_notice_even_with_shared_name_prefix() {
        let named = |id: &str, name: &str, shortcut: &str| HotkeyProfile {
            id: id.into(),
            name: name.into(),
            shortcut: shortcut.into(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        };
        let mut settings = Settings {
            hotkey_profiles: vec![
                named("a", "Svenska", "Super+J"),
                named("b", "Svenska exakt", "Super+K"),
            ],
            ..Default::default()
        };
        settings
            .set_profile_model_gated("a", FileModelPreference::PianissimoOriginal, true)
            .unwrap();
        settings
            .set_profile_model_gated("b", FileModelPreference::PianissimoOriginal, true)
            .unwrap();
        let mut profiles = settings.resolved_hotkey_profiles();
        for profile in &mut profiles {
            profile.language = Language::English;
        }
        let choice = FileModelPreference::Whisper(WhisperModel::BaseEn);
        let notices = settings
            .apply_profiles_change(profiles, Some(("a", choice)))
            .unwrap();
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert_eq!(notices[0].profile_id, "b");
        assert!(notices[0].message.contains("'Svenska exakt'"));
        assert_eq!(settings.profile_models["a"], choice);
        assert_eq!(settings.profile_models["b"], FileModelPreference::Auto);
    }

    #[test]
    fn language_change_with_profile_dictionary_is_rejected_without_mutation() {
        let mut settings = Settings::default();
        settings.hotkey_profiles = vec![HotkeyProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            shortcut: settings.hotkey.clone(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }];
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = apply_setting_value(&mut settings, "language", "en").unwrap_err();

        assert!(error.to_string().contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn reset_all_without_dictionaries_uses_defaults_and_preserves_global_source() {
        let mut settings = Settings {
            language: Language::Swedish,
            initial_prompt: "Codex".to_string(),
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Option+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            }],
            ..Default::default()
        };

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.language, Settings::default().language);
        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(
            settings.profile_models["default"],
            FileModelPreference::Auto
        );
        assert_eq!(settings.initial_prompt, "Codex");
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn reset_all_preserves_same_language_active_default_dictionary() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::English,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(settings.language, Language::English);
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("merge = merch")
        );
    }

    #[test]
    fn reset_all_rejects_default_language_change_with_active_dictionary_atomically() {
        let mut settings = Settings {
            language: Language::Swedish,
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::Swedish,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = reset_all_settings(&mut settings).unwrap_err();

        assert!(error.contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn reset_all_rejects_implicit_swedish_default_dictionary_atomically() {
        let mut settings = Settings {
            language: Language::Swedish,
            hotkey_profiles: Vec::new(),
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".into(), "merge = merch".into());
        let before = settings.clone();
        let error = reset_all_settings(&mut settings).unwrap_err();
        assert!(error.contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
        assert_eq!(settings.initial_prompt, before.initial_prompt);
    }

    #[test]
    fn reset_all_keeps_removed_profile_dictionary_inactive() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::English,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("removed".to_string(), "merge = merch".to_string());

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(
            settings
                .profile_glossaries
                .get("removed")
                .map(String::as_str),
            Some("merge = merch")
        );
        assert_eq!(settings.effective_glossary_source(Some("removed")), "");
    }

    #[test]
    fn reset_all_rejects_orphan_default_dictionary_atomically() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Option+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            }],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = reset_all_settings(&mut settings).unwrap_err();

        assert!(error.contains("inactive personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn parse_enum_value_all_valid_models() {
        let valid = [
            "tiny.en",
            "tiny",
            "base.en",
            "base",
            "kb-whisper-tiny",
            "kb-whisper-base",
            "kb-whisper-small",
            "nb-whisper-tiny",
            "nb-whisper-base",
            "nb-whisper-small",
            "fi-whisper-tiny",
        ];
        for v in valid {
            let result = parse_value::<WhisperModel>(v, "whisper_model");
            assert!(result.is_ok(), "should parse model '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_model() {
        let result = parse_value::<WhisperModel>("large-v3", "whisper_model");
        assert!(result.is_err());
    }

    #[test]
    fn parse_enum_value_all_valid_hotkey_modes() {
        let valid = ["push", "toggle"];
        for v in valid {
            let result = apply_setting_value(&mut Settings::default(), "hotkey_mode", v);
            assert!(result.is_ok(), "should parse hotkey_mode '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_hotkey_mode() {
        let result = apply_setting_value(&mut Settings::default(), "hotkey_mode", "hold");
        assert!(result.is_err());
    }

    #[test]
    fn presenter_hotkey_mode_is_rejected_through_cli_helper() {
        let mut settings = Settings::default();
        let before_mode = settings.hotkey_mode;
        let before_hotkey = settings.hotkey.clone();
        let error = apply_setting_value(&mut settings, "hotkey_mode", "presenter").unwrap_err();
        assert!(error.to_string().contains("Supported values: push, toggle"));
        assert_eq!(settings.hotkey_mode, before_mode);
        assert_eq!(settings.hotkey, before_hotkey);
    }

    // -- parse_bool --

    #[test]
    fn parse_bool_valid() {
        assert!(parse_bool("true", "test").unwrap());
        assert!(!parse_bool("false", "test").unwrap());
    }

    #[test]
    fn parse_bool_rejects_invalid() {
        assert!(parse_bool("yes", "test").is_err());
        assert!(parse_bool("no", "test").is_err());
        assert!(parse_bool("1", "test").is_err());
        assert!(parse_bool("0", "test").is_err());
        assert!(parse_bool("True", "test").is_err());
        assert!(parse_bool("FALSE", "test").is_err());
        assert!(parse_bool("", "test").is_err());
    }

    #[test]
    fn parse_bool_error_message() {
        let err = parse_bool("yes", "auto_paste").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("yes"), "error should mention input: {msg}");
        assert!(
            msg.contains("auto_paste"),
            "error should mention key: {msg}"
        );
    }

    // -- get_setting_value / format helpers --

    #[test]
    fn file_model_setting_is_independent_of_live_model() {
        let mut settings = Settings::default();
        let live = settings.whisper_model;
        apply_setting_value(&mut settings, "file_transcription_model", "pianissimo-sv").unwrap();
        assert_eq!(settings.whisper_model, live);
        assert_eq!(
            get_setting_value(&settings, "file_transcription_model"),
            "pianissimo-sv"
        );
        assert!(apply_setting_value(&mut settings, "file_transcription_model", "unknown").is_err());
        assert_eq!(
            get_setting_value(&settings, "file_transcription_model"),
            "pianissimo-sv"
        );
    }

    // Selecting Pianissimo is refused unless the platform gate is open.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn pianissimo_dictation_can_be_set_and_reset_to_default() {
        let mut settings = Settings::default();
        assert!(!settings.pianissimo_dictation);
        assert!(apply_setting_value(&mut settings, "pianissimo_dictation", "true").is_err());
        settings
            .set_default_profile_language(Language::Swedish)
            .unwrap();

        apply_setting_value(&mut settings, "pianissimo_dictation", "true").unwrap();
        assert!(settings.pianissimo_dictation);
        assert_eq!(get_setting_value(&settings, "pianissimo_dictation"), "true");

        let defaults = Settings::default();
        reset_setting_value(&mut settings, "pianissimo_dictation", &defaults).unwrap();
        assert_eq!(settings.pianissimo_dictation, defaults.pianissimo_dictation);
        assert_eq!(
            get_setting_value(&settings, "pianissimo_dictation"),
            defaults.pianissimo_dictation.to_string()
        );
    }

    #[test]
    fn legacy_initial_prompt_alias_never_overwrites_default_profile_aliases() {
        let mut settings = Settings::default();
        settings
            .profile_glossaries
            .insert("default".into(), "merge = merch".into());
        apply_setting_value(&mut settings, "initial_prompt", "OpenRouter").unwrap();
        assert!(settings.profile_glossaries["default"].contains("merge = merch"));
        assert!(settings.profile_glossaries["default"].contains("OpenRouter"));
        apply_setting_value(&mut settings, "initial_prompt", "OpenRouter = open router").unwrap();
        assert!(settings.profile_glossaries["default"].contains("OpenRouter = open router"));
        let before = settings.profile_glossaries["default"].clone();
        assert!(
            reset_setting_value(&mut settings, "initial_prompt", &Settings::default()).is_err()
        );
        assert_eq!(settings.profile_glossaries["default"], before);
    }

    #[test]
    fn pianissimo_dictation_rejects_invalid_boolean_without_mutating() {
        let mut settings = Settings {
            pianissimo_dictation: true,
            ..Settings::default()
        };

        for value in ["yes", "1", "TRUE"] {
            let error =
                apply_setting_value(&mut settings, "pianissimo_dictation", value).unwrap_err();
            let message = error.to_string();
            assert!(message.contains(value));
            assert!(message.contains("pianissimo_dictation"));
            assert!(settings.pianissimo_dictation);
        }
    }

    #[test]
    fn get_setting_value_returns_serialized_values() {
        let settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "language"), "en");
        assert_eq!(get_setting_value(&settings, "hotkey_mode"), "push");
        assert_eq!(get_setting_value(&settings, "show_overlay"), "true");
        assert_eq!(get_setting_value(&settings, "auto_paste"), "true");
        assert_eq!(get_setting_value(&settings, "auto_select_model"), "true");
        assert_eq!(
            get_setting_value(&settings, "pianissimo_dictation"),
            "false"
        );
        assert_eq!(
            get_setting_value(&settings, "hotkey"),
            "Control+Shift+Space"
        );
        assert_eq!(get_setting_value(&settings, "initial_prompt"), "");
        assert_eq!(get_setting_value(&settings, "beam_size"), "0");
        assert_eq!(get_setting_value(&settings, "temperature_fallback"), "true");
        assert_eq!(get_setting_value(&settings, "vad_enabled"), "false");
    }

    #[test]
    fn engine_settings_can_be_set_reset_and_reject_bad_values() {
        let mut settings = Settings::default();
        assert_eq!(
            get_setting_value(&settings, "engine_prewarm"),
            "on_app_start"
        );
        assert_eq!(
            get_setting_value(&settings, "engine_idle_unload_minutes"),
            "60"
        );

        apply_setting_value(&mut settings, "engine_prewarm", "on_app_start").unwrap();
        apply_setting_value(&mut settings, "engine_idle_unload_minutes", "3").unwrap();
        assert_eq!(
            get_setting_value(&settings, "engine_prewarm"),
            "on_app_start"
        );
        assert_eq!(
            get_setting_value(&settings, "engine_idle_unload_minutes"),
            "3"
        );

        assert!(apply_setting_value(&mut settings, "engine_prewarm", "always").is_err());
        assert!(apply_setting_value(&mut settings, "engine_idle_unload_minutes", "-1").is_err());
        assert!(apply_setting_value(&mut settings, "engine_idle_unload_minutes", "ten").is_err());
        assert_eq!(
            get_setting_value(&settings, "engine_prewarm"),
            "on_app_start"
        );

        let defaults = Settings::default();
        reset_setting_value(&mut settings, "engine_prewarm", &defaults).unwrap();
        reset_setting_value(&mut settings, "engine_idle_unload_minutes", &defaults).unwrap();
        assert_eq!(
            get_setting_value(&settings, "engine_prewarm"),
            "on_app_start"
        );
        assert_eq!(
            get_setting_value(&settings, "engine_idle_unload_minutes"),
            "60"
        );
    }

    #[test]
    fn get_setting_value_unknown_key_returns_unknown() {
        let settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "nonexistent"), "unknown");
    }

    #[test]
    fn enabling_auto_paste_warns_about_gui_accessibility_requirement() {
        let settings = Settings::default();
        let warning = setting_warning("auto_paste", &settings).unwrap();
        assert!(warning.contains("Accessibility approval"));
        assert!(warning.contains("reset auto-paste to false"));

        let disabled = Settings {
            auto_paste: false,
            ..Default::default()
        };
        assert!(setting_warning("auto_paste", &disabled).is_none());
        assert!(setting_warning("show_overlay", &Settings::default()).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bare_extended_hotkey_warns_about_gui_accessibility_requirement() {
        let settings = Settings {
            hotkey: "F24".to_string(),
            ..Default::default()
        };
        let warning = setting_warning("hotkey", &settings).unwrap();
        assert!(warning.contains("F13-F24"));
        assert!(warning.contains("Accessibility approval"));

        assert!(bare_extended_hotkey_warning("Shift+F24").is_none());
        assert!(bare_extended_hotkey_warning("F013").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn brightness_key_hotkey_warns_only_for_bare_f14_and_f15() {
        assert!(brightness_key_hotkey_warning("F14")
            .unwrap()
            .contains("brightness"));
        assert!(brightness_key_hotkey_warning(" f15 ").is_some());
        assert!(brightness_key_hotkey_warning("F13").is_none());
        assert!(brightness_key_hotkey_warning("F16").is_none());
        assert!(brightness_key_hotkey_warning("Shift+F14").is_none());
        assert!(brightness_key_hotkey_warning("F140").is_none());
        assert!(shortcut_warning("F14").unwrap().contains("brightness"));
        assert!(shortcut_warning("F13").unwrap().contains("Accessibility"));
    }

    // -- the key table (#373) --

    #[test]
    fn the_key_table_has_unique_names_and_generates_valid_keys() {
        let names: Vec<&str> = KEYS.iter().map(|key| key.name).collect();
        assert_eq!(names, VALID_KEYS);
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "duplicate key in the table");
        for key in KEYS {
            assert!(!key.help.is_empty(), "{} has no help line", key.name);
        }
    }

    #[test]
    fn every_key_resets_to_the_value_of_the_defaults() {
        let defaults = Settings::default();
        for key in KEYS {
            let mut settings = Settings::default();
            // `initial_prompt` refuses to forget entries; the rest must reset.
            (key.reset)(&mut settings, &defaults)
                .unwrap_or_else(|error| panic!("{}: {error}", key.name));
            assert_eq!(
                (key.get)(&settings),
                (key.get)(&defaults),
                "{} after reset",
                key.name
            );
        }
    }

    #[test]
    fn help_lists_every_key_in_every_place_a_key_is_listed() {
        use clap::{Args, Command};

        let mut config = ConfigArgs::augment_args(Command::new("config"));
        for (name, with_values) in [("list", false), ("get", false), ("set", true)] {
            let help = config
                .find_subcommand_mut(name)
                .unwrap()
                .render_long_help()
                .to_string();
            for key in KEYS {
                assert!(
                    help.contains(key.name),
                    "config {name} --help lacks {}",
                    key.name
                );
                if with_values {
                    for line in key.help {
                        assert!(
                            help.contains(line),
                            "config set --help lacks the values of {}: {line}",
                            key.name
                        );
                    }
                }
            }
        }
        let set = config.find_subcommand_mut("set").unwrap();
        assert!(
            set.render_long_help()
                .to_string()
                .contains("[possible values: language, whisper_model, "),
            "the key argument lists the possible values"
        );
    }

    // -- one parser per type (#373) --

    #[test]
    fn config_and_the_flags_take_the_same_language_spellings() {
        let spellings = [
            "en",
            "sv",
            "no",
            "fi",
            "auto",
            "english",
            "swedish",
            "norwegian",
            "finnish",
            "EN",
            "English",
            "de",
            "",
            " en",
            "en ",
            "svenska",
            "\\u0073v",
            "\"sv\"",
        ];
        for text in spellings {
            let flag = crate::transcribe::parse_language(text);
            let config = apply_setting_value(&mut Settings::default(), "language", text);
            let profile = parse_value::<Language>(text, "language");
            assert_eq!(flag.is_ok(), config.is_ok(), "{text:?}");
            assert_eq!(flag.is_ok(), profile.is_ok(), "{text:?}");
            if let (Ok(language), Ok(())) = (flag, config) {
                assert_eq!(profile.unwrap(), language, "{text:?}");
            }
        }
    }

    #[test]
    fn config_and_the_flags_take_the_same_model_spellings() {
        for model in WhisperModel::ALL {
            let id = model.id();
            assert_eq!(crate::transcribe::parse_model(id).unwrap(), model, "{id}");
            assert_eq!(
                parse_value::<WhisperModel>(id, "whisper_model").unwrap(),
                model
            );
        }
        for text in [
            "",
            "Base",
            "BASE",
            " base",
            "large-v3",
            "pianissimo-sv",
            "auto",
            "ba\\u0073e",
        ] {
            assert!(crate::transcribe::parse_model(text).is_err(), "{text:?}");
            let error = apply_setting_value(&mut Settings::default(), "whisper_model", text)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&format!("Invalid value '{text}' for whisper_model")),
                "{text:?}: {error}"
            );
        }
    }

    #[test]
    fn the_spellings_each_surface_reports_keep_their_own_text() {
        assert_eq!(
            crate::transcribe::parse_language("de")
                .unwrap_err()
                .to_string(),
            "Settings error: Unknown language 'de'. Valid: en, sv, no, fi, auto"
        );
        assert_eq!(
            apply_setting_value(&mut Settings::default(), "language", "de")
                .unwrap_err()
                .to_string(),
            "Settings error: Invalid value 'de' for language. Run 'sagascript config get language' to see current value."
        );
        assert_eq!(
            crate::transcribe::parse_model("nope").unwrap_err().to_string(),
            "Settings error: Unknown model 'nope'. Run 'sagascript list-models' to see available models."
        );
    }

    // -- the same persisted state as the app (#373) --

    #[test]
    fn config_set_language_resets_the_legacy_model_fields_like_the_app() {
        let mut settings = Settings {
            whisper_model: WhisperModel::Small,
            auto_select_model: false,
            ..Settings::default()
        };
        apply_setting_value(&mut settings, "language", "sv").unwrap();
        assert_eq!(
            settings.whisper_model,
            WhisperModel::recommended(Language::Swedish)
        );
        assert!(settings.auto_select_model);

        let mut settings = Settings {
            whisper_model: WhisperModel::Small,
            auto_select_model: false,
            ..Settings::default()
        };
        let defaults = Settings::default();
        reset_setting_value(&mut settings, "language", &defaults).unwrap();
        assert_eq!(
            settings.whisper_model,
            WhisperModel::recommended(defaults.language)
        );
        assert!(settings.auto_select_model, "reset is a set of the default");
    }

    #[test]
    fn config_set_hotkey_edits_the_first_profile_when_none_is_called_default() {
        let mut settings = Settings::default();
        settings
            .replace_hotkey_profiles(vec![
                HotkeyProfile {
                    id: "english".into(),
                    name: "English".into(),
                    shortcut: "Super+E".into(),
                    language: Language::English,
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                },
                HotkeyProfile {
                    id: "swedish".into(),
                    name: "Swedish".into(),
                    shortcut: "Super+S".into(),
                    language: Language::Swedish,
                    push_to_talk_shortcut: None,
                    toggle_shortcut: None,
                },
            ])
            .unwrap();
        apply_setting_value(&mut settings, "hotkey", "Control+Alt+D").unwrap();
        assert_eq!(settings.resolved_shortcuts(), ["Control+Alt+D", "Super+S"]);
        let error = apply_setting_value(&mut settings, "hotkey", "Super+S").unwrap_err();
        assert!(error.to_string().contains("Duplicate hotkey"), "{error}");
        assert_eq!(settings.resolved_shortcuts(), ["Control+Alt+D", "Super+S"]);
    }

    #[test]
    fn config_set_auto_paste_stores_the_request_and_warns() {
        let mut settings = Settings {
            auto_paste: false,
            ..Settings::default()
        };
        apply_setting_value(&mut settings, "auto_paste", "true").unwrap();
        assert!(
            settings.auto_paste,
            "the CLI cannot ask the app's permission"
        );
        assert!(setting_warning("auto_paste", &settings).is_some());
    }
}
