use clap::{Args, Subcommand};

use sagascript_core::error::DictationError;
use sagascript_core::settings::{
    self, validate_hotkey, HotkeyMode, HotkeyProfile, Language, Settings, WhisperModel,
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

Valid keys: language, whisper_model, hotkey_mode, show_overlay, \
auto_paste, auto_select_model, hotkey, initial_prompt, \
beam_size, temperature_fallback, vad_enabled")]
    List,

    /// Get a single setting value
    #[command(
        long_about = "\
Print the current value of a single setting to stdout.

Valid keys: language, whisper_model, hotkey_mode, show_overlay, \
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
  language             en, sv, no, auto (auto uses a generic model — less accurate)
  whisper_model        tiny.en, tiny, base.en, base, kb-whisper-tiny,
                       kb-whisper-base, kb-whisper-small, nb-whisper-tiny,
                       nb-whisper-base, nb-whisper-small
  hotkey_mode          push, toggle
  show_overlay         true, false
  auto_paste           true, false (enabling requires Accessibility approval for the installed GUI)
  auto_select_model    true, false
  hotkey               Modifier+Key (e.g. Control+Shift+Space, Option+Space)
  initial_prompt       Personal dictionary text; aliases use TERM = ALIAS | ALIAS
  beam_size            Integer >= 0 (0 = greedy/fast, 5 = beam search/accurate)
  temperature_fallback true, false
  vad_enabled          true, false",
        after_long_help = "\
EXAMPLES:
  sagascript config set language sv
  sagascript config set whisper_model kb-whisper-base
  sagascript config set hotkey 'Option+Space'
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
Reset a single setting or all settings to their default values.

If KEY is provided, only that setting is reset. \
If KEY is omitted, ALL settings are reset.",
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
Print the absolute path to the settings JSON file. Useful for manual \
editing or backup.")]
    Path,

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
        ConfigAction::Profiles { action } => cmd_profiles(action),
    }
}

fn cmd_profiles(action: ProfileAction) -> Result<(), DictationError> {
    match action {
        ProfileAction::List => {
            println!("{:<16} {:<20} {:<28} LANGUAGE", "ID", "NAME", "HOTKEY");
            for profile in settings::store::load().resolved_hotkey_profiles() {
                println!("{:<16} {:<20} {:<28} {}", profile.id, profile.name, profile.shortcut, format_language(profile.language));
            }
            Ok(())
        }
        ProfileAction::Create { id, name, hotkey, language } => {
            let language = parse_enum_value::<Language>(&language, "language")?;
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            if profiles.iter().any(|profile| profile.id == id) {
                return Err(DictationError::SettingsError(format!("Profile '{id}' already exists")));
            }
            profiles.push(HotkeyProfile { id: id.clone(), name, shortcut: hotkey, language });
            persist_profiles(profiles)?;
            eprintln!("Created profile {id}");
            Ok(())
        }
        ProfileAction::Update { id, name, hotkey, language } => {
            if name.is_none() && hotkey.is_none() && language.is_none() {
                return Err(DictationError::SettingsError("Specify at least one of --name, --hotkey, or --language".to_string()));
            }
            let language = language.as_deref().map(|value| parse_enum_value::<Language>(value, "language")).transpose()?;
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            let profile = profiles.iter_mut().find(|profile| profile.id == id).ok_or_else(|| DictationError::SettingsError(format!("Unknown profile '{id}'")))?;
            if let Some(name) = name { profile.name = name; }
            if let Some(hotkey) = hotkey { profile.shortcut = hotkey; }
            if let Some(language) = language { profile.language = language; }
            persist_profiles(profiles)?;
            eprintln!("Updated profile {id}");
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
                return Err(DictationError::SettingsError(format!("Unknown profile '{id}'")));
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
        "show_overlay",
        current.show_overlay,
        defaults.show_overlay
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_paste",
        current.auto_paste,
        defaults.auto_paste
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_select_model",
        current.auto_select_model,
        defaults.auto_select_model
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
    if key == "hotkey" {
        Settings::validate_hotkey_profiles(&validation_target.resolved_hotkey_profiles())
            .map_err(DictationError::SettingsError)?;
    }
    let settings = settings::store::try_update(|settings| {
        apply_setting_value(settings, key, value).map_err(|error| error.to_string())?;
        if key == "hotkey" {
            Settings::validate_hotkey_profiles(&settings.resolved_hotkey_profiles())?;
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
    (key == "auto_paste" && settings.auto_paste).then_some(
        "auto-paste requires Accessibility approval for the installed Sagascript app; \
         until it is granted, the GUI will keep or reset auto-paste to false",
    )
}

fn apply_setting_value(
    settings: &mut Settings,
    key: &str,
    value: &str,
) -> Result<(), DictationError> {
    match key {
        "language" => {
            settings.set_legacy_language(parse_enum_value::<Language>(value, "language")?);
        }
        "whisper_model" => {
            settings.whisper_model = parse_enum_value::<WhisperModel>(value, "whisper_model")?;
        }
        "hotkey_mode" => {
            settings.hotkey_mode = parse_enum_value::<HotkeyMode>(value, "hotkey_mode")?;
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
            settings.set_legacy_hotkey(value.to_string());
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
            let index = profiles.iter().position(|profile| profile.id == "default").unwrap_or(0);
            profiles[index].shortcut = defaults.hotkey;
            persist_profiles(profiles)?;
            eprintln!("Reset hotkey to {}", settings::store::load().hotkey);
            return Ok(());
        }
        let settings = settings::store::update(|settings| match key {
            "language" => settings.set_legacy_language(defaults.language),
            "whisper_model" => settings.whisper_model = defaults.whisper_model,
            "hotkey_mode" => settings.hotkey_mode = defaults.hotkey_mode,
            "show_overlay" => settings.show_overlay = defaults.show_overlay,
            "auto_paste" => settings.auto_paste = defaults.auto_paste,
            "auto_select_model" => settings.auto_select_model = defaults.auto_select_model,
            "hotkey" => unreachable!("hotkey reset handled transactionally above"),
            "initial_prompt" => settings.initial_prompt = defaults.initial_prompt,
            "beam_size" => settings.beam_size = defaults.beam_size,
            "temperature_fallback" => settings.temperature_fallback = defaults.temperature_fallback,
            "vad_enabled" => settings.vad_enabled = defaults.vad_enabled,
            _ => unreachable!(),
        })
        .map_err(DictationError::SettingsError)?;
        eprintln!("Reset {key} to {}", get_setting_value(&settings, key));
    } else {
        let defaults = Settings::default();
        settings::store::save(&defaults).map_err(DictationError::SettingsError)?;
        eprintln!("All settings reset to defaults");
    }
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
            assert!(msg.contains(key), "error should list valid key '{key}': {msg}");
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
        let valid = ["en", "sv", "no", "auto"];
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
    fn parse_enum_value_all_valid_models() {
        let valid = [
            "tiny.en", "tiny", "base.en", "base",
            "kb-whisper-tiny", "kb-whisper-base", "kb-whisper-small",
            "nb-whisper-tiny", "nb-whisper-base", "nb-whisper-small",
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
        let valid = ["push", "toggle"];
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
        assert!(msg.contains("auto_paste"), "error should mention key: {msg}");
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
        assert_eq!(get_setting_value(&settings, "hotkey"), "Control+Shift+Space");
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
}
