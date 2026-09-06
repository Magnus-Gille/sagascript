use clap::{Args, Subcommand};

use sagascript_core::error::DictationError;
use sagascript_core::settings::{
    self, validate_hotkey, HotkeyMode, HotkeyProfile, Language, PresenterConfig,
    PresenterFinishAction, Settings, WhisperModel,
};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show all settings with current and default values
    #[command(long_about = "\
Show all settings in a table with their current values and defaults.

Valid keys: language, whisper_model, hotkey_mode (push, toggle, presenter), show_overlay, \
auto_paste, auto_select_model, hotkey, initial_prompt, \
beam_size, temperature_fallback, vad_enabled. Use `sagascript config presenter` \
for presenter finish/cancel/app actions.")]
    List,

    /// Get a single setting value
    #[command(
        long_about = "\
Print the current value of a single setting to stdout.

Valid keys: language, whisper_model, hotkey_mode (push, toggle, presenter), show_overlay, \
auto_paste, auto_select_model, hotkey, initial_prompt, \
beam_size, temperature_fallback, vad_enabled",
        after_long_help = "\
EXAMPLES:
  sagascript config get language
  sagascript config get hotkey
  sagascript config get initial_prompt"
    )]
    Get {
        /// Setting key [possible values: language, whisper_model, hotkey_mode, show_overlay, auto_paste, auto_select_model, hotkey, initial_prompt, beam_size, temperature_fallback, vad_enabled]
        key: String,
    },

    /// Set a setting value
    #[command(
        long_about = "\
Update a setting. The new value takes effect immediately — the GUI \
hot-reloads changes made via CLI.

Valid values per key:
  language             en, sv, no, fi, auto (auto uses a generic model — less accurate)
  whisper_model        tiny.en, tiny, base.en, base, kb-whisper-tiny,
                       kb-whisper-base, kb-whisper-small, nb-whisper-tiny,
                       nb-whisper-base, nb-whisper-small, fi-whisper-tiny
  hotkey_mode          push, toggle, presenter
  show_overlay         true, false
  auto_paste           true, false (enabling requires Accessibility approval for the installed GUI)
  auto_select_model    true, false
  hotkey               Modifier+Key; bare F13-F24 on macOS (Accessibility) or Windows
  initial_prompt       Personal dictionary text; aliases use TERM = ALIAS | ALIAS
  beam_size            Integer >= 0 (0 = greedy/fast, 5 = beam search/accurate)
  temperature_fallback true, false
  vad_enabled          true, false",
        after_long_help = "\
EXAMPLES:
  sagascript config set language sv
  sagascript config set whisper_model kb-whisper-base
  sagascript config set hotkey 'Option+Space'
  sagascript config set hotkey F13
  sagascript config set auto_paste false
  sagascript config set initial_prompt $'OpenRouter = open router | open vrouter\\nmerge = merch'"
    )]
    Set {
        /// Setting key [possible values: language, whisper_model, hotkey_mode, show_overlay, auto_paste, auto_select_model, hotkey, initial_prompt, beam_size, temperature_fallback, vad_enabled]
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
dictionaries are preserved. Clear the global dictionary with `sagascript glossary \
clear --yes`; repeat with `--profile ID` for each profile dictionary.",
        after_long_help = "\
EXAMPLES:
  # Reset just the language
  sagascript config reset language

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

    /// Configure the opt-in presenter hotkey mode
    #[command(
        long_about = "\
Configure presenter mode. Existing profile shortcuts start dictation; the \
finish shortcut ends it. App actions are opt-in and default to insert-only.",
        after_long_help = "\
EXAMPLES:
  sagascript config presenter show
  sagascript config presenter finish 'Control+Shift+Enter'
  sagascript config presenter cancel 'Control+Shift+Escape'
  sagascript config presenter cancel
  sagascript config presenter app com.example.editor command_return
  sagascript config presenter remove-app com.example.editor"
    )]
    Presenter {
        #[command(subcommand)]
        action: PresenterAction,
    },

    /// Manage per-shortcut dictation language profiles
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
    Create {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        hotkey: String,
        #[arg(long)]
        language: String,
    },
    /// Update a dictation profile
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(long)]
        language: Option<String>,
    },
    /// Remove a dictation profile (at least one must remain)
    Remove { id: String },
}

#[derive(Subcommand)]
pub enum PresenterAction {
    /// Print the presenter configuration as JSON
    Show,
    /// Set the global presenter finish shortcut
    Finish { shortcut: String },
    /// Set the presenter cancel shortcut, or omit it to disable cancel
    Cancel { shortcut: Option<String> },
    /// Set an explicit action for an application identifier
    App {
        app_id: String,
        #[arg(value_parser = ["insert_only", "return", "command_return"])]
        action: String,
    },
    /// Remove an application-specific presenter action
    RemoveApp { app_id: String },
}

const VALID_KEYS: &[&str] = &[
    "language",
    "whisper_model",
    "hotkey_mode",
    "show_overlay",
    "auto_paste",
    "auto_select_model",
    "hotkey",
    "initial_prompt",
    "beam_size",
    "temperature_fallback",
    "vad_enabled",
];

pub fn run(args: ConfigArgs) -> Result<(), DictationError> {
    match args.action {
        ConfigAction::List => cmd_list(),
        ConfigAction::Get { key } => cmd_get(&key),
        ConfigAction::Set { key, value } => cmd_set(&key, &value),
        ConfigAction::Reset { key } => cmd_reset(key.as_deref()),
        ConfigAction::Path => cmd_path(),
        ConfigAction::Presenter { action } => cmd_presenter(action),
        ConfigAction::Profiles { action } => cmd_profiles(action),
    }
}

fn cmd_presenter(action: PresenterAction) -> Result<(), DictationError> {
    if matches!(&action, PresenterAction::Show) {
        let presenter = settings::store::load().presenter;
        let json = serde_json::to_string_pretty(&presenter)
            .map_err(|error| DictationError::SettingsError(error.to_string()))?;
        println!("{json}");
        return Ok(());
    }

    let summary = match &action {
        PresenterAction::Finish { shortcut } => format!("Presenter finish shortcut = {shortcut}"),
        PresenterAction::Cancel {
            shortcut: Some(shortcut),
        } => {
            format!("Presenter cancel shortcut = {shortcut}")
        }
        PresenterAction::Cancel { shortcut: None } => {
            "Presenter cancel shortcut disabled".to_string()
        }
        PresenterAction::App { app_id, action } => {
            format!("Presenter action for {app_id} = {action}")
        }
        PresenterAction::RemoveApp { app_id } => format!("Removed presenter action for {app_id}"),
        PresenterAction::Show => unreachable!(),
    };
    update_presenter_config(|presenter| apply_presenter_action(presenter, action))?;
    eprintln!("{summary}");
    Ok(())
}

fn apply_presenter_action(
    presenter: &mut PresenterConfig,
    action: PresenterAction,
) -> Result<(), String> {
    match action {
        PresenterAction::Show => Err("Presenter show does not mutate settings".to_string()),
        PresenterAction::Finish { shortcut } => {
            validate_hotkey(&shortcut)?;
            presenter.finish_shortcut = shortcut;
            Ok(())
        }
        PresenterAction::Cancel { shortcut } => {
            if let Some(shortcut) = &shortcut {
                validate_hotkey(shortcut)?;
            }
            presenter.cancel_shortcut = shortcut;
            Ok(())
        }
        PresenterAction::App { app_id, action } => {
            let action = parse_enum_value::<PresenterFinishAction>(&action, "presenter action")
                .map_err(|error| error.to_string())?;
            presenter.app_actions.insert(app_id, action);
            Ok(())
        }
        PresenterAction::RemoveApp { app_id } => {
            if presenter.app_actions.remove(&app_id).is_none() {
                return Err(format!("No presenter action configured for app '{app_id}'"));
            }
            Ok(())
        }
    }
}

fn update_presenter_config<F>(mutate: F) -> Result<PresenterConfig, DictationError>
where
    F: FnOnce(&mut PresenterConfig) -> Result<(), String>,
{
    let updated = settings::store::try_update(|settings| {
        let mut presenter = settings.presenter.clone();
        mutate(&mut presenter)?;
        settings.replace_presenter_config(presenter)
    })
    .map_err(DictationError::SettingsError)?;
    Ok(updated.presenter)
}

fn cmd_profiles(action: ProfileAction) -> Result<(), DictationError> {
    match action {
        ProfileAction::List => {
            println!("{:<16} {:<20} {:<28} LANGUAGE", "ID", "NAME", "HOTKEY");
            for profile in settings::store::load().resolved_hotkey_profiles() {
                println!(
                    "{:<16} {:<20} {:<28} {}",
                    profile.id,
                    profile.name,
                    profile.shortcut,
                    format_language(profile.language)
                );
            }
            Ok(())
        }
        ProfileAction::Create {
            id,
            name,
            hotkey,
            language,
        } => {
            let language = parse_enum_value::<Language>(&language, "language")?;
            let hotkey_warning = bare_extended_hotkey_warning(&hotkey);
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            if profiles.iter().any(|profile| profile.id == id) {
                return Err(DictationError::SettingsError(format!(
                    "Profile '{id}' already exists"
                )));
            }
            profiles.push(HotkeyProfile {
                id: id.clone(),
                name,
                shortcut: hotkey,
                language,
            });
            persist_profiles(profiles)?;
            eprintln!("Created profile {id}");
            if let Some(warning) = hotkey_warning {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Update {
            id,
            name,
            hotkey,
            language,
        } => {
            if name.is_none() && hotkey.is_none() && language.is_none() {
                return Err(DictationError::SettingsError(
                    "Specify at least one of --name, --hotkey, or --language".to_string(),
                ));
            }
            let hotkey_warning = hotkey.as_deref().and_then(bare_extended_hotkey_warning);
            let language = language
                .as_deref()
                .map(|value| parse_enum_value::<Language>(value, "language"))
                .transpose()?;
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            let profile = profiles
                .iter_mut()
                .find(|profile| profile.id == id)
                .ok_or_else(|| DictationError::SettingsError(format!("Unknown profile '{id}'")))?;
            if let Some(name) = name {
                profile.name = name;
            }
            if let Some(hotkey) = hotkey {
                profile.shortcut = hotkey;
            }
            if let Some(language) = language {
                profile.language = language;
            }
            persist_profiles(profiles)?;
            eprintln!("Updated profile {id}");
            if let Some(warning) = hotkey_warning {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Remove { id } => {
            let stored = settings::store::load();
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
            persist_profiles(profiles)?;
            eprintln!("Removed profile {id}");
            if dictionary_kept {
                eprintln!(
                    "Its personal dictionary was kept. Inspect it with `sagascript glossary list --profile {id}` or remove it with `sagascript glossary clear --profile {id}`."
                );
            }
            Ok(())
        }
    }
}

fn persist_profiles(profiles: Vec<HotkeyProfile>) -> Result<(), DictationError> {
    Settings::validate_hotkey_profiles(&profiles).map_err(DictationError::SettingsError)?;
    settings::store::try_update(|settings| {
        settings.replace_hotkey_profiles(profiles)?;
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;
    Ok(())
}

fn cmd_list() -> Result<(), DictationError> {
    let current = settings::store::load();
    let defaults = Settings::default();

    println!("{:<20} {:<24} DEFAULT", "KEY", "CURRENT");
    println!("{:<20} {:<24} -------", "---", "-------");
    println!(
        "{:<20} {:<24} {}",
        "language",
        format_language(current.language),
        format_language(defaults.language)
    );
    println!(
        "{:<20} {:<24} {}",
        "whisper_model",
        format_model(current.whisper_model),
        format_model(defaults.whisper_model)
    );
    println!(
        "{:<20} {:<24} {}",
        "hotkey_mode",
        format_hotkey_mode(current.hotkey_mode),
        format_hotkey_mode(defaults.hotkey_mode)
    );
    println!(
        "{:<20} {:<24} {}",
        "show_overlay", current.show_overlay, defaults.show_overlay
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_paste", current.auto_paste, defaults.auto_paste
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_select_model", current.auto_select_model, defaults.auto_select_model
    );
    println!(
        "{:<20} {:<24} {}",
        "hotkey", current.hotkey, defaults.hotkey
    );
    println!(
        "{:<20} {:<24} {}",
        "initial_prompt", current.initial_prompt, defaults.initial_prompt
    );
    println!(
        "{:<20} {:<24} {}",
        "beam_size", current.beam_size, defaults.beam_size
    );
    println!(
        "{:<20} {:<24} {}",
        "temperature_fallback", current.temperature_fallback, defaults.temperature_fallback
    );
    println!(
        "{:<20} {:<24} {}",
        "vad_enabled", current.vad_enabled, defaults.vad_enabled
    );
    Ok(())
}

fn cmd_get(key: &str) -> Result<(), DictationError> {
    validate_key(key)?;
    let settings = settings::store::load();
    let value = get_setting_value(&settings, key);
    println!("{value}");
    Ok(())
}

fn cmd_set(key: &str, value: &str) -> Result<(), DictationError> {
    validate_key(key)?;
    // Parse before acquiring the settings lock so invalid input never writes.
    let mut validation_target = settings::store::load();
    apply_setting_value(&mut validation_target, key, value)?;
    if matches!(key, "hotkey" | "hotkey_mode") {
        validation_target
            .validate_shortcut_configuration()
            .map_err(DictationError::SettingsError)?;
    }
    let settings = settings::store::try_update(|settings| {
        apply_setting_value(settings, key, value).map_err(|error| error.to_string())?;
        if matches!(key, "hotkey" | "hotkey_mode") {
            settings.validate_shortcut_configuration()?;
        }
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;

    eprintln!("Set {key} = {}", get_setting_value(&settings, key));
    if let Some(warning) = setting_warning(key, &settings) {
        eprintln!("Warning: {warning}");
    }
    Ok(())
}

fn setting_warning(key: &str, settings: &Settings) -> Option<&'static str> {
    if key == "auto_paste" && settings.auto_paste {
        Some(
            "auto-paste requires Accessibility approval for the installed Sagascript app; \
             until it is granted, the GUI will keep or reset auto-paste to false",
        )
    } else if key == "hotkey" {
        bare_extended_hotkey_warning(&settings.hotkey)
    } else {
        None
    }
}

fn bare_extended_hotkey_warning(shortcut: &str) -> Option<&'static str> {
    // Strict parity with src/lib/hotkey.js (/^F(\d{1,2})$/i): at most two
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

fn apply_setting_value(
    settings: &mut Settings,
    key: &str,
    value: &str,
) -> Result<(), DictationError> {
    match key {
        "language" => {
            settings
                .set_legacy_language(parse_enum_value::<Language>(value, "language")?)
                .map_err(DictationError::SettingsError)?;
        }
        "whisper_model" => {
            settings.whisper_model = parse_enum_value::<WhisperModel>(value, "whisper_model")?;
        }
        "hotkey_mode" => {
            settings
                .replace_hotkey_mode(parse_enum_value::<HotkeyMode>(value, "hotkey_mode")?)
                .map_err(DictationError::SettingsError)?;
        }
        "show_overlay" => {
            settings.show_overlay = parse_bool(value, "show_overlay")?;
        }
        "auto_paste" => {
            settings.auto_paste = parse_bool(value, "auto_paste")?;
        }
        "auto_select_model" => {
            settings.auto_select_model = parse_bool(value, "auto_select_model")?;
        }
        "hotkey" => {
            validate_hotkey(value).map_err(DictationError::SettingsError)?;
            settings
                .try_set_legacy_hotkey(value.to_string())
                .map_err(DictationError::SettingsError)?;
        }
        "initial_prompt" => settings.initial_prompt = value.to_string(),
        "beam_size" => {
            settings.beam_size = value.parse::<u32>().map_err(|_| {
                DictationError::SettingsError(format!(
                    "beam_size must be a non-negative integer, got '{value}'"
                ))
            })?;
        }
        "temperature_fallback" => {
            settings.temperature_fallback = parse_bool(value, "temperature_fallback")?;
        }
        "vad_enabled" => {
            settings.vad_enabled = parse_bool(value, "vad_enabled")?;
        }
        _ => unreachable!(), // validate_key already checked
    }
    Ok(())
}

fn cmd_reset(key: Option<&str>) -> Result<(), DictationError> {
    if let Some(key) = key {
        validate_key(key)?;
        let defaults = Settings::default();
        if key == "hotkey" {
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            let index = profiles
                .iter()
                .position(|profile| profile.id == "default")
                .unwrap_or(0);
            profiles[index].shortcut = defaults.hotkey;
            persist_profiles(profiles)?;
            eprintln!("Reset hotkey to {}", settings::store::load().hotkey);
            return Ok(());
        }
        let settings = settings::store::try_update(|settings| match key {
            "language" => settings.set_legacy_language(defaults.language),
            "whisper_model" => {
                settings.whisper_model = defaults.whisper_model;
                Ok(())
            }
            "hotkey_mode" => settings.replace_hotkey_mode(defaults.hotkey_mode),
            "show_overlay" => {
                settings.show_overlay = defaults.show_overlay;
                Ok(())
            }
            "auto_paste" => {
                settings.auto_paste = defaults.auto_paste;
                Ok(())
            }
            "auto_select_model" => {
                settings.auto_select_model = defaults.auto_select_model;
                Ok(())
            }
            "hotkey" => unreachable!("hotkey reset handled transactionally above"),
            "initial_prompt" => {
                settings.initial_prompt = defaults.initial_prompt;
                Ok(())
            }
            "beam_size" => {
                settings.beam_size = defaults.beam_size;
                Ok(())
            }
            "temperature_fallback" => {
                settings.temperature_fallback = defaults.temperature_fallback;
                Ok(())
            }
            "vad_enabled" => {
                settings.vad_enabled = defaults.vad_enabled;
                Ok(())
            }
            _ => unreachable!(),
        })
        .map_err(DictationError::SettingsError)?;
        eprintln!("Reset {key} to {}", get_setting_value(&settings, key));
    } else {
        settings::store::try_update(|current| {
            reset_all_settings(current)?;
            Ok(())
        })
        .map_err(DictationError::SettingsError)?;
        eprintln!("All application settings reset to defaults; personal dictionaries preserved");
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
    *current = Settings {
        initial_prompt,
        profile_glossaries,
        ..defaults
    };
    Ok(())
}

fn cmd_path() -> Result<(), DictationError> {
    println!("{}", settings::store::settings_path().display());
    Ok(())
}

// -- Helpers --

fn validate_key(key: &str) -> Result<(), DictationError> {
    if VALID_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(DictationError::SettingsError(format!(
            "Unknown setting '{key}'. Valid keys: {}",
            VALID_KEYS.join(", ")
        )))
    }
}

fn get_setting_value(settings: &Settings, key: &str) -> String {
    match key {
        "language" => format_language(settings.language),
        "whisper_model" => format_model(settings.whisper_model),
        "hotkey_mode" => format_hotkey_mode(settings.hotkey_mode),
        "show_overlay" => settings.show_overlay.to_string(),
        "auto_paste" => settings.auto_paste.to_string(),
        "auto_select_model" => settings.auto_select_model.to_string(),
        "hotkey" => settings.hotkey.clone(),
        "initial_prompt" => settings.initial_prompt.clone(),
        "beam_size" => settings.beam_size.to_string(),
        "temperature_fallback" => settings.temperature_fallback.to_string(),
        "vad_enabled" => settings.vad_enabled.to_string(),
        _ => "unknown".to_string(),
    }
}

fn format_language(lang: Language) -> String {
    serde_json::to_value(lang)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", lang))
}

fn format_model(model: WhisperModel) -> String {
    serde_json::to_value(model)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", model))
}

fn format_hotkey_mode(mode: HotkeyMode) -> String {
    serde_json::to_value(mode)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", mode))
}

fn parse_enum_value<T: serde::de::DeserializeOwned>(
    value: &str,
    key: &str,
) -> Result<T, DictationError> {
    let quoted = format!("\"{}\"", value);
    serde_json::from_str::<T>(&quoted).map_err(|_| {
        DictationError::SettingsError(format!(
            "Invalid value '{value}' for {key}. Run 'sagascript config get {key}' to see current value."
        ))
    })
}

fn parse_bool(value: &str, key: &str) -> Result<bool, DictationError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(DictationError::SettingsError(format!(
            "Invalid value '{value}' for {key}. Must be 'true' or 'false'."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "presenter",
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
            let result = parse_enum_value::<Language>(v, "language");
            assert!(result.is_ok(), "should parse language '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_language() {
        let result = parse_enum_value::<Language>("de", "language");
        assert!(result.is_err());
    }

    #[test]
    fn language_change_with_profile_dictionary_is_rejected_without_mutation() {
        let mut settings = Settings::default();
        settings.hotkey_profiles = vec![HotkeyProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            shortcut: settings.hotkey.clone(),
            language: Language::Swedish,
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
            }],
            ..Default::default()
        };

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.language, Settings::default().language);
        assert!(settings.hotkey_profiles.is_empty());
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

        assert!(settings.hotkey_profiles.is_empty());
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

        assert!(settings.hotkey_profiles.is_empty());
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
            let result = parse_enum_value::<WhisperModel>(v, "whisper_model");
            assert!(result.is_ok(), "should parse model '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_model() {
        let result = parse_enum_value::<WhisperModel>("large-v3", "whisper_model");
        assert!(result.is_err());
    }

    #[test]
    fn parse_enum_value_all_valid_hotkey_modes() {
        let valid = ["push", "toggle", "presenter"];
        for v in valid {
            let result = parse_enum_value::<HotkeyMode>(v, "hotkey_mode");
            assert!(result.is_ok(), "should parse hotkey_mode '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_hotkey_mode() {
        let result = parse_enum_value::<HotkeyMode>("hold", "hotkey_mode");
        assert!(result.is_err());
    }

    #[test]
    fn presenter_action_values_are_strict_and_snake_case() {
        assert_eq!(
            parse_enum_value::<PresenterFinishAction>("insert_only", "presenter action").unwrap(),
            PresenterFinishAction::InsertOnly
        );
        assert_eq!(
            parse_enum_value::<PresenterFinishAction>("return", "presenter action").unwrap(),
            PresenterFinishAction::Return
        );
        assert_eq!(
            parse_enum_value::<PresenterFinishAction>("command_return", "presenter action")
                .unwrap(),
            PresenterFinishAction::CommandReturn
        );
        assert!(
            parse_enum_value::<PresenterFinishAction>("commandReturn", "presenter action").is_err()
        );
        assert!(parse_enum_value::<PresenterFinishAction>("submit", "presenter action").is_err());
    }

    #[test]
    fn presenter_hotkey_mode_mutation_is_atomic_through_cli_helper() {
        let mut settings = Settings::default();
        settings
            .replace_hotkey_profiles(vec![HotkeyProfile::legacy_default(
                "Control+Shift+Enter".to_string(),
                Language::English,
            )])
            .unwrap();
        let before = settings.hotkey_mode;
        let error = apply_setting_value(&mut settings, "hotkey_mode", "presenter").unwrap_err();
        assert!(error.to_string().contains("profile shortcut"));
        assert_eq!(settings.hotkey_mode, before);
    }

    #[test]
    fn presenter_legacy_hotkey_mutation_is_checked_and_atomic() {
        let mut settings = Settings::default();
        settings.replace_hotkey_mode(HotkeyMode::Presenter).unwrap();
        let before = settings.hotkey.clone();
        let error = apply_setting_value(&mut settings, "hotkey", "Ctrl+Shift+Enter").unwrap_err();
        assert!(error.to_string().contains("profile shortcut"));
        assert_eq!(settings.hotkey, before);
        assert_eq!(settings.resolved_hotkey_profiles()[0].shortcut, before);
    }

    #[test]
    fn presenter_command_help_inventory_has_all_mutations() {
        let command = <PresenterAction as clap::Subcommand>::augment_subcommands(
            clap::Command::new("presenter"),
        );
        let names: Vec<_> = command
            .get_subcommands()
            .map(|command| command.get_name())
            .collect();
        assert_eq!(names, ["show", "finish", "cancel", "app", "remove-app"]);
    }

    #[test]
    fn apply_presenter_action_rejects_invalid_input_without_partial_mutation() {
        let mut presenter = PresenterConfig::default();
        let before = presenter.clone();
        let error = apply_presenter_action(
            &mut presenter,
            PresenterAction::App {
                app_id: "com.example.editor".to_string(),
                action: "submit".to_string(),
            },
        )
        .unwrap_err();
        assert!(error.contains("Invalid value"));
        assert_eq!(presenter, before);

        let error = apply_presenter_action(
            &mut presenter,
            PresenterAction::Finish {
                shortcut: "NotAHotkey".to_string(),
            },
        )
        .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(presenter, before);
    }

    #[test]
    fn presenter_candidate_uses_fresh_state_and_core_validation_before_commit() {
        let mut settings = Settings::default();
        let mut candidate = settings.presenter.clone();
        for index in 0..PresenterConfig::MAX_APP_ACTIONS {
            candidate.app_actions.insert(
                format!("com.example.editor{index}"),
                PresenterFinishAction::InsertOnly,
            );
        }
        apply_presenter_action(
            &mut candidate,
            PresenterAction::App {
                app_id: "com.example.editor32".to_string(),
                action: "return".to_string(),
            },
        )
        .unwrap();
        assert!(settings.replace_presenter_config(candidate).is_err());
        assert!(settings.presenter.app_actions.is_empty());
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
    fn get_setting_value_returns_serialized_values() {
        let settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "language"), "en");
        assert_eq!(get_setting_value(&settings, "hotkey_mode"), "push");
        assert_eq!(get_setting_value(&settings, "show_overlay"), "true");
        assert_eq!(get_setting_value(&settings, "auto_paste"), "true");
        assert_eq!(get_setting_value(&settings, "auto_select_model"), "true");
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
}
