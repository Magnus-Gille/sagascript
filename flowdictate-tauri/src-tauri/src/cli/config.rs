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
            validate_hotkey(value)?;
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

/// Validate a hotkey string against the format accepted by Tauri's global-hotkey crate.
/// Format: [Modifier+]*Key (case-insensitive)
fn validate_hotkey(value: &str) -> Result<(), DictationError> {
    const MODIFIERS: &[&str] = &[
        "shift", "control", "ctrl", "alt", "option",
        "super", "command", "cmd",
        "commandorcontrol", "commandorctrl", "cmdorctrl", "cmdorcontrol",
    ];

    const KEYS: &[&str] = &[
        // Letters
        "keya", "keyb", "keyc", "keyd", "keye", "keyf", "keyg", "keyh", "keyi",
        "keyj", "keyk", "keyl", "keym", "keyn", "keyo", "keyp", "keyq", "keyr",
        "keys", "keyt", "keyu", "keyv", "keyw", "keyx", "keyy", "keyz",
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m",
        "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z",
        // Digits
        "digit0", "digit1", "digit2", "digit3", "digit4",
        "digit5", "digit6", "digit7", "digit8", "digit9",
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
        // Function keys
        "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10",
        "f11", "f12", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20",
        "f21", "f22", "f23", "f24",
        // Navigation
        "home", "end", "pageup", "pagedown",
        "arrowup", "arrowdown", "arrowleft", "arrowright",
        "up", "down", "left", "right",
        // Editing
        "backspace", "delete", "enter", "tab", "space",
        "escape", "esc",
        // Special characters
        "backquote", "`", "backslash", "\\",
        "bracketleft", "[", "bracketright", "]",
        "comma", ",", "equal", "=", "minus", "-",
        "period", ".", "quote", "'", "semicolon", ";", "slash", "/",
        // Lock & control
        "capslock", "numlock", "scrolllock",
        "pause", "pausebreak", "printscreen", "insert",
        // Numpad
        "numpad0", "numpad1", "numpad2", "numpad3", "numpad4",
        "numpad5", "numpad6", "numpad7", "numpad8", "numpad9",
        "num0", "num1", "num2", "num3", "num4",
        "num5", "num6", "num7", "num8", "num9",
        "numpadadd", "numadd", "numpadplus", "numplus",
        "numpadsubtract", "numsubtract",
        "numpadmultiply", "nummultiply",
        "numpaddivide", "numdivide",
        "numpaddecimal", "numdecimal",
        "numpadequal", "numequal",
        "numpadenter", "numenter",
        // Media
        "mediaplay", "mediapause", "mediaplaypause", "mediastop",
        "mediatracknext", "mediatrackprevious", "mediatrackprev",
        "audiovolumeup", "volumeup",
        "audiovolumedown", "volumedown",
        "audiovolumemute", "volumemute",
    ];

    let tokens: Vec<&str> = value.split('+').map(|t| t.trim()).collect();

    if tokens.is_empty() || tokens.iter().any(|t| t.is_empty()) {
        return Err(DictationError::SettingsError(
            "Invalid hotkey: empty or malformed. Example: 'Control+Shift+Space'".to_string(),
        ));
    }

    // Last token must be a key, preceding tokens must be modifiers
    let (mod_tokens, key_token) = tokens.split_at(tokens.len() - 1);
    let key = key_token[0].to_lowercase();

    if !KEYS.contains(&key.as_str()) {
        // Check if it's a modifier used as a key (common mistake)
        if MODIFIERS.contains(&key.as_str()) {
            return Err(DictationError::SettingsError(format!(
                "Invalid hotkey '{}': '{}' is a modifier, not a key. \
                 A hotkey must end with a key (e.g. Space, A, F1). \
                 Example: 'Control+Shift+Space'",
                value, key_token[0]
            )));
        }
        return Err(DictationError::SettingsError(format!(
            "Invalid hotkey '{}': unknown key '{}'. \
             Examples of valid keys: Space, A, F1, Enter, Tab, ArrowUp",
            value, key_token[0]
        )));
    }

    for &tok in mod_tokens {
        let lower = tok.to_lowercase();
        if !MODIFIERS.contains(&lower.as_str()) {
            if KEYS.contains(&lower.as_str()) {
                return Err(DictationError::SettingsError(format!(
                    "Invalid hotkey '{}': '{}' is a key, not a modifier. \
                     Modifiers must come before the key. \
                     Valid modifiers: Control, Shift, Alt/Option, Command/Super, CmdOrCtrl",
                    value, tok
                )));
            }
            return Err(DictationError::SettingsError(format!(
                "Invalid hotkey '{}': unknown modifier '{}'. \
                 Valid modifiers: Control, Shift, Alt/Option, Command/Super, CmdOrCtrl",
                value, tok
            )));
        }
    }

    if mod_tokens.is_empty() {
        return Err(DictationError::SettingsError(format!(
            "Invalid hotkey '{}': at least one modifier is required. \
             Example: 'Control+Space', 'Option+Space'",
            value
        )));
    }

    Ok(())
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
            "Super+KeyX",
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
}
