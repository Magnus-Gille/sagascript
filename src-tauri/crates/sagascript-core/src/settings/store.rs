use std::{
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use fs2::FileExt;

use crate::settings::{
    FileModelPreference, LineupEnv, Settings, WhisperModel, ENGINE_DEFAULTS_VERSION, MODEL_LINEUP_VERSION,
};

const APP_IDENTIFIER: &str = "ai.gille.sagascript";
const LEGACY_APP_IDENTIFIERS: &[&str] = &["com.sagascript.app"];
const CONFIG_DIR_NAME: &str = "sagascript";
const SETTINGS_FILENAME: &str = "sagascript-settings.json";
const GLOBAL_GLOSSARY_FILENAME: &str = "glossary.txt";
const PROFILE_GLOSSARY_DIRNAME: &str = "glossaries";
const XDG_CONFIG_HOME_ENV: &str = "XDG_CONFIG_HOME";
/// Optional exact settings-file location for isolated CLI sessions and tests.
///
/// When set, legacy settings migration is disabled so an isolated session can
/// never import or mutate the user's normal application settings by accident.
pub const SETTINGS_PATH_ENV: &str = "SAGASCRIPT_SETTINGS_PATH";
const LEGACY_ONBOARDING_KEY: &str = "hasCompletedOnboarding";
const ONBOARDING_KEY: &str = "has_completed_onboarding";

fn canonicalize_legacy_keys(map: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(legacy) = map.remove(LEGACY_ONBOARDING_KEY) {
        map.entry(ONBOARDING_KEY.to_string()).or_insert(legacy);
    }
}

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
    configured_settings_location().0
}

/// Returns the directory containing the user-managed configuration files.
pub fn config_dir() -> PathBuf {
    settings_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns the global personal-dictionary file.
pub fn global_glossary_path() -> PathBuf {
    config_dir().join(GLOBAL_GLOSSARY_FILENAME)
}

/// Returns the personal-dictionary file for a profile.
pub fn profile_glossary_path(profile_id: &str) -> Result<PathBuf, String> {
    validate_profile_file_id(profile_id)?;
    Ok(profile_glossary_dir().join(format!("{profile_id}.txt")))
}

/// Returns the directory containing profile-scoped personal dictionaries.
pub fn profile_glossary_dir() -> PathBuf {
    config_dir().join(PROFILE_GLOSSARY_DIRNAME)
}

/// Returns whether this process is using an explicit settings file.
pub fn settings_path_is_overridden() -> bool {
    settings_override_path(std::env::var_os(SETTINGS_PATH_ENV)).is_some()
}

fn legacy_settings_paths() -> impl Iterator<Item = PathBuf> {
    configured_settings_location().1.into_iter()
}

fn configured_settings_location() -> (PathBuf, Vec<PathBuf>) {
    settings_location(
        std::env::var_os(SETTINGS_PATH_ENV),
        std::env::var_os(XDG_CONFIG_HOME_ENV),
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")),
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")),
    )
}

fn settings_location(
    override_path: Option<OsString>,
    xdg_config_home: Option<OsString>,
    home_dir: PathBuf,
    data_dir: PathBuf,
) -> (PathBuf, Vec<PathBuf>) {
    if let Some(override_path) = settings_override_path(override_path) {
        return (override_path, Vec::new());
    }

    let config_base = absolute_nonempty_path(xdg_config_home)
        .unwrap_or_else(|| default_config_base(&home_dir, &data_dir));
    let settings_path = config_base.join(CONFIG_DIR_NAME).join(SETTINGS_FILENAME);
    let mut legacy_paths = vec![data_dir.join(APP_IDENTIFIER).join(SETTINGS_FILENAME)];
    legacy_paths.extend(
        LEGACY_APP_IDENTIFIERS
            .iter()
            .map(|identifier| data_dir.join(identifier).join(SETTINGS_FILENAME)),
    );
    legacy_paths.push(
        data_dir
            .join(APP_IDENTIFIER)
            .join("flowdictate-settings.json"),
    );
    (settings_path, legacy_paths)
}

fn settings_override_path(value: Option<OsString>) -> Option<PathBuf> {
    let path = value.filter(|value| !value.is_empty()).map(PathBuf::from)?;
    if path.is_absolute() {
        Some(path)
    } else {
        Some(
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path),
        )
    }
}

fn absolute_nonempty_path(value: Option<OsString>) -> Option<PathBuf> {
    value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[cfg(not(windows))]
fn default_config_base(home_dir: &Path, _data_dir: &Path) -> PathBuf {
    home_dir.join(".config")
}

#[cfg(windows)]
fn default_config_base(_home_dir: &Path, data_dir: &Path) -> PathBuf {
    data_dir.to_path_buf()
}

/// Caller must hold the destination's settings lock.
fn copy_legacy_settings(source: &Path, destination: &Path) -> Result<bool, String> {
    if destination.exists() || !source.is_file() {
        return Ok(false);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "Settings destination has no parent directory".to_string())?;
    create_private_dir_all(parent)
        .map_err(|error| format!("Failed to create settings directory: {error}"))?;
    let contents = std::fs::read_to_string(source)
        .map_err(|error| format!("Failed to read legacy settings: {error}"))?;
    let mut value: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse legacy settings: {error}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Legacy settings root is not an object".to_string())?;
    canonicalize_legacy_keys(object);

    // Moving the current bundle's settings into the XDG directory preserves
    // its TCC identity. Older bundle identifiers and FlowDictate do not.
    let same_bundle_settings = source.file_name().and_then(|name| name.to_str())
        == Some(SETTINGS_FILENAME)
        && source
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(APP_IDENTIFIER);
    if !same_bundle_settings {
        object.insert("auto_paste".to_string(), serde_json::Value::Bool(false));
    }
    let migrated = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("Failed to serialize migrated settings: {error}"))?;
    atomic_write(destination, migrated.as_bytes(), "migrated settings")?;
    Ok(true)
}

/// Copy settings from an earlier bundle identifier on first use of the new
/// identifier. Keep the source in place so rolling back a pre-launch build is
/// safe. Once the destination exists it always wins.
fn migrate_legacy_identifier_settings_locked(
    destination: &Path,
    sources: impl IntoIterator<Item = PathBuf>,
) {
    if destination.exists() {
        return;
    }

    for source in sources {
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
    let settings = load_at_with_legacy_sources(&path, legacy_settings_paths());
    let mut settings = settings;
    // Hot path: filesystem/OS probes only run while the one-time lineup
    // migration is still pending.
    if settings.model_lineup_version < MODEL_LINEUP_VERSION {
        let whisper_downloaded = |model: WhisperModel| crate::transcription::model::is_model_downloaded(model);
        settings = migrate_lineup_at(
            &path,
            settings,
            &LineupEnv {
                pianissimo_supported: crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
                pianissimo_downloaded: crate::transcription::pianissimo_model::is_downloaded(),
                whisper_downloaded: &whisper_downloaded,
            },
        );
    }
    if settings.engine_defaults_version < ENGINE_DEFAULTS_VERSION {
        settings = migrate_engine_defaults_at(&path, settings);
    }
    // A settings file copied from a supported machine must not select an engine
    // this system cannot run. In memory only: the file keeps the user's choice.
    settings.demote_unsupported_pianissimo(
        crate::transcription::pianissimo_backend::runtime_supported_on_this_os(),
    );
    settings
}

/// Run the one-time engine warm-up defaults migration and persist it under the
/// settings lock. Returns the settings to use.
fn migrate_engine_defaults_at(path: &Path, loaded: Settings) -> Settings {
    if loaded.engine_defaults_version >= ENGINE_DEFAULTS_VERSION {
        return loaded;
    }
    if !path.exists() {
        let mut fresh = loaded;
        fresh.engine_defaults_version = ENGINE_DEFAULTS_VERSION;
        return fresh;
    }
    with_settings_lock(path, || {
        let mut settings = load_from(path);
        if settings.migrate_engine_defaults() {
            if let Err(error) = save_to(path, &settings) {
                tracing::warn!("Failed to persist the engine defaults migration to {}: {error}", path.display());
            }
        }
        Ok(settings)
    })
    .unwrap_or(loaded)
}

/// Run the one-time Swedish model-lineup migration against the real machine
/// state and persist it under the settings lock. Returns the settings to use.
fn migrate_lineup_at(path: &Path, loaded: Settings, env: &LineupEnv<'_>) -> Settings {
    if loaded.model_lineup_version >= MODEL_LINEUP_VERSION {
        return loaded;
    }
    if !path.exists() {
        // Nothing persisted yet: there is nothing to migrate.
        let mut fresh = loaded;
        fresh.model_lineup_version = MODEL_LINEUP_VERSION;
        return fresh;
    }
    with_settings_lock(path, || {
        let mut settings = load_from(path);
        if settings.migrate_swedish_lineup(env) {
            if let Err(error) = save_to(path, &settings) {
                tracing::warn!("Failed to persist the Swedish model lineup migration to {}: {error}", path.display());
            }
        }
        Ok(settings)
    })
    .unwrap_or(loaded)
}

fn load_at_with_legacy_sources(
    path: &Path,
    legacy_sources: impl IntoIterator<Item = PathBuf>,
) -> Settings {
    // Existing settings are installed with atomic rename, so an ordinary read
    // needs no lock. Embedded dictionaries from older releases are the one
    // exception: migrate those under the writer lock before returning.
    if path.exists() && !has_embedded_glossary_fields(path) && !needs_profile_model_migration(path) {
        return load_from(path);
    }
    with_settings_lock(path, || {
        migrate_legacy_identifier_settings_locked(path, legacy_sources);
        let settings = load_from(path);
        if has_embedded_glossary_fields(path) || needs_profile_model_migration(path) {
            if let Err(error) = save_to(path, &settings) {
                tracing::warn!(
                    "Failed to migrate profile settings and dictionaries from {}: {error}",
                    path.display()
                );
            }
        }
        Ok(settings)
    })
    .unwrap_or_else(|error| {
        tracing::warn!(
            "Failed to lock settings file at {} for loading: {error} — falling back to defaults",
            path.display()
        );
        Settings::default()
    })
}

/// Load settings from a specific path. Returns defaults if missing or unreadable.
pub fn load_from(path: &Path) -> Settings {
    let mut settings = match std::fs::read_to_string(path) {
        Ok(contents) => {
            match serde_json::from_str::<serde_json::Value>(&contents).and_then(|mut value| {
                if let Some(map) = value.as_object_mut() {
                    canonicalize_legacy_keys(map);
                    if let Some(models) = map.get_mut("profile_models") {
                        if let Some(entries) = models.as_object_mut() {
                            entries.retain(|id, model| {
                                let valid = validate_profile_file_id(id).is_ok()
                                    && model.as_str().is_some_and(|id| FileModelPreference::parse_id(id).is_ok());
                                if !valid {
                                    tracing::warn!("Ignoring invalid profile model setting");
                                }
                                valid
                            });
                        } else {
                            tracing::warn!("Ignoring malformed profile model map");
                            map.remove("profile_models");
                        }
                    }
                }
                serde_json::from_value(value)
            }) {
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
            }
        }
        Err(_) => Settings::default(),
    };
    settings.profile_glossaries.retain(|profile_id, _| {
        if let Err(error) = validate_profile_file_id(profile_id) {
            tracing::warn!("Ignoring embedded {error}");
            false
        } else {
            true
        }
    });
    load_external_glossaries(path, &mut settings);
    settings.materialize_profile_models();
    settings.migrate_global_glossary_to_profiles();
    settings
}

/// Persist settings to disk using read-merge-write to preserve unknown or
/// legacy keys while writing the canonical Settings fields.
/// Uses atomic write: write a private temp file, fsync, then rename.
pub fn save(settings: &Settings) -> Result<(), String> {
    let path = settings_path();
    with_settings_lock(&path, || {
        migrate_legacy_identifier_settings_locked(&path, legacy_settings_paths());
        save_to(&path, settings)
    })
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
    update_at_with_legacy_sources(&path, legacy_settings_paths(), mutate)
}

/// Like [`update`], but lets validation fail while the cross-process lock is
/// held so read/validate/write profile mutations cannot race another writer.
pub fn try_update<F>(mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    let path = settings_path();
    try_update_at_with_legacy_sources(&path, legacy_settings_paths(), mutate)
}

#[cfg(test)]
fn try_update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    try_update_at_with_legacy_sources(path, std::iter::empty(), mutate)
}

fn try_update_at_with_legacy_sources<F>(
    path: &Path,
    sources: impl IntoIterator<Item = PathBuf>,
    mutate: F,
) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    with_settings_lock(path, || {
        migrate_legacy_identifier_settings_locked(path, sources);
        let mut settings = load_from(path);
        mutate(&mut settings)?;
        save_to(path, &settings)?;
        Ok(settings)
    })
}

#[cfg(test)]
fn update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    update_at_with_legacy_sources(path, std::iter::empty(), mutate)
}

fn update_at_with_legacy_sources<F>(
    path: &Path,
    sources: impl IntoIterator<Item = PathBuf>,
    mutate: F,
) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    with_settings_lock(path, || {
        migrate_legacy_identifier_settings_locked(path, sources);
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
    create_private_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

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
    create_private_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

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
                if let Err(be) = atomic_write(&bak_path, contents.as_bytes(), "corrupt settings backup") {
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
    canonicalize_legacy_keys(&mut map);

    save_external_glossaries(path, settings)?;

    // The personal dictionaries have human-editable files of their own.
    // Remove the legacy embedded fields after their external writes succeed.
    map.remove("initial_prompt");
    map.remove("profile_glossaries");

    // Merge settings fields into the map
    let settings_value =
        serde_json::to_value(settings).map_err(|e| format!("Serialize error: {e}"))?;
    if let serde_json::Value::Object(mut settings_map) = settings_value {
        settings_map.remove("initial_prompt");
        settings_map.remove("profile_glossaries");
        for (k, v) in settings_map {
            map.insert(k, v);
        }
    }

    let json = serde_json::to_string_pretty(&map).map_err(|e| format!("Serialize error: {e}"))?;

    atomic_write(path, json.as_bytes(), "settings")?;

    Ok(())
}

fn has_embedded_glossary_fields(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|map| {
            map.contains_key("initial_prompt") || map.contains_key("profile_glossaries")
        })
}

fn needs_profile_model_migration(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok())
        .and_then(|value| value.as_object().cloned())
        .is_some_and(|map| {
            if map.get("profile_glossary_migrated") != Some(&serde_json::Value::Bool(true)) {
                return true;
            }
            let Some(profiles) = map.get("hotkey_profiles").and_then(serde_json::Value::as_array) else {
                return true;
            };
            let Some(models) = map.get("profile_models").and_then(serde_json::Value::as_object) else {
                return true;
            };
            profiles.is_empty() || profiles.iter().any(|profile| {
                profile.get("id").and_then(serde_json::Value::as_str)
                    .is_some_and(|id| models.get(id).and_then(serde_json::Value::as_str)
                        .is_none_or(|model| FileModelPreference::parse_id(model).is_err()))
            })
        })
}

fn config_dir_for_settings(path: &Path) -> PathBuf {
    path.parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn global_glossary_path_for_settings(path: &Path) -> PathBuf {
    config_dir_for_settings(path).join(GLOBAL_GLOSSARY_FILENAME)
}

fn profile_glossary_dir_for_settings(path: &Path) -> PathBuf {
    config_dir_for_settings(path).join(PROFILE_GLOSSARY_DIRNAME)
}

fn validate_profile_file_id(profile_id: &str) -> Result<(), String> {
    if profile_id.is_empty()
        || profile_id.len() > 32
        || !profile_id.starts_with(|character: char| character.is_ascii_alphanumeric())
        || !profile_id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(format!(
            "Invalid profile id '{profile_id}' for personal dictionary file"
        ));
    }
    Ok(())
}

fn normalize_glossary_file(contents: String) -> String {
    contents.trim_end_matches(['\r', '\n']).to_string()
}

/// Compare editor baselines using the existing glossary file round-trip rule.
/// Only trailing CR/LF is normalized; spaces and internal line breaks matter.
pub fn glossary_sources_match(left: &str, right: &str) -> bool {
    left.trim_end_matches(['\r', '\n']) == right.trim_end_matches(['\r', '\n'])
}

fn glossary_file_contents(source: &str) -> String {
    let source = source.trim_end_matches(['\r', '\n']);
    if source.is_empty() {
        String::new()
    } else {
        format!("{source}\n")
    }
}

fn load_external_glossaries(path: &Path, settings: &mut Settings) {
    let global_path = global_glossary_path_for_settings(path);
    match std::fs::read_to_string(&global_path) {
        Ok(contents) => settings.initial_prompt = normalize_glossary_file(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(
            "Failed to read global personal dictionary at {}: {error}",
            global_path.display()
        ),
    }

    let profile_dir = profile_glossary_dir_for_settings(path);
    let entries = match std::fs::read_dir(&profile_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(
                "Failed to read profile dictionary directory at {}: {error}",
                profile_dir.display()
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("txt")
        {
            continue;
        }
        let Some(profile_id) = entry_path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Err(error) = validate_profile_file_id(profile_id) {
            tracing::warn!("Ignoring {error} ({})", entry_path.display());
            continue;
        }
        match std::fs::read_to_string(&entry_path) {
            Ok(contents) => {
                settings
                    .profile_glossaries
                    .insert(profile_id.to_string(), normalize_glossary_file(contents));
            }
            Err(error) => tracing::warn!(
                "Failed to read profile personal dictionary at {}: {error}",
                entry_path.display()
            ),
        }
    }
}

fn save_external_glossaries(path: &Path, settings: &Settings) -> Result<(), String> {
    for profile_id in settings.profile_glossaries.keys() {
        validate_profile_file_id(profile_id)?;
    }

    let global_path = global_glossary_path_for_settings(path);
    atomic_write_if_changed(
        &global_path,
        glossary_file_contents(&settings.initial_prompt).as_bytes(),
        "global personal dictionary",
    )?;

    let profile_dir = profile_glossary_dir_for_settings(path);
    for (profile_id, source) in &settings.profile_glossaries {
        let profile_path = profile_dir.join(format!("{profile_id}.txt"));
        atomic_write_if_changed(
            &profile_path,
            glossary_file_contents(source).as_bytes(),
            "profile personal dictionary",
        )?;
    }
    Ok(())
}

fn atomic_write_if_changed(path: &Path, contents: &[u8], label: &str) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    atomic_write(path, contents, label)
}

/// Atomically replace a regular file while preserving a user-managed symlink.
fn atomic_write(path: &Path, contents: &[u8], label: &str) -> Result<(), String> {
    let destination = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path)
                .map_err(|error| format!("Failed to resolve {label} symlink: {error}"))?;
            let target = if target.is_absolute() {
                target
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(target)
            };
            if target.exists() {
                std::fs::canonicalize(&target).map_err(|error| {
                    format!("Failed to canonicalize {label} symlink target: {error}")
                })?
            } else {
                target
            }
        }
        Ok(_) | Err(_) => path.to_path_buf(),
    };

    let parent = destination
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    create_private_dir_all(&parent)
        .map_err(|error| format!("Failed to create {label} directory: {error}"))?;
    install_replacement(&destination, &parent, contents, label, &copy_target_permissions)
}

/// Copy an existing regular target's permissions onto the replacement file.
fn copy_target_permissions(file: &std::fs::File, existing: &std::fs::Metadata) -> std::io::Result<()> {
    file.set_permissions(existing.permissions())
}

fn install_replacement(
    destination: &Path,
    parent: &Path,
    contents: &[u8],
    label: &str,
    copy_permissions: &dyn Fn(&std::fs::File, &std::fs::Metadata) -> std::io::Result<()>,
) -> Result<(), String> {
    use std::io::Write;

    // Exclusive, uniquely named, owner-only (0600 on Unix) temporary file in the
    // destination directory; removed automatically if anything below fails.
    let mut tmp = tempfile::Builder::new()
        .prefix(".sagascript-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("Failed to create {label} temporary file: {error}"))?;
    // Distinguish an absent target from an unreadable one; fail closed on the latter.
    let existing = match std::fs::metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!("Failed to inspect existing {label}: {error}"));
        }
    };
    tmp.write_all(contents)
        .map_err(|error| format!("Failed to write {label}: {error}"))?;
    // Keep an existing target's permissions (e.g. a deliberately restrictive or
    // shared mode); a new file stays owner-only. Applied before the final fsync.
    if let Some(existing) = existing.as_ref().filter(|metadata| metadata.is_file()) {
        copy_permissions(tmp.as_file(), existing)
            .map_err(|error| format!("Failed to preserve {label} permissions: {error}"))?;
    }
    tmp.as_file()
        .sync_all()
        .map_err(|error| format!("Failed to sync {label} before install: {error}"))?;
    install_temp_file(tmp, destination, existing.is_some(), label)?;
    sync_directory(parent)
        .map_err(|error| format!("Installed {label} but failed to sync its directory: {error}"))
}

/// Unix: rename(2) replaces atomically.
#[cfg(unix)]
fn install_temp_file(
    tmp: tempfile::NamedTempFile,
    destination: &Path,
    _target_exists: bool,
    label: &str,
) -> Result<(), String> {
    tmp.persist(destination)
        .map(|_| ())
        .map_err(|error| format!("Failed to install {label}: {}", error.error))
}

/// Windows: `ReplaceFileW` replaces atomically and merges the replaced file's
/// attributes and security descriptor (its ACL) into the replacement, so a
/// restricted target stays restricted. A new file inherits from its directory.
/// Durability is whatever NTFS gives the replace after the data was flushed;
/// there is no portable directory fsync.
#[cfg(windows)]
fn install_temp_file(
    tmp: tempfile::NamedTempFile,
    destination: &Path,
    target_exists: bool,
    label: &str,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !target_exists {
        return tmp
            .persist(destination)
            .map(|_| ())
            .map_err(|error| format!("Failed to install {label}: {}", error.error));
    }
    // Close the handle; the TempPath still removes the file on failure.
    let temp_path = tmp.into_temp_path();
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let replaced = wide(destination);
    let replacement = wide(&temp_path);
    // SAFETY: both buffers are NUL-terminated and outlive the call; the
    // remaining pointer arguments are documented as optional/reserved nulls.
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "Failed to install {label}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(path)
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{EnginePrewarm, FileModelPreference, HotkeyMode, Language, WhisperModel};
    use std::fs;
    use std::sync::mpsc;
    use std::thread;

    /// Helper: create a temp dir and override settings_path for testing
    fn with_temp_settings<F: FnOnce(PathBuf)>(f: F) {
        let dir = std::env::temp_dir().join(format!("sagascript-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILENAME);
        f(path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_global_model_selection_migrates_to_each_profile_without_losing_settings() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","pianissimo_dictation":true,"auto_select_model":false,"whisper_model":"kb-whisper-small","hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}]}"#).unwrap();
            let migrated = load_at_with_legacy_sources(&path, std::iter::empty());
            assert_eq!(migrated.profile_models["default"], FileModelPreference::PianissimoOriginal);
            assert_eq!(migrated.profile_models["english"], FileModelPreference::Auto);
            assert_eq!(migrated.hotkey_profiles[0].shortcut, "Super+S");
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["profile_models"]["default"], "pianissimo-sv");
            let reloaded = load_from(&path);
            assert_eq!(reloaded.profile_models, migrated.profile_models);
        });
    }

    #[test]
    fn engine_defaults_migration_persists_once_and_keeps_explicit_choices() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","engine_prewarm":"on_key_down","engine_idle_unload_minutes":10,"future_key":1}"#).unwrap();
            let migrated = migrate_engine_defaults_at(&path, load_from(&path));
            assert_eq!(migrated.engine_prewarm, EnginePrewarm::OnAppStart);
            assert_eq!(migrated.engine_idle_unload_minutes, 60);
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["engine_prewarm"], "on_app_start");
            assert_eq!(persisted["engine_idle_unload_minutes"], 60);
            assert_eq!(persisted["engine_defaults_version"], ENGINE_DEFAULTS_VERSION);
            assert_eq!(persisted["future_key"], 1);
            let before = fs::read_to_string(&path).unwrap();
            migrate_engine_defaults_at(&path, load_from(&path));
            assert_eq!(fs::read_to_string(&path).unwrap(), before);

            fs::write(&path, r#"{"engine_prewarm":"off","engine_idle_unload_minutes":10}"#).unwrap();
            let off = migrate_engine_defaults_at(&path, load_from(&path));
            assert_eq!(off.engine_prewarm, EnginePrewarm::Off);
            assert_eq!(off.engine_idle_unload_minutes, 60);
        });
    }

    #[test]
    fn swedish_lineup_migration_persists_once_and_is_idempotent_on_disk() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","auto_select_model":false,"whisper_model":"kb-whisper-small","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-small"}}"#).unwrap();
            let ready = |_: WhisperModel| true;
            let env = LineupEnv { pianissimo_supported: true, pianissimo_downloaded: true, whisper_downloaded: &ready };
            let migrated = migrate_lineup_at(&path, load_at_with_legacy_sources(&path, std::iter::empty()), &env);
            assert_eq!(migrated.profile_models["default"], FileModelPreference::PianissimoOriginal);
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["profile_models"]["default"], "pianissimo-sv");
            assert_eq!(persisted["model_lineup_version"], MODEL_LINEUP_VERSION);

            // A second load neither re-runs nor rewrites the migration.
            let before = fs::read_to_string(&path).unwrap();
            let again = migrate_lineup_at(&path, load_at_with_legacy_sources(&path, std::iter::empty()), &env);
            assert_eq!(again.profile_models, migrated.profile_models);
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        });
    }

    #[test]
    fn lineup_migration_preserves_unknown_keys_profiles_and_glossaries() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","future_key":{"a":[1,2]},"profile_glossary_migrated":true,"profile_glossaries":{"default":"Sagascript"},"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}],"profile_models":{"default":"kb-whisper-small","english":"auto"}}"#).unwrap();
            let ready = |_: WhisperModel| true;
            let env = LineupEnv { pianissimo_supported: true, pianissimo_downloaded: true, whisper_downloaded: &ready };
            let migrated = migrate_lineup_at(&path, load_at_with_legacy_sources(&path, std::iter::empty()), &env);
            assert_eq!(migrated.profile_models["default"], FileModelPreference::PianissimoOriginal);
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["future_key"], serde_json::json!({"a":[1,2]}));
            // Glossaries live in per-profile dictionary files, not settings.json.
            assert_eq!(load_from(&path).profile_glossaries["default"], "Sagascript");
            assert_eq!(persisted["hotkey_profiles"].as_array().unwrap().len(), 2);
            assert_eq!(persisted["hotkey_profiles"][1]["shortcut"], "Super+E");
            assert_eq!(persisted["profile_models"]["english"], "auto");
        });
    }

    #[test]
    fn lineup_migration_leaves_swedish_profile_without_model_entry_alone() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}]}"#).unwrap();
            let ready = |_: WhisperModel| true;
            let env = LineupEnv { pianissimo_supported: true, pianissimo_downloaded: true, whisper_downloaded: &ready };
            let mut loaded = load_at_with_legacy_sources(&path, std::iter::empty());
            loaded.profile_models.remove("default");
            loaded.model_lineup_version = 0;
            let migrated = migrate_lineup_at(&path, loaded, &env);
            assert!(!migrated.profile_models.contains_key("default") || migrated.profile_models["default"] == FileModelPreference::Auto);
            assert_eq!(migrated.model_lineup_version, MODEL_LINEUP_VERSION);
        });
    }

    #[test]
    fn swedish_lineup_migration_keeps_an_undownloaded_target_out_of_the_way() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-base"}}"#).unwrap();
            let only_base = |model: WhisperModel| model == WhisperModel::KbWhisperBase;
            let env = LineupEnv { pianissimo_supported: true, pianissimo_downloaded: false, whisper_downloaded: &only_base };
            let migrated = migrate_lineup_at(&path, load_at_with_legacy_sources(&path, std::iter::empty()), &env);
            assert_eq!(migrated.profile_models["default"], FileModelPreference::Whisper(WhisperModel::KbWhisperBase));
            // The marker is recorded even though nothing moved, so it never re-runs.
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["model_lineup_version"], MODEL_LINEUP_VERSION);
        });
    }

    #[test]
    fn global_glossary_migrates_once_to_each_profile_without_deleting_rollback_source() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","initial_prompt":"OpenRouter","hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}],"profile_glossaries":{"default":"merge = merch"}}"#).unwrap();
            let migrated = load_at_with_legacy_sources(&path, std::iter::empty());
            assert!(migrated.profile_glossary_migrated);
            assert!(migrated.profile_glossaries["default"].contains("OpenRouter"));
            assert!(migrated.profile_glossaries["english"].contains("OpenRouter"));
            assert!(migrated.profile_glossaries["default"].contains("merge = merch"));
            assert_eq!(fs::read_to_string(global_glossary_path_for_settings(&path)).unwrap(), "OpenRouter\n");
            let reloaded = load_at_with_legacy_sources(&path, std::iter::empty());
            assert_eq!(reloaded.profile_glossaries, migrated.profile_glossaries);
            assert!(!reloaded.effective_glossary_source(Some("english")).contains("merge = merch"));
        });
    }

    #[test]
    fn invalid_profile_model_does_not_reset_language_hotkey_or_other_settings() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","profile_models":{"default":"not-a-model"}}"#).unwrap();
            let loaded = load_at_with_legacy_sources(&path, std::iter::empty());
            assert_eq!(loaded.default_profile().language, Language::Swedish);
            assert_eq!(loaded.default_profile().shortcut, "Super+S");
            assert_eq!(loaded.profile_models["default"], FileModelPreference::Auto);
            let persisted: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["profile_models"]["default"], "auto");
        });
    }

    #[test]
    fn default_settings_path_uses_the_platform_config_directory() {
        let root = std::env::temp_dir().join("sagascript-location-test");
        let data_dir = root.join("application-support");
        let home_dir = root.join("home");
        let expected_base = default_config_base(&home_dir, &data_dir);
        let (p, legacy) = settings_location(None, None, home_dir, data_dir.clone());
        assert_eq!(
            p,
            expected_base.join(CONFIG_DIR_NAME).join(SETTINGS_FILENAME)
        );
        assert!(legacy.contains(&data_dir.join(APP_IDENTIFIER).join(SETTINGS_FILENAME)));
    }

    #[test]
    fn explicit_settings_path_disables_legacy_migration_sources() {
        let root = std::env::temp_dir().join("sagascript-location-test");
        let isolated = root.join("isolated").join("settings.json");
        let (path, legacy) = settings_location(
            Some(isolated.clone().into_os_string()),
            None,
            root.join("home"),
            root.join("application-support"),
        );

        assert_eq!(path, isolated);
        assert!(legacy.is_empty());
    }

    #[test]
    fn empty_settings_path_override_uses_the_normal_location() {
        let root = std::env::temp_dir().join("sagascript-location-test");
        let data_dir = root.join("application-support");
        let home_dir = root.join("home");
        let expected_base = default_config_base(&home_dir, &data_dir);
        let (path, legacy) = settings_location(
            Some(OsString::new()),
            None,
            home_dir.clone(),
            data_dir.clone(),
        );

        assert_eq!(
            path,
            expected_base.join(CONFIG_DIR_NAME).join(SETTINGS_FILENAME)
        );
        assert_eq!(legacy.len(), LEGACY_APP_IDENTIFIERS.len() + 2);
    }

    #[test]
    fn absolute_xdg_config_home_overrides_the_default() {
        let root = std::env::temp_dir().join("sagascript-location-test");
        let xdg_config_home = root.join("dotfiles-config");
        let (path, _) = settings_location(
            None,
            Some(xdg_config_home.clone().into_os_string()),
            root.join("home"),
            root.join("application-support"),
        );
        assert_eq!(
            path,
            xdg_config_home
                .join(CONFIG_DIR_NAME)
                .join(SETTINGS_FILENAME)
        );
    }

    #[test]
    fn relative_exact_override_remains_isolated_while_relative_xdg_is_ignored() {
        let (path, legacy) = settings_location(
            Some(OsString::from("relative/settings.json")),
            Some(OsString::from("relative-config")),
            PathBuf::from("/home/example"),
            PathBuf::from("/example/application-support"),
        );
        assert_eq!(
            path,
            std::env::current_dir()
                .unwrap()
                .join("relative/settings.json")
        );
        assert!(legacy.is_empty());
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
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert_eq!(migrated["language"], "sv");
        assert_eq!(migrated["auto_paste"], false);
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
    fn migration_disables_previously_enabled_auto_paste() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-permission-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy = root.join("legacy").join(SETTINGS_FILENAME);
        let destination = root.join("current").join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            r#"{"language":"sv","auto_paste":true,"hasCompletedOnboarding":true,"future_key":{"x":1}}"#,
        )
        .unwrap();

        assert!(copy_legacy_settings(&legacy, &destination).unwrap());
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert_eq!(migrated["auto_paste"], false);
        assert_eq!(migrated["has_completed_onboarding"], true);
        assert!(migrated.get("hasCompletedOnboarding").is_none());
        assert_eq!(migrated["future_key"]["x"], 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn xdg_move_preserves_auto_paste_for_the_same_bundle_identity() {
        let root =
            std::env::temp_dir().join(format!("sagascript-xdg-migration-{}", uuid::Uuid::new_v4()));
        let legacy = root.join(APP_IDENTIFIER).join(SETTINGS_FILENAME);
        let destination = root.join("xdg").join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, r#"{"auto_paste":true}"#).unwrap();

        assert!(copy_legacy_settings(&legacy, &destination).unwrap());
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert_eq!(migrated["auto_paste"], true);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn first_xdg_load_copies_settings_and_externalizes_embedded_glossaries() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-first-xdg-load-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy = root
            .join("application-support")
            .join(APP_IDENTIFIER)
            .join(SETTINGS_FILENAME);
        let destination = root
            .join("xdg")
            .join(CONFIG_DIR_NAME)
            .join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            r#"{"language":"sv","auto_paste":true,"initial_prompt":"OpenRouter = open router","profile_glossaries":{"swedish":"merge = merch"}}"#,
        )
        .unwrap();

        let loaded = load_at_with_legacy_sources(&destination, [legacy.clone()]);

        assert_eq!(loaded.language, Language::Swedish);
        assert!(loaded.auto_paste);
        assert_eq!(loaded.initial_prompt, "OpenRouter = open router");
        assert_eq!(
            loaded.profile_glossaries.get("swedish").map(String::as_str),
            Some("merge = merch")
        );
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert!(raw.get("initial_prompt").is_none());
        assert!(raw.get("profile_glossaries").is_none());
        assert_eq!(
            fs::read_to_string(destination.parent().unwrap().join("glossary.txt")).unwrap(),
            "OpenRouter = open router\n"
        );
        assert_eq!(
            fs::read_to_string(
                destination
                    .parent()
                    .unwrap()
                    .join("glossaries")
                    .join("swedish.txt")
            )
            .unwrap(),
            "merge = merch\n"
        );
        assert!(legacy.exists(), "rollback source must remain untouched");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn embedded_invalid_profile_dictionary_id_is_skipped_during_externalization() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-invalid-embedded-glossary-{}",
            uuid::Uuid::new_v4()
        ));
        let destination = root.join(CONFIG_DIR_NAME).join(SETTINGS_FILENAME);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(
            &destination,
            r#"{"language":"sv","profile_glossaries":{"Bad/Id":"unsafe","swedish":"merge = merch"}}"#,
        )
        .unwrap();

        let loaded = load_at_with_legacy_sources(&destination, std::iter::empty());

        assert!(!loaded.profile_glossaries.contains_key("Bad/Id"));
        assert_eq!(
            loaded.profile_glossaries.get("swedish").map(String::as_str),
            Some("merge = merch")
        );
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
        assert!(raw.get("profile_glossaries").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn migration_writes_through_a_dangling_settings_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "sagascript-dangling-settings-link-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy = root.join("legacy").join(SETTINGS_FILENAME);
        let target = root.join("dotfiles").join(SETTINGS_FILENAME);
        let destination = root.join("config").join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();
        symlink(&target, &destination).unwrap();

        assert!(copy_legacy_settings(&legacy, &destination).unwrap());
        assert!(fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        let migrated: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(migrated["language"], "sv");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_first_run_update_waits_for_atomic_migration() {
        let root = std::env::temp_dir().join(format!(
            "sagascript-concurrent-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let legacy = root.join("legacy").join(SETTINGS_FILENAME);
        let destination = root.join("current").join(SETTINGS_FILENAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(
            &legacy,
            r#"{"language":"sv","auto_paste":true,"future_key":{"x":1}}"#,
        )
        .unwrap();

        let (migration_locked_tx, migration_locked_rx) = mpsc::channel();
        let (release_migration_tx, release_migration_rx) = mpsc::channel();
        let migration_destination = destination.clone();
        let migration_legacy = legacy.clone();
        let migration = thread::spawn(move || {
            with_settings_lock(&migration_destination, || {
                migrate_legacy_identifier_settings_locked(
                    &migration_destination,
                    [migration_legacy],
                );
                migration_locked_tx.send(()).unwrap();
                release_migration_rx.recv().unwrap();
                Ok(())
            })
        });

        // Hold the lock after the atomic migration has installed the file,
        // then start an update in a second thread. It cannot read or write the
        // destination until the migration's critical section is released.
        migration_locked_rx.recv().unwrap();
        let (update_blocked_tx, update_blocked_rx) = mpsc::channel();
        let update_destination = destination.clone();
        let update_legacy = legacy.clone();
        let update = thread::spawn(move || {
            let lock_probe = OpenOptions::new()
                .read(true)
                .write(true)
                .open(update_destination.with_extension("json.lock"))
                .unwrap();
            assert!(
                lock_probe.try_lock_exclusive().is_err(),
                "migration must still hold the settings lock"
            );
            update_blocked_tx.send(()).unwrap();
            update_at_with_legacy_sources(&update_destination, [update_legacy], |settings| {
                settings.language = Language::Norwegian;
            })
        });
        update_blocked_rx.recv().unwrap();
        release_migration_tx.send(()).unwrap();

        migration.join().unwrap().unwrap();
        let updated = update.join().unwrap().unwrap();
        assert_eq!(updated.language, Language::Norwegian);
        assert!(!updated.auto_paste);

        let contents = fs::read_to_string(&destination).unwrap();
        let persisted: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(persisted["language"], "no");
        assert_eq!(persisted["auto_paste"], false);
        assert_eq!(persisted["future_key"]["x"], 1);
        assert!(!destination.with_extension("json.tmp").exists());
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
    fn dictionary_compare_and_set_survives_external_file_round_trip() {
        with_temp_settings(|path| {
            let first = try_update_at(&path, |settings| {
                settings.initial_prompt = "OpenRouter\n\n".into();
                settings
                    .profile_glossaries
                    .insert("swedish".into(), "merge = merch\r\n".into());
                settings
                    .profile_glossaries
                    .insert("empty".into(), String::new());
                Ok(())
            })
            .unwrap();
            let second = try_update_at(&path, |settings| {
                if !glossary_sources_match(&settings.initial_prompt, &first.initial_prompt)
                    || !glossary_sources_match(
                        &settings.profile_glossaries["swedish"],
                        &first.profile_glossaries["swedish"],
                    )
                {
                    return Err("Spurious dictionary conflict after file round trip".into());
                }
                assert_eq!(
                    settings.profile_glossaries.get("empty").map(String::as_str),
                    Some("")
                );
                assert!(!settings.profile_glossaries.contains_key("absent"));
                settings.initial_prompt = "OpenRouter\nCodex\n".into();
                settings
                    .profile_glossaries
                    .insert("swedish".into(), "merge = merch\nCodex = code x\n".into());
                Ok(())
            })
            .unwrap();
            let loaded = load_from(&path);
            assert!(glossary_sources_match(
                &loaded.initial_prompt,
                &second.initial_prompt
            ));
            assert!(glossary_sources_match(
                &loaded.profile_glossaries["swedish"],
                &second.profile_glossaries["swedish"]
            ));
            assert_eq!(
                loaded.profile_glossaries.get("empty").map(String::as_str),
                Some("")
            );
            assert!(!loaded.profile_glossaries.contains_key("absent"));
            assert!(!glossary_sources_match("OpenRouter ", "OpenRouter"));
            assert!(!glossary_sources_match(
                "OpenRouter\nCodex",
                "OpenRouter Codex"
            ));
        });
    }

    #[test]
    fn save_externalizes_global_and_profile_glossaries() {
        with_temp_settings(|path| {
            let mut settings = Settings {
                initial_prompt: "OpenRouter = open router".to_string(),
                ..Default::default()
            };
            settings
                .profile_glossaries
                .insert("swedish".to_string(), "merge = merch".to_string());

            save_to(&path, &settings).unwrap();

            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert!(raw.get("initial_prompt").is_none());
            assert!(raw.get("profile_glossaries").is_none());
            assert_eq!(
                fs::read_to_string(path.parent().unwrap().join("glossary.txt")).unwrap(),
                "OpenRouter = open router\n"
            );
            assert_eq!(
                fs::read_to_string(
                    path.parent()
                        .unwrap()
                        .join("glossaries")
                        .join("swedish.txt")
                )
                .unwrap(),
                "merge = merch\n"
            );

            let loaded = load_from(&path);
            assert_eq!(loaded.initial_prompt, "OpenRouter = open router");
            assert_eq!(
                loaded.profile_glossaries.get("swedish").map(String::as_str),
                Some("merge = merch")
            );
        });
    }

    #[test]
    fn external_glossary_files_override_legacy_embedded_values() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir.join("glossaries")).unwrap();
            fs::write(
                &path,
                r#"{"initial_prompt":"legacy","profile_glossaries":{"swedish":"legacy scoped"}}"#,
            )
            .unwrap();
            fs::write(dir.join("glossary.txt"), "external global\n").unwrap();
            fs::write(
                dir.join("glossaries").join("swedish.txt"),
                "external scoped\n",
            )
            .unwrap();

            let loaded = load_from(&path);
            assert_eq!(loaded.initial_prompt, "external global");
            assert_eq!(
                loaded.profile_glossaries.get("swedish").map(String::as_str),
                Some("external scoped")
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn glossary_save_preserves_a_dotfiles_symlink() {
        use std::os::unix::fs::symlink;

        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            let dotfiles_dir = dir.join("dotfiles");
            fs::create_dir_all(&dotfiles_dir).unwrap();
            let target = dotfiles_dir.join("glossary.txt");
            fs::write(&target, "old\n").unwrap();
            let link = dir.join("glossary.txt");
            symlink(&target, &link).unwrap();

            let settings = Settings {
                initial_prompt: "new = knew".to_string(),
                ..Default::default()
            };
            save_to(&path, &settings).unwrap();

            assert!(fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(fs::read_to_string(&target).unwrap(), "new = knew\n");
        });
    }

    #[cfg(unix)]
    #[test]
    fn settings_save_preserves_a_dotfiles_symlink() {
        use std::os::unix::fs::symlink;

        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            let target = dir.join("tracked-settings.json");
            fs::write(&target, "{}\n").unwrap();
            symlink(&target, &path).unwrap();

            let settings = Settings {
                language: Language::Swedish,
                ..Default::default()
            };
            save_to(&path, &settings).unwrap();

            assert!(fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
            assert_eq!(raw["language"], "sv");
        });
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_configuration_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "sagascript-private-config-{}",
            uuid::Uuid::new_v4()
        ));
        let path = root.join("config").join(SETTINGS_FILENAME);
        save_to(&path, &Settings::default()).unwrap();
        let mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_glossary_save_does_not_replace_the_file() {
        use std::os::unix::fs::MetadataExt;

        let root = std::env::temp_dir().join(format!(
            "sagascript-unchanged-glossary-{}",
            uuid::Uuid::new_v4()
        ));
        let settings_path = root.join(SETTINGS_FILENAME);
        let glossary_path = root.join(GLOBAL_GLOSSARY_FILENAME);
        fs::create_dir_all(&root).unwrap();
        fs::write(&glossary_path, "OpenRouter = open router\n").unwrap();
        let original_inode = fs::metadata(&glossary_path).unwrap().ino();
        let settings = Settings {
            initial_prompt: "OpenRouter = open router".to_string(),
            ..Default::default()
        };

        save_to(&settings_path, &settings).unwrap();

        assert_eq!(fs::metadata(&glossary_path).unwrap().ino(), original_inode);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fallible_update_does_not_persist_a_rejected_mutation() {
        with_temp_settings(|path| {
            save_to(&path, &Settings::default()).unwrap();
            let result = try_update_at(&path, |settings| {
                settings.hotkey = "invalid".to_string();
                Err("rejected".to_string())
            });
            assert_eq!(result.unwrap_err(), "rejected");
            assert_eq!(load_from(&path).hotkey, Settings::default().hotkey);
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
    fn load_accepts_both_legacy_and_canonical_onboarding_keys() {
        with_temp_settings(|path| {
            fs::write(
                &path,
                r#"{"language":"sv","hasCompletedOnboarding":false,"has_completed_onboarding":true}"#,
            )
            .unwrap();

            let loaded = load_from(&path);
            assert_eq!(loaded.language, Language::Swedish);
            assert!(loaded.has_completed_onboarding);
        });
    }

    #[test]
    fn save_preserves_unknown_keys_and_canonicalizes_legacy_key() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();

            // Pre-populate with the legacy camelCase onboarding key.
            let initial = serde_json::json!({
                "hasCompletedOnboarding": true,
                "language": "en",
                "future_key": {"x": 1}
            });
            fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

            // Save settings via merge
            let settings = Settings {
                language: Language::Norwegian,
                ..Default::default()
            };

            save_to(&path, &settings).unwrap();

            // Unknown data survives, but the alias is removed so serde cannot
            // see the same field twice on the next launch.
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert!(raw.get("hasCompletedOnboarding").is_none());
            assert_eq!(raw["has_completed_onboarding"], false);
            assert_eq!(raw["future_key"]["x"], 1);
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
        assert_eq!(s.file_transcription_model, FileModelPreference::Auto);
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
    fn legacy_presenter_settings_fixture_preserves_all_user_state() {
        with_temp_settings(|path| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                r#"{
                    "language":"sv",
                    "whisper_model":"kb-whisper-small",
                    "hotkey_mode":"presenter",
                    "hotkey":"Option+Space",
                    "hotkey_profiles":[{
                        "id":"swedish",
                        "name":"Swedish",
                        "shortcut":"Option+Space",
                        "language":"sv",
                        "push_to_talk_shortcut":"Control+Shift+P",
                        "toggle_shortcut":"Control+Shift+T"
                    }],
                    "initial_prompt":"OpenRouter = open router",
                    "profile_glossaries":{"swedish":"merge = merch"},
                    "presenter":{
                        "finish_shortcut":"Control+Shift+Enter",
                        "cancel_shortcut":"Option+Escape"
                    }
                }"#,
            )
            .unwrap();

            let loaded = load_from(&path);

            assert_eq!(loaded.language, Language::Swedish);
            assert_eq!(loaded.whisper_model, WhisperModel::KbWhisperSmall);
            assert_eq!(loaded.hotkey_mode, HotkeyMode::PushToTalk);
            assert_eq!(loaded.hotkey, "Option+Space");
            assert_eq!(loaded.hotkey_profiles.len(), 1);
            assert_eq!(loaded.hotkey_profiles[0].id, "swedish");
            assert_eq!(
                loaded.hotkey_profiles[0].push_to_talk_shortcut.as_deref(),
                Some("Control+Shift+P")
            );
            assert_eq!(
                loaded.hotkey_profiles[0].toggle_shortcut.as_deref(),
                Some("Control+Shift+T")
            );
            assert_eq!(loaded.initial_prompt, "OpenRouter = open router");
            assert_eq!(
                loaded.profile_glossaries.get("swedish").map(String::as_str),
                Some("merge = merch\nOpenRouter")
            );
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

    #[cfg(unix)]
    mod private_atomic_write {
        use super::*;
        use std::os::unix::fs::{symlink, PermissionsExt};

        const CHILD_ENV: &str = "SAGASCRIPT_UMASK_CHILD";

        #[cfg(target_os = "macos")]
        type ModeT = u16;
        #[cfg(not(target_os = "macos"))]
        type ModeT = u32;
        extern "C" {
            fn umask(mask: ModeT) -> ModeT;
        }

        fn mode(path: &Path) -> u32 {
            fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        fn names(dir: &Path) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }

        /// Re-runs the umask scenarios in a child process so the process-wide
        /// umask is never changed in this (parallel) test binary.
        #[test]
        fn private_modes_hold_under_umask_022_in_a_child_process() {
            if std::env::var_os(CHILD_ENV).is_some() {
                return;
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "settings::store::tests::private_atomic_write::umask_child_scenarios",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed"),
                "child failed:\n{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        #[test]
        fn umask_child_scenarios() {
            if std::env::var_os(CHILD_ENV).is_none() {
                return;
            }
            // The child process exits after this single test; never restored.
            // SAFETY: umask has no memory-safety preconditions.
            unsafe { umask(0o022) };

            // A private target stays private across repeated saves.
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("custom-settings.json");
            fs::write(&path, b"{}").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            for i in 0..3 {
                atomic_write(&path, format!("{{\"n\":{i}}}").as_bytes(), "settings").unwrap();
                assert_eq!(mode(&path), 0o600);
            }
            assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"n":2}"#);
            assert_eq!(names(dir.path()), vec!["custom-settings.json"]);

            // A new file is created private.
            let new = dir.path().join("glossary.txt");
            atomic_write(&new, b"term", "glossary").unwrap();
            assert_eq!(mode(&new), 0o600);

            // Narrower and broader existing modes are preserved.
            for m in [0o400, 0o640] {
                let p = dir.path().join(format!("m{m:o}.json"));
                fs::write(&p, b"{}").unwrap();
                fs::set_permissions(&p, fs::Permissions::from_mode(m)).unwrap();
                atomic_write(&p, b"{\"a\":1}", "settings").unwrap();
                assert_eq!(mode(&p), m);
                assert_eq!(fs::read(&p).unwrap(), b"{\"a\":1}");
            }

            // Relative and absolute symlinks are written through privately.
            let real = dir.path().join("dotfiles");
            fs::create_dir(&real).unwrap();
            for (name, absolute) in [("rel", false), ("abs", true)] {
                let target = real.join(format!("{name}.json"));
                fs::write(&target, b"{}").unwrap();
                fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
                let link = dir.path().join(format!("{name}-link.json"));
                if absolute {
                    symlink(&target, &link).unwrap();
                } else {
                    symlink(format!("dotfiles/{name}.json"), &link).unwrap();
                }
                atomic_write(&link, b"{\"x\":1}", "settings").unwrap();
                assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
                assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"x":1}"#);
                assert_eq!(mode(&target), 0o600);
            }
            assert_eq!(names(&real), vec!["abs.json", "rel.json"]);

            // The corrupt-settings backup is private.
            let settings = dir.path().join("cfg").join("settings.json");
            fs::create_dir_all(settings.parent().unwrap()).unwrap();
            fs::write(&settings, b"not json").unwrap();
            fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
            save_to(&settings, &Settings::default()).unwrap();
            assert_eq!(mode(&settings.with_extension("json.bak")), 0o600);
            assert_eq!(mode(&settings), 0o600);
        }

        #[test]
        fn failed_install_leaves_original_and_no_temp_file() {
            let dir = tempfile::tempdir().unwrap();
            // Renaming a file over a non-empty directory fails.
            let path = dir.path().join("settings.json");
            fs::create_dir(&path).unwrap();
            fs::write(path.join("keep"), b"original").unwrap();
            assert!(atomic_write(&path, b"{}", "settings").is_err());
            assert_eq!(fs::read(path.join("keep")).unwrap(), b"original");
            assert_eq!(names(dir.path()), vec!["settings.json"]);
        }

        #[test]
        fn permission_copy_failure_aborts_before_replacement() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            fs::write(&path, b"original").unwrap();
            let fail = |_: &fs::File, _: &fs::Metadata| {
                Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "injected"))
            };
            let error = install_replacement(&path, dir.path(), b"new", "settings", &fail).unwrap_err();
            assert!(error.contains("preserve settings permissions"), "{error}");
            assert_eq!(fs::read(&path).unwrap(), b"original");
            assert_eq!(names(dir.path()), vec!["settings.json"]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn restricted_dacl_survives_an_atomic_save() {
        use std::process::Command;

        fn acl(path: &Path) -> Vec<String> {
            let out = Command::new("icacls").arg(path).output().unwrap();
            assert!(out.status.success());
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let prefix = path.to_string_lossy().into_owned();
            let mut aces: Vec<String> = text
                .lines()
                .map(|l| l.replace(&prefix, "").trim().to_string())
                .filter(|l| l.contains(":("))
                .collect();
            aces.sort();
            aces
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"{}").unwrap();
        let user = std::env::var("USERNAME").unwrap();
        let status = Command::new("icacls")
            .arg(&path)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("{user}:F"))
            .output()
            .unwrap();
        assert!(status.status.success());
        let before = acl(&path);
        assert_eq!(before.len(), 1, "{before:?}");

        atomic_write(&path, b"{\"a\":1}", "settings").unwrap();
        atomic_write(&path, b"{\"a\":2}", "settings").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"a":2}"#);
        assert_eq!(acl(&path), before);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }
}
