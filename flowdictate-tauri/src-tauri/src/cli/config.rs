use clap::{Args, Subcommand};

use crate::error::DictationError;
use crate::settings::{self, HotkeyMode, Language, Settings, WhisperModel};

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show all settings with current and default values
    List,
    /// Get a single setting value
    Get {
        /// Setting key (e.g. language, whisper_model, hotkey)
        key: String,
    },
    /// Set a setting value
    Set {
        /// Setting key
        key: String,
        /// New value
        value: String,
    },
    /// Reset one or all settings to defaults
    Reset {
        /// Setting key to reset (omit to reset all)
        key: Option<String>,
    },
    /// Print the settings file path
    Path,
}

const VALID_KEYS: &[&str] = &[
    "language",
    "whisper_model",
    "hotkey_mode",
    "show_overlay",
    "auto_paste",
    "auto_select_model",
    "hotkey",
];

pub fn run(args: ConfigArgs) -> Result<(), DictationError> {
    match args.action {
        ConfigAction::List => cmd_list(),
        ConfigAction::Get { key } => cmd_get(&key),
        ConfigAction::Set { key, value } => cmd_set(&key, &value),
        ConfigAction::Reset { key } => cmd_reset(key.as_deref()),
        ConfigAction::Path => cmd_path(),
    }
}

fn cmd_list() -> Result<(), DictationError> {
    let current = settings::store::load();
    let defaults = Settings::default();

    println!("{:<20} {:<24} {}", "KEY", "CURRENT", "DEFAULT");
    println!("{:<20} {:<24} {}", "---", "-------", "-------");
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
    let mut settings = settings::store::load();

    match key {
        "language" => {
            settings.language = parse_enum_value::<Language>(value, "language")?;
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
            settings.hotkey = value.to_string();
        }
        _ => unreachable!(), // validate_key already checked
    }

    settings::store::save(&settings).map_err(|e| DictationError::SettingsError(e))?;
    eprintln!("Set {key} = {}", get_setting_value(&settings, key));
    Ok(())
}

fn cmd_reset(key: Option<&str>) -> Result<(), DictationError> {
    if let Some(key) = key {
        validate_key(key)?;
        let mut settings = settings::store::load();
        let defaults = Settings::default();
        match key {
            "language" => settings.language = defaults.language,
            "whisper_model" => settings.whisper_model = defaults.whisper_model,
            "hotkey_mode" => settings.hotkey_mode = defaults.hotkey_mode,
            "show_overlay" => settings.show_overlay = defaults.show_overlay,
            "auto_paste" => settings.auto_paste = defaults.auto_paste,
            "auto_select_model" => settings.auto_select_model = defaults.auto_select_model,
            "hotkey" => settings.hotkey = defaults.hotkey,
            _ => unreachable!(),
        }
        settings::store::save(&settings).map_err(|e| DictationError::SettingsError(e))?;
        eprintln!("Reset {key} to {}", get_setting_value(&settings, key));
    } else {
        let defaults = Settings::default();
        settings::store::save(&defaults).map_err(|e| DictationError::SettingsError(e))?;
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
        _ => "unknown".to_string(),
    }
}

fn format_language(lang: Language) -> String {
    serde_json::to_value(&lang)
        .and_then(|v| serde_json::from_value::<String>(v))
        .unwrap_or_else(|_| format!("{:?}", lang))
}

fn format_model(model: WhisperModel) -> String {
    serde_json::to_value(&model)
        .and_then(|v| serde_json::from_value::<String>(v))
        .unwrap_or_else(|_| format!("{:?}", model))
}

fn format_hotkey_mode(mode: HotkeyMode) -> String {
    serde_json::to_value(&mode)
        .and_then(|v| serde_json::from_value::<String>(v))
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
