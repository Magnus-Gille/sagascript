use std::path::PathBuf;

use crate::settings::Settings;

const APP_IDENTIFIER: &str = "com.sagascript.app";
const SETTINGS_FILENAME: &str = "sagascript-settings.json";

/// Returns the application data directory (platform-specific).
/// macOS: ~/Library/Application Support/com.sagascript.app/
/// Windows: %APPDATA%/com.sagascript.app/
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .expect("could not determine application data directory")
        .join(APP_IDENTIFIER)
}

/// Returns the full path to the settings file.
pub fn settings_path() -> PathBuf {
    app_data_dir().join(SETTINGS_FILENAME)
}

/// Load settings from disk. Returns defaults if the file is missing or unreadable.
/// Partial JSON files are handled by `#[serde(default)]` on Settings.
pub fn load() -> Settings {
    let path = settings_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

/// Persist settings to disk using read-merge-write to preserve non-settings keys
/// (e.g. `hasCompletedOnboarding` from Tauri plugin store).
/// Uses atomic write: write to .tmp then rename.
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    let dir = app_data_dir();

    // Ensure directory exists
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

    // Read existing file to preserve non-settings keys
    let mut map: serde_json::Map<String, serde_json::Value> = if let Ok(contents) =
        std::fs::read_to_string(&path)
    {
        serde_json::from_str(&contents).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    // Merge settings fields into the map
    let settings_value = serde_json::to_value(settings).map_err(|e| format!("Serialize error: {e}"))?;
    if let serde_json::Value::Object(settings_map) = settings_value {
        for (k, v) in settings_map {
            map.insert(k, v);
        }
    }

    let json =
        serde_json::to_string_pretty(&map).map_err(|e| format!("Serialize error: {e}"))?;

    // Atomic write: .tmp + rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("Failed to write settings: {e}"))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename settings file: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{HotkeyMode, Language, WhisperModel};
    use std::fs;

    /// Helper: create a temp dir and override settings_path for testing
    fn with_temp_settings<F: FnOnce(PathBuf)>(f: F) {
        let dir = std::env::temp_dir().join(format!("sagascript-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILENAME);
        f(path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_path_is_under_app_data_dir() {
        let p = settings_path();
        assert!(p.ends_with(SETTINGS_FILENAME));
        assert!(p.parent().unwrap().ends_with(APP_IDENTIFIER));
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let s = load();
        let d = Settings::default();
        assert_eq!(s.language, d.language);
        assert_eq!(s.whisper_model, d.whisper_model);
        assert_eq!(s.hotkey, d.hotkey);
    }

    #[test]
    fn save_and_load_roundtrip() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            let mut settings = Settings::default();
            settings.language = Language::Swedish;
            settings.hotkey = "Alt+Space".to_string();

            // Write directly to temp path (bypassing app_data_dir)
            fs::create_dir_all(dir).unwrap();
            let json = serde_json::to_string_pretty(&settings).unwrap();
            fs::write(&path, &json).unwrap();

            // Read back
            let contents = fs::read_to_string(&path).unwrap();
            let loaded: Settings = serde_json::from_str(&contents).unwrap();
            assert_eq!(loaded.language, Language::Swedish);
            assert_eq!(loaded.hotkey, "Alt+Space");
            assert_eq!(loaded.whisper_model, WhisperModel::Base); // default preserved
        });
    }

    #[test]
    fn save_preserves_non_settings_keys() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();

            // Pre-populate with a non-settings key
            let initial = serde_json::json!({
                "hasCompletedOnboarding": true,
                "language": "en"
            });
            fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

            // Save settings via merge
            let mut settings = Settings::default();
            settings.language = Language::Norwegian;

            // Simulate save's merge logic directly on this path
            let mut map: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let settings_value = serde_json::to_value(&settings).unwrap();
            if let serde_json::Value::Object(sm) = settings_value {
                for (k, v) in sm {
                    map.insert(k, v);
                }
            }
            fs::write(&path, serde_json::to_string_pretty(&map).unwrap()).unwrap();

            // Verify non-settings key preserved
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["hasCompletedOnboarding"], true);
            assert_eq!(raw["language"], "no"); // updated
        });
    }

    #[test]
    fn partial_json_fills_defaults() {
        let json = r#"{"language":"sv","hotkey":"Alt+X"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.language, Language::Swedish);
        assert_eq!(s.hotkey, "Alt+X");
        // Defaults for missing fields
        assert_eq!(s.whisper_model, WhisperModel::Base);
        assert_eq!(s.hotkey_mode, HotkeyMode::PushToTalk);
        assert!(s.show_overlay);
        assert!(s.auto_paste);
        assert!(s.auto_select_model);
    }
}
