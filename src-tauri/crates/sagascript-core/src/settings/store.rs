use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use fs2::FileExt;

use crate::settings::Settings;

const APP_IDENTIFIER: &str = "ai.gille.sagascript";
const LEGACY_APP_IDENTIFIERS: &[&str] = &["com.sagascript.app"];
const SETTINGS_FILENAME: &str = "sagascript-settings.json";

/// Returns the application data directory (platform-specific).
/// macOS: ~/Library/Application Support/ai.gille.sagascript/
/// Windows: %APPDATA%/ai.gille.sagascript/
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_IDENTIFIER)
}

/// Returns the full path to the settings file.
pub fn settings_path() -> PathBuf {
    app_data_dir().join(SETTINGS_FILENAME)
}

fn legacy_settings_paths() -> impl Iterator<Item = PathBuf> {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    LEGACY_APP_IDENTIFIERS
        .iter()
        .map(move |identifier| base.join(identifier).join(SETTINGS_FILENAME))
}

fn copy_legacy_settings(source: &PathBuf, destination: &PathBuf) -> Result<bool, String> {
    if destination.exists() || !source.is_file() {
        return Ok(false);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "Settings destination has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create settings directory: {error}"))?;
    std::fs::copy(source, destination)
        .map_err(|error| format!("Failed to copy legacy settings: {error}"))?;
    Ok(true)
}

/// Copy settings from an earlier bundle identifier on first use of the new
/// identifier. Keep the source in place so rolling back a pre-launch build is
/// safe. Once the destination exists it always wins.
fn migrate_legacy_identifier_settings(destination: &PathBuf) {
    if destination.exists() {
        return;
    }

    for source in legacy_settings_paths() {
        if !source.is_file() {
            continue;
        }
        match copy_legacy_settings(&source, destination) {
            Ok(true) => tracing::info!(
                "Migrated settings from legacy application identifier ({})",
                source.display()
            ),
            Ok(false) => {}
            Err(error) => tracing::warn!(
                "Failed to migrate settings from {} to {}: {error}",
                source.display(),
                destination.display()
            ),
        }
        return;
    }
}

/// Load settings from disk. Returns defaults if the file is missing or unreadable.
/// Partial JSON files are handled by `#[serde(default)]` on Settings.
pub fn load() -> Settings {
    let path = settings_path();
    migrate_legacy_identifier_settings(&path);
    load_from(&path)
}

/// Load settings from a specific path. Returns defaults if missing or unreadable.
pub fn load_from(path: &Path) -> Settings {
    match std::fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(settings) => settings,
            Err(e) => {
                // One wrong-typed field would otherwise silently reset ALL
                // user settings to defaults with no diagnostic trail. We
                // still fall back to defaults (self-healing contract), but
                // now there's a log line to explain why.
                tracing::warn!(
                    "Failed to parse settings file at {}: {e} — falling back to defaults",
                    path.display()
                );
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// Persist settings to disk using read-merge-write to preserve unknown or
/// legacy keys while writing the canonical Settings fields.
/// Uses atomic write: write to .tmp then rename.
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    migrate_legacy_identifier_settings(&path);
    with_settings_lock(&path, || save_to(&path, settings))
}

/// Apply a field-level settings mutation to the latest on-disk snapshot and
/// return the persisted result.
///
/// GUI commands use this instead of saving their in-memory `Settings` clone,
/// which may be stale when the CLI changed another field moments earlier.
pub fn update<F>(mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    let path = settings_path();
    migrate_legacy_identifier_settings(&path);
    update_at(&path, mutate)
}

fn update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    with_settings_lock(path, || {
        let mut settings = load_from(path);
        mutate(&mut settings);
        save_to(path, &settings)?;
        Ok(settings)
    })
}

fn with_settings_lock<T, F>(path: &Path, operation: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String>,
{
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

    let lock_path = path.with_extension("json.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| format!("Failed to open settings lock: {e}"))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| format!("Failed to lock settings: {e}"))?;

    let result = operation();
    if let Err(e) = lock_file.unlock() {
        tracing::warn!(
            "Failed to unlock settings file {}: {e}",
            lock_path.display()
        );
    }
    result
}

/// Persist settings to a specific path. Test seam for `save`, which always
/// targets `settings_path()`.
fn save_to(path: &Path, settings: &Settings) -> Result<(), String> {
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Ensure directory exists
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

    // Read existing file to preserve non-settings keys
    let mut map: serde_json::Map<String, serde_json::Value> = if let Ok(contents) =
        std::fs::read_to_string(path)
    {
        match serde_json::from_str(&contents) {
            Ok(m) => m,
            Err(e) => {
                // A parse failure here used to silently drop every
                // preserved unknown or legacy key
                // this read-merge-write exists to protect. Back up the
                // corrupt bytes to a sidecar before starting fresh, rather
                // than aborting the save (aborting would contradict the
                // corrupt-to-defaults self-healing contract).
                tracing::warn!(
                    "Existing settings file at {} is corrupt ({e}) — backing up to .bak and starting fresh",
                    path.display()
                );
                let bak_path = path.with_extension("json.bak");
                if let Err(be) = std::fs::write(&bak_path, &contents) {
                    tracing::warn!(
                        "Failed to write corrupt settings backup to {}: {be}",
                        bak_path.display()
                    );
                }
                serde_json::Map::new()
            }
        }
    } else {
        serde_json::Map::new()
    };

    // Merge settings fields into the map
    let settings_value =
        serde_json::to_value(settings).map_err(|e| format!("Serialize error: {e}"))?;
    if let serde_json::Value::Object(settings_map) = settings_value {
        for (k, v) in settings_map {
            map.insert(k, v);
        }
    }

    let json = serde_json::to_string_pretty(&map).map_err(|e| format!("Serialize error: {e}"))?;

    // Atomic write: .tmp + rename
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json).map_err(|e| format!("Failed to write settings: {e}"))?;
    std::fs::rename(&tmp_path, path).map_err(|e| format!("Failed to rename settings file: {e}"))?;

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
    fn migration_copies_legacy_settings_without_overwriting_destination() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-identifier-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy = root.join("legacy").join(SETTINGS_FILENAME);
        let destination = root.join("current").join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();

        assert!(copy_legacy_settings(&legacy, &destination).unwrap());
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            r#"{"language":"sv"}"#
        );
        assert!(
            legacy.exists(),
            "migration must leave rollback source intact"
        );

        fs::write(&destination, r#"{"language":"no"}"#).unwrap();
        assert!(!copy_legacy_settings(&legacy, &destination).unwrap());
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            r#"{"language":"no"}"#
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        let nonexistent = std::env::temp_dir()
            .join(format!("sagascript-test-{}", uuid::Uuid::new_v4()))
            .join(SETTINGS_FILENAME);
        let s = load_from(&nonexistent);
        let d = Settings::default();
        assert_eq!(s.language, d.language);
        assert_eq!(s.whisper_model, d.whisper_model);
        assert_eq!(s.hotkey, d.hotkey);
    }

    #[test]
    fn save_and_load_roundtrip() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            let settings = Settings {
                language: Language::Swedish,
                hotkey: "Alt+Space".to_string(),
                ..Default::default()
            };

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
    fn field_update_preserves_external_changes_from_latest_disk_snapshot() {
        with_temp_settings(|path| {
            let external = Settings {
                hotkey: "Super+Q".to_string(),
                language: Language::English,
                ..Default::default()
            };
            save_to(&path, &external).unwrap();

            // Simulate a GUI language control whose in-memory snapshot still
            // contains the default hotkey. Only the selected field is passed
            // to the store mutation, so the CLI's newer hotkey must survive.
            let persisted = update_at(&path, |settings| {
                settings.language = Language::Swedish;
            })
            .unwrap();

            assert_eq!(persisted.language, Language::Swedish);
            assert_eq!(persisted.hotkey, "Super+Q");
            let reloaded = load_from(&path);
            assert_eq!(reloaded.hotkey, "Super+Q");
        });
    }

    #[test]
    fn update_uses_a_cross_process_lock_file() {
        with_temp_settings(|path| {
            update_at(&path, |settings| settings.auto_paste = false).unwrap();
            assert!(path.with_extension("json.lock").exists());
            assert!(!load_from(&path).auto_paste);
        });
    }

    #[test]
    fn save_preserves_non_settings_keys() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();

            // Pre-populate with the legacy camelCase onboarding key.
            let initial = serde_json::json!({
                "hasCompletedOnboarding": true,
                "language": "en"
            });
            fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

            // Save settings via merge
            let settings = Settings {
                language: Language::Norwegian,
                ..Default::default()
            };

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

    // -- load_from with corrupt/invalid JSON --

    #[test]
    fn load_from_corrupt_json_returns_defaults() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, "this is not json{{{").unwrap();

            let s = load_from(&path);
            let d = Settings::default();
            assert_eq!(s.language, d.language);
            assert_eq!(s.whisper_model, d.whisper_model);
            assert_eq!(s.hotkey, d.hotkey);
        });
    }

    #[test]
    fn load_from_empty_file_returns_defaults() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, "").unwrap();

            let s = load_from(&path);
            let d = Settings::default();
            assert_eq!(s.language, d.language);
        });
    }

    #[test]
    fn load_from_empty_object_returns_defaults() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, "{}").unwrap();

            let s = load_from(&path);
            let d = Settings::default();
            assert_eq!(s.language, d.language);
            assert_eq!(s.whisper_model, d.whisper_model);
            assert_eq!(s.hotkey_mode, d.hotkey_mode);
        });
    }

    #[test]
    fn load_from_unknown_fields_ignored() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            fs::write(&path, r#"{"language":"sv","unknown_field":42}"#).unwrap();

            let s = load_from(&path);
            assert_eq!(s.language, Language::Swedish);
            // Unknown field should not cause errors
            assert_eq!(s.whisper_model, WhisperModel::Base); // default
        });
    }

    #[test]
    fn load_from_invalid_enum_value_returns_defaults() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            // "de" is not a valid Language variant
            fs::write(&path, r#"{"language":"de"}"#).unwrap();

            let s = load_from(&path);
            let d = Settings::default();
            // Should fall back to full defaults since deserialization fails
            assert_eq!(s.language, d.language);
        });
    }

    // -- save_to backing up a corrupt existing file --

    #[test]
    fn save_backs_up_corrupt_existing_file() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();

            let corrupt = "this is not json{{{";
            fs::write(&path, corrupt).unwrap();

            let settings = Settings {
                language: Language::Swedish,
                ..Default::default()
            };
            let result = save_to(&path, &settings);
            assert!(
                result.is_ok(),
                "save_to should still succeed over a corrupt existing file: {result:?}"
            );

            // The corrupt bytes must be preserved in a .bak sidecar rather
            // than silently discarded.
            let bak_path = path.with_extension("json.bak");
            assert!(
                bak_path.exists(),
                "expected a .bak sidecar for the corrupt file"
            );
            let bak_contents = fs::read_to_string(&bak_path).unwrap();
            assert_eq!(bak_contents, corrupt);

            // And the save itself produced valid, fresh settings.
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["language"], "sv");
        });
    }

    #[test]
    fn save_to_no_existing_file_does_not_create_bak() {
        with_temp_settings(|path| {
            let settings = Settings::default();
            let result = save_to(&path, &settings);
            assert!(result.is_ok());

            let bak_path = path.with_extension("json.bak");
            assert!(
                !bak_path.exists(),
                "no corrupt file existed, so no .bak should be created"
            );
        });
    }
}
