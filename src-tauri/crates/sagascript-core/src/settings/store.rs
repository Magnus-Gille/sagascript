use std::{
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use fs2::FileExt;

use serde_json::Value;

use crate::settings::{
    lenient,
    migrations::{self, MachineProbe, Migration, Object},
    FileModelPreference, LineupEnv, Settings, SCHEMA_VERSION,
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

    // Moving the current bundle's settings into the XDG directory preserves
    // its TCC identity. Older bundle identifiers and FlowDictate do not.
    let same_bundle_settings = source.file_name().and_then(|name| name.to_str())
        == Some(SETTINGS_FILENAME)
        && source
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(APP_IDENTIFIER);
    migrations::from_legacy_location(object, same_bundle_settings);
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

/// The machine this process runs on. Probing it from the settings store is
/// the `settings` → `transcription` module cycle that #372 removes.
struct ThisMachine;

impl MachineProbe for ThisMachine {
    fn pianissimo_supported(&self) -> bool {
        crate::transcription::pianissimo_backend::runtime_supported_on_this_os()
    }

    fn lineup_env(&self) -> LineupEnv<'_> {
        LineupEnv {
            pianissimo_supported: self.pianissimo_supported(),
            pianissimo_downloaded: crate::transcription::pianissimo_model::is_downloaded(),
            whisper_downloaded: &crate::transcription::model::is_model_downloaded,
        }
    }
}

/// Load settings from disk, migrating an older settings file (see
/// [`migrations`]). Returns defaults if the file is missing or unreadable.
/// Partial JSON files are handled by `#[serde(default)]` on Settings.
pub fn load() -> Settings {
    load_at_with_machine(&settings_path(), legacy_settings_paths(), &ThisMachine)
}

/// Test seam for `tests/settings_fixtures.rs`: [`load`] for an explicit
/// settings file and legacy locations, with fixed machine state instead of
/// probing this machine.
#[doc(hidden)]
pub fn load_at(path: &Path, legacy_sources: &[PathBuf], machine: &LineupEnv<'_>) -> Settings {
    load_at_with_machine(path, legacy_sources.iter().cloned(), machine)
}

fn load_at_with_machine(
    path: &Path,
    legacy_sources: impl IntoIterator<Item = PathBuf>,
    machine: &dyn MachineProbe,
) -> Settings {
    let mut settings = load_migrated(path, legacy_sources, machine);
    // A settings file copied from a supported machine must not select an engine
    // this system cannot run. In memory only: the file keeps the user's choice.
    settings.demote_unsupported_pianissimo(machine.pianissimo_supported());
    settings
}

/// The startup load: copy settings from a legacy location on first use, run
/// every pending migration and persist the result under the settings lock.
/// A file with values that had to be reset is migrated in memory only: like
/// a file that cannot be loaded at all, it is replaced only by an explicit
/// save, which first preserves its bytes (see [`save_to`]).
fn load_migrated(
    path: &Path,
    legacy_sources: impl IntoIterator<Item = PathBuf>,
    machine: &dyn MachineProbe,
) -> Settings {
    // Settings files are installed with atomic rename, so an ordinary read
    // needs no lock. Only a file with a migration pending, or a missing one
    // that a legacy location may provide, is loaded under the writer lock.
    if !needs_startup_migration(path) {
        return load_from(path);
    }
    with_settings_lock(path, || {
        migrate_legacy_identifier_settings_locked(path, legacy_sources);
        let loaded = match load_checked(path, Some(machine)) {
            Ok(loaded) => loaded,
            Err(error) => return Ok(unmigrated_defaults(path, &error)),
        };
        let migration = loaded.migration.filter(Migration::ran);
        if let (Some(migration), false) = (migration, loaded.reset.is_empty()) {
            tracing::info!(
                "Settings file {} needs migrating from schema version {} to {}; it is migrated in memory only until a save, because it has values that had to be reset",
                path.display(),
                migration.from,
                migration.to
            );
        } else if let Some(migration) = migration {
            match save_to(path, &loaded.settings) {
                Ok(()) => tracing::info!(
                    "Migrated settings file {} from schema version {} to {}",
                    path.display(),
                    migration.from,
                    migration.to
                ),
                Err(error) => tracing::warn!(
                    "Failed to persist the settings migration to {}: {error}",
                    path.display()
                ),
            }
        }
        Ok(loaded.settings)
    })
    .unwrap_or_else(|error| {
        tracing::warn!(
            "Failed to lock settings file at {} for loading: {error} — falling back to defaults",
            path.display()
        );
        defaults_for(path)
    })
}

/// Whether the startup load needs the writer lock: the settings file is
/// missing (a legacy location may provide it) or has a migration pending. A
/// file that cannot be read or parsed is left to [`load_from`] to report.
fn needs_startup_migration(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(contents) => parse_settings_object(&contents)
            .is_ok_and(|object| migrations::starting_version(&object) < SCHEMA_VERSION),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A startup migration never persists over a settings file it could not load.
/// Warn once and use defaults in memory; the file stays as it is until an
/// explicit save, which preserves its bytes first (see [`save_to`]).
fn unmigrated_defaults(path: &Path, error: &SettingsLoadError) -> Settings {
    tracing::warn!(
        "Settings file at {} could not be loaded ({error}); skipping its migrations, which leaves the file untouched, and using defaults in memory",
        path.display()
    );
    defaults_for(path)
}

/// Why the bytes of a settings file cannot be used as [`Settings`].
#[derive(Debug)]
enum SettingsParseError {
    /// Not UTF-8, not JSON, or not a JSON object.
    Syntax(String),
    /// A JSON object that does not deserialize as `Settings`, such as one with
    /// a wrong-typed known field.
    Schema(String),
}

impl std::fmt::Display for SettingsParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax(reason) => write!(f, "not a valid JSON settings object: {reason}"),
            Self::Schema(reason) => f.write_str(reason),
        }
    }
}

/// Why [`load_checked`] could not load an existing settings file.
#[derive(Debug)]
enum SettingsLoadError {
    /// Any read error other than a missing file.
    Read(std::io::Error),
    Syntax(String),
    Schema(String),
}

impl From<SettingsParseError> for SettingsLoadError {
    fn from(error: SettingsParseError) -> Self {
        match error {
            SettingsParseError::Syntax(reason) => Self::Syntax(reason),
            SettingsParseError::Schema(reason) => Self::Schema(reason),
        }
    }
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => write!(f, "read failed: {error}"),
            Self::Syntax(reason) => write!(f, "not a valid JSON settings object: {reason}"),
            Self::Schema(reason) => f.write_str(reason),
        }
    }
}

/// Syntax stage of every load: the bytes must be a UTF-8 JSON object.
fn parse_settings_object(contents: &[u8]) -> Result<Object, SettingsParseError> {
    let text = std::str::from_utf8(contents)
        .map_err(|error| SettingsParseError::Syntax(format!("not UTF-8 ({error})")))?;
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(SettingsParseError::Syntax(
            "the top level is not a JSON object".to_string(),
        )),
        Err(error) => Err(SettingsParseError::Syntax(error.to_string())),
    }
}

const PROFILE_MODELS_KEY: &str = "profile_models";

/// `profile_models` is lenient per entry (ADR 0007): an entry that does not
/// read, such as a model id from a newer release, is dropped on its own, and
/// a value that is not a map is dropped whole. Runs before the migrations,
/// which give every profile left without a model the one its old selection
/// implies. Returns whether anything was dropped, which [`decode_object`]
/// counts as a reset of `profile_models`: like any reset value, the dropped
/// entries stay in the file until a save, which first preserves its bytes.
fn drop_invalid_profile_models(object: &mut Object) -> bool {
    let Some(models) = object.get_mut(PROFILE_MODELS_KEY) else {
        return false;
    };
    let Some(entries) = models.as_object_mut() else {
        object.remove(PROFILE_MODELS_KEY);
        return true;
    };
    let before = entries.len();
    entries.retain(|id, model| {
        validate_profile_file_id(id).is_ok()
            && model
                .as_str()
                .is_some_and(|id| FileModelPreference::parse_id(id).is_ok())
    });
    entries.len() != before
}

/// [`lenient::decode`] of an object [`drop_invalid_profile_models`] ran on,
/// with `profile_models` among the reset fields if it dropped anything.
fn decode_object(
    object: &Object,
    dropped_profile_models: bool,
) -> Result<lenient::Decoded, String> {
    let mut decoded = lenient::decode(object)?;
    if dropped_profile_models && !decoded.reset.iter().any(|key| key == PROFILE_MODELS_KEY) {
        decoded.reset.push(PROFILE_MODELS_KEY.to_string());
        decoded.reset.sort();
    }
    Ok(decoded)
}

/// Whether a settings file's object reads as [`Settings`] as it is, with
/// no field reset. A save over a file that does not first preserves its
/// bytes (see [`existing_settings_for_merge`]).
fn check_settings_object(mut object: Object) -> Result<(), SettingsParseError> {
    migrations::canonical_onboarding_key(&mut object);
    let dropped_profile_models = drop_invalid_profile_models(&mut object);
    match decode_object(&object, dropped_profile_models) {
        Ok(decoded) if decoded.reset.is_empty() => Ok(()),
        Ok(decoded) => Err(SettingsParseError::Schema(invalid_fields(&decoded.reset))),
        Err(reason) => Err(SettingsParseError::Schema(format!(
            "invalid settings: {reason}"
        ))),
    }
}

/// Names, never values: a settings value can be anything the user typed.
fn invalid_fields(fields: &[String]) -> String {
    let noun = if fields.len() == 1 { "value" } else { "values" };
    format!("invalid {noun} for {}", fields.join(", "))
}

/// The settings a settings file's object holds: its personal dictionaries
/// read in from their files and its pending migrations run. `machine`
/// enables the steps that need machine state (the startup load and field
/// updates). A field that does not decode takes its default (see
/// [`lenient`]) and is reported.
fn settings_from_file_object(
    path: &Path,
    mut object: Object,
    machine: Option<&dyn MachineProbe>,
) -> Result<FileSettings, SettingsParseError> {
    // Taken before the dictionaries are read in: embedding them is a legacy shape.
    let from = migrations::starting_version(&object);
    read_dictionary_files(path, &mut object);
    let dropped_profile_models = drop_invalid_profile_models(&mut object);
    let migration = migrations::migrate(&mut object, from, machine)
        .map_err(|error| SettingsParseError::Schema(error.to_string()))?;
    let decoded = decode_object(&object, dropped_profile_models)
        .map_err(|reason| SettingsParseError::Schema(format!("invalid settings: {reason}")))?;
    if !decoded.reset.is_empty() {
        tracing::warn!(
            "Settings file {} has {}; ignoring what cannot be read. The file keeps it until the next save, which first saves a recovery copy of it",
            path.display(),
            invalid_fields(&decoded.reset)
        );
    }
    Ok(FileSettings {
        settings: decoded.settings,
        migration,
        reset: decoded.reset,
    })
}

/// What [`settings_from_file_object`] made of a settings object.
struct FileSettings {
    settings: Settings,
    migration: Migration,
    /// Fields that did not decode and took their defaults.
    reset: Vec<String>,
}

/// A settings file as loaded, what migrating it did (`None`: no file) and
/// the fields that had to be reset.
struct Loaded {
    settings: Settings,
    migration: Option<Migration>,
    reset: Vec<String>,
}

/// Load settings from a specific path, reporting why an existing file could
/// not be used. A missing file is not an error: it loads as defaults.
fn load_checked(
    path: &Path,
    machine: Option<&dyn MachineProbe>,
) -> Result<Loaded, SettingsLoadError> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Loaded {
                settings: defaults_for(path),
                migration: None,
                reset: Vec::new(),
            })
        }
        Err(error) => return Err(SettingsLoadError::Read(error)),
    };
    let object = parse_settings_object(&contents)?;
    let loaded = settings_from_file_object(path, object, machine)?;
    Ok(Loaded {
        settings: loaded.settings,
        migration: Some(loaded.migration),
        reset: loaded.reset,
    })
}

/// Defaults as a load would return them: the personal dictionary files read
/// in and the migrations without machine state run.
fn defaults_for(path: &Path) -> Settings {
    let defaults = Settings::default();
    match serde_json::to_value(&defaults) {
        Ok(Value::Object(object)) => {
            settings_from_file_object(path, object, None).map_or(defaults, |loaded| loaded.settings)
        }
        _ => defaults,
    }
}

/// Load settings from a specific path, without machine state: the steps
/// that need it wait for a load that has it (see [`migrations`]). A value
/// that cannot be read takes its default; a file that is missing or cannot
/// be read or parsed at all loads as defaults. Saving over an invalid file
/// preserves its bytes first (see [`save_to`]).
pub fn load_from(path: &Path) -> Settings {
    load_checked(path, None).map_or_else(
        |error| defaults_after_failed_load(path, &error),
        |loaded| loaded.settings,
    )
}

/// The settings a load of a file [`load_checked`] could not load returns.
fn defaults_after_failed_load(path: &Path, error: &SettingsLoadError) -> Settings {
    tracing::warn!(
        "Failed to load settings file at {} ({error}) — falling back to defaults",
        path.display()
    );
    defaults_for(path)
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
    try_update(|settings| {
        mutate(settings);
        Ok(())
    })
}

/// Like [`update`], but lets validation fail while the cross-process lock is
/// held so read/validate/write profile mutations cannot race another writer.
pub fn try_update<F>(mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    let path = settings_path();
    update_locked(&path, legacy_settings_paths(), &ThisMachine, mutate)
}

/// [`try_update`] of `path` (no legacy locations) on a machine with
/// Pianissimo supported and every model downloaded.
#[cfg(test)]
fn try_update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    update_locked(path, std::iter::empty(), &tests::ready_machine(), mutate)
}

/// [`update`] as [`try_update_at`] runs it.
#[cfg(test)]
fn update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings),
{
    try_update_at(path, |settings| {
        mutate(settings);
        Ok(())
    })
}

/// A field update under the settings lock. The latest file is loaded as the
/// startup load does, with every pending migration step, machine state
/// included: the save records `schema_version` and the steps' markers, so a
/// step left pending here would run at the next start over the value the
/// user has just chosen. A file that cannot be loaded is updated from
/// defaults, so the save first preserves its bytes (see [`save_over`]).
fn update_locked<F>(
    path: &Path,
    sources: impl IntoIterator<Item = PathBuf>,
    machine: &dyn MachineProbe,
    mutate: F,
) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    with_settings_lock(path, || {
        migrate_legacy_identifier_settings_locked(path, sources);
        let (mut settings, load_failure) = match load_checked(path, Some(machine)) {
            Ok(loaded) => (loaded.settings, None),
            Err(error) => (defaults_after_failed_load(path, &error), Some(error)),
        };
        mutate(&mut settings)?;
        save_over(path, &settings, load_failure.as_ref())?;
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
    // fs2's trait method, spelled out: from Rust 1.89 the inherent
    // `File::unlock` shadows it, and the 1.88 MSRV does not have that.
    if let Err(e) = FileExt::unlock(&lock_file) {
        tracing::warn!(
            "Failed to unlock settings file {}: {e}",
            lock_path.display()
        );
    }
    result
}

/// Persist settings to a specific path. Test seam for `save`, which always
/// targets `settings_path()`. Caller must hold the settings lock.
fn save_to(path: &Path, settings: &Settings) -> Result<(), String> {
    save_over(path, settings, None)
}

/// [`save_to`] for settings that come from a load of `path`. `load_failure`
/// is why that load could not use the file: the file is then preserved
/// before it is replaced even if it reads as valid settings (a migration
/// step failed on it, say), because the settings written are defaults.
fn save_over(
    path: &Path,
    settings: &Settings,
    load_failure: Option<&SettingsLoadError>,
) -> Result<(), String> {
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Ensure directory exists
    create_private_dir_all(&dir).map_err(|e| format!("Failed to create settings dir: {e}"))?;

    // Read existing file to preserve non-settings keys. This is the one place
    // every settings write passes through, so it is also where an unreadable
    // or invalid file is protected before anything is replaced.
    let (mut map, recovered) = existing_settings_for_merge(path, load_failure)?;
    let written = merge_and_write(path, settings, &mut map);
    if let Some(recovered) = recovered {
        return match written {
            Ok(()) => {
                tracing::warn!(
                    "Settings file {} was invalid ({}). Its exact original contents are preserved at {}; the file was rewritten from the current settings, so restore any lost values from that copy.",
                    path.display(),
                    recovered.problem,
                    recovered.copy.display()
                );
                Ok(())
            }
            Err(error) => Err(format!(
                "{error} (the invalid original settings file is preserved at {})",
                recovered.copy.display()
            )),
        };
    }
    written
}

/// The merge-and-write half of [`save_to`], once the existing file is safe to
/// replace.
fn merge_and_write(
    path: &Path,
    settings: &Settings,
    map: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    migrations::canonical_onboarding_key(map);

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

    atomic_write(path, json.as_bytes(), "settings")
}

/// An invalid settings file whose exact bytes [`preserve_invalid_settings`]
/// saved before `save_to` replaces it.
struct RecoveredSettings {
    problem: String,
    copy: PathBuf,
}

/// The existing JSON object `save_to` merges into, after making it safe to
/// replace the file. Caller must hold the settings lock.
///
/// - Missing: an empty object.
/// - Unreadable: an error, so the file is never replaced.
/// - Valid settings, unless `load_failure` says their load failed: its
///   object, so unknown keys survive.
/// - Invalid settings: its exact bytes are first preserved by
///   [`preserve_invalid_settings`] (an error if that fails, so nothing is
///   written). Then its object for a schema error or a failed load, so
///   unknown keys survive, or an empty object for a syntax error, plus where
///   the copy went.
fn existing_settings_for_merge(
    path: &Path,
    load_failure: Option<&SettingsLoadError>,
) -> Result<
    (
        serde_json::Map<String, serde_json::Value>,
        Option<RecoveredSettings>,
    ),
    String,
> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((serde_json::Map::new(), None))
        }
        Err(error) => {
            return Err(format!(
                "Settings file {} could not be read ({error}); refusing to overwrite it",
                path.display()
            ))
        }
    };
    // The load already reported any ignored profile-model entries.
    let (problem, merge_base) = match parse_settings_object(&contents) {
        Err(problem) => (problem.to_string(), serde_json::Map::new()),
        Ok(object) => match (check_settings_object(object.clone()), load_failure) {
            (Ok(()), None) => return Ok((object, None)),
            (Ok(()), Some(failure)) => (failure.to_string(), object),
            (Err(problem), _) => (problem.to_string(), object),
        },
    };
    let recovery = preserve_invalid_settings(path, &contents).map_err(|error| {
        format!(
            "Settings file {} is invalid ({problem}) and a recovery copy could not be saved ({error}); refusing to overwrite it",
            path.display()
        )
    })?;
    Ok((
        merge_base,
        Some(RecoveredSettings {
            problem,
            copy: recovery,
        }),
    ))
}

/// Bound on numbered recovery copies, so the search always ends.
const MAX_RECOVERY_COPIES: usize = 1000;

/// Preserve the exact bytes of an invalid settings file before it is replaced
/// and return where they are. Caller must hold the settings lock.
///
/// Never overwrites anything: the first copy is `settings.json.bak`, later ones
/// `settings.json.bak.1`, `settings.json.bak.2`, ..., each created exclusively.
/// A candidate that already holds these exact bytes is reused instead of
/// duplicated. A new copy is owner-only (0600 on Unix) and synced, with its
/// directory, before this returns.
fn preserve_invalid_settings(path: &Path, contents: &[u8]) -> Result<PathBuf, String> {
    #[cfg(test)]
    if FAIL_RECOVERY_COPY.with(std::cell::Cell::get) {
        return Err("injected recovery copy failure".to_string());
    }

    let first = path.with_extension("json.bak");
    let dir = match first.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    for index in 0..MAX_RECOVERY_COPIES {
        let candidate = if index == 0 {
            first.clone()
        } else {
            let mut name = first.clone().into_os_string();
            name.push(format!(".{index}"));
            PathBuf::from(name)
        };
        match create_new_private_file(&candidate) {
            Ok(file) => {
                write_recovery_copy(file, &candidate, &dir, contents)?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // Only a regular file (not a symlink) with these exact bytes
                // counts as an earlier copy; anything else is skipped.
                let identical = std::fs::symlink_metadata(&candidate).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.len() == contents.len() as u64
                }) && std::fs::read(&candidate)
                    .is_ok_and(|existing| existing == contents);
                if identical {
                    return Ok(candidate);
                }
            }
            Err(error) => return Err(format!("Failed to create {}: {error}", candidate.display())),
        }
    }
    Err(format!(
        "{MAX_RECOVERY_COPIES} recovery copies of {} already exist",
        path.display()
    ))
}

fn write_recovery_copy(
    mut file: std::fs::File,
    candidate: &Path,
    dir: &Path,
    contents: &[u8],
) -> Result<(), String> {
    use std::io::Write;

    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        // Created exclusively above, so this incomplete copy is ours to remove.
        let _ = std::fs::remove_file(candidate);
        return Err(format!("Failed to write {}: {error}", candidate.display()));
    }
    sync_directory(dir).map_err(|error| {
        format!(
            "Saved {} but failed to sync its directory: {error}",
            candidate.display()
        )
    })
}

/// Create a new owner-only file, failing if anything (even a dangling
/// symlink) already has that name.
fn create_new_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
thread_local! {
    static FAIL_RECOVERY_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test-only fault injection: while alive, recovery copies on this thread fail.
#[cfg(test)]
struct FailRecoveryCopy;

#[cfg(test)]
impl FailRecoveryCopy {
    fn enable() -> Self {
        FAIL_RECOVERY_COPY.with(|fail| fail.set(true));
        Self
    }
}

#[cfg(test)]
impl Drop for FailRecoveryCopy {
    fn drop(&mut self) {
        FAIL_RECOVERY_COPY.with(|fail| fail.set(false));
    }
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

pub(super) fn validate_profile_file_id(profile_id: &str) -> Result<(), String> {
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

/// Read the personal dictionaries, which live in their own files since 1.1.0,
/// into the settings object as `initial_prompt` and `profile_glossaries`.
/// A file wins over a value embedded in the settings file, which only
/// releases before 1.1.0 wrote.
fn read_dictionary_files(path: &Path, object: &mut Object) {
    let global_path = global_glossary_path_for_settings(path);
    match std::fs::read_to_string(&global_path) {
        Ok(contents) => {
            object.insert(
                "initial_prompt".to_string(),
                Value::String(normalize_glossary_file(contents)),
            );
        }
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

    let mut files = Vec::new();
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
            Ok(contents) => files.push((profile_id.to_string(), normalize_glossary_file(contents))),
            Err(error) => tracing::warn!(
                "Failed to read profile personal dictionary at {}: {error}",
                entry_path.display()
            ),
        }
    }
    if files.is_empty() {
        return;
    }
    if !matches!(
        object.get("profile_glossaries"),
        None | Some(Value::Object(_))
    ) {
        tracing::warn!("Ignoring malformed embedded profile dictionaries");
        object.remove("profile_glossaries");
    }
    if let Value::Object(dictionaries) = object
        .entry("profile_glossaries")
        .or_insert_with(|| Value::Object(Object::new()))
    {
        for (profile_id, contents) in files {
            dictionaries.insert(profile_id, Value::String(contents));
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
    install_replacement(
        &destination,
        &parent,
        contents,
        label,
        &copy_target_permissions,
    )
}

/// Copy an existing regular target's permissions onto the replacement file.
fn copy_target_permissions(
    file: &std::fs::File,
    existing: &std::fs::Metadata,
) -> std::io::Result<()> {
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
    // Close the handle; the TempPath removes the replacement unless kept.
    let temp_path = tmp.into_temp_path();
    // A backup name is mandatory for safety. Microsoft's ReplaceFileW remarks:
    // on ERROR_UNABLE_TO_MOVE_REPLACEMENT (1176), "If lpBackupFileName was
    // specified, the replaced and replacement files retain their original file
    // names. Otherwise, the replaced file no longer exists and the replacement
    // file exists under its original name." On ERROR_UNABLE_TO_MOVE_REPLACEMENT_2
    // (1177), "the replaced file exists under the name of the backup file".
    // Reserve a unique, never auto-deleted name next to the target.
    let backup = {
        let reserved = tempfile::Builder::new()
            .prefix(".sagascript-bak-")
            .suffix(".tmp")
            .tempfile_in(destination.parent().unwrap_or_else(|| Path::new(".")))
            .map_err(|error| format!("Failed to reserve {label} backup name: {error}"))?
            .into_temp_path();
        let path = reserved.to_path_buf();
        reserved
            .close()
            .map_err(|error| format!("Failed to reserve {label} backup name: {error}"))?;
        path
    };
    let wide =
        |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let replaced = wide(destination);
    let replacement = wide(&temp_path);
    let backup_wide = wide(&backup);
    // SAFETY: all buffers are NUL-terminated and outlive the call; the
    // remaining pointer arguments are documented as reserved nulls.
    let ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            backup_wide.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok != 0 {
        if let Err(error) = std::fs::remove_file(&backup) {
            tracing::warn!(
                "Failed to remove {label} backup {}: {error}",
                backup.display()
            );
        }
        return Ok(());
    }
    let os_error = std::io::Error::last_os_error();
    const ERROR_UNABLE_TO_MOVE_REPLACEMENT_2: i32 = 1177;
    if os_error.raw_os_error() == Some(ERROR_UNABLE_TO_MOVE_REPLACEMENT_2) {
        // The original now lives under the backup name: move it back.
        if let Err(restore) = std::fs::rename(&backup, destination) {
            let kept = temp_path.keep().ok();
            return Err(format!(
                "Failed to install {label}: {os_error}; restoring the original from {} also failed ({restore}); new contents kept at {}",
                backup.display(),
                kept.map_or_else(|| "(unknown)".to_string(), |p| p.display().to_string())
            ));
        }
    }
    // Never let cleanup delete the only remaining copy.
    if !destination.exists() {
        let kept = temp_path.keep().ok();
        return Err(format!(
            "Failed to install {label}: {os_error}; the original is missing, new contents kept at {}",
            kept.map_or_else(|| "(unknown)".to_string(), |p| p.display().to_string())
        ));
    }
    let _ = std::fs::remove_file(&backup);
    Err(format!("Failed to install {label}: {os_error}"))
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
    use crate::settings::{
        EnginePrewarm, FileModelPreference, HotkeyMode, Language, WhisperModel,
        ENGINE_DEFAULTS_VERSION, MODEL_LINEUP_VERSION,
    };
    use std::fs;
    use std::sync::mpsc;
    use std::thread;

    fn every_model_ready(_: WhisperModel) -> bool {
        true
    }

    /// Pianissimo supported and every model downloaded.
    pub(super) fn ready_machine() -> LineupEnv<'static> {
        LineupEnv {
            pianissimo_supported: true,
            pianissimo_downloaded: true,
            whisper_downloaded: &every_model_ready,
        }
    }

    /// The startup load of `path` (no legacy locations) on [`ready_machine`].
    fn startup_load(path: &Path) -> Settings {
        load_at_with_machine(path, std::iter::empty(), &ready_machine())
    }

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
            let migrated = startup_load(&path);
            assert_eq!(
                migrated.profile_models["default"],
                FileModelPreference::PianissimoOriginal
            );
            assert_eq!(
                migrated.profile_models["english"],
                FileModelPreference::Auto
            );
            assert_eq!(migrated.hotkey_profiles[0].shortcut, "Super+S");
            let persisted: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["profile_models"]["default"], "pianissimo-sv");
            let reloaded = load_from(&path);
            assert_eq!(reloaded.profile_models, migrated.profile_models);
        });
    }

    #[test]
    fn engine_defaults_migration_persists_once_and_keeps_explicit_choices() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","engine_prewarm":"on_key_down","engine_idle_unload_minutes":10,"future_key":1}"#).unwrap();
            let migrated = startup_load(&path);
            assert_eq!(migrated.engine_prewarm, EnginePrewarm::OnAppStart);
            assert_eq!(migrated.engine_idle_unload_minutes, 60);
            let persisted: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["engine_prewarm"], "on_app_start");
            assert_eq!(persisted["engine_idle_unload_minutes"], 60);
            assert_eq!(
                persisted["engine_defaults_version"],
                ENGINE_DEFAULTS_VERSION
            );
            assert_eq!(persisted["future_key"], 1);
            let before = fs::read_to_string(&path).unwrap();
            startup_load(&path);
            assert_eq!(fs::read_to_string(&path).unwrap(), before);

            fs::write(
                &path,
                r#"{"engine_prewarm":"off","engine_idle_unload_minutes":10}"#,
            )
            .unwrap();
            let off = startup_load(&path);
            assert_eq!(off.engine_prewarm, EnginePrewarm::Off);
            assert_eq!(off.engine_idle_unload_minutes, 60);
        });
    }

    #[test]
    fn swedish_lineup_migration_persists_once_and_is_idempotent_on_disk() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","auto_select_model":false,"whisper_model":"kb-whisper-small","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-small"}}"#).unwrap();
            let ready = |_: WhisperModel| true;
            let env = LineupEnv {
                pianissimo_supported: true,
                pianissimo_downloaded: true,
                whisper_downloaded: &ready,
            };
            let migrated = load_at_with_machine(&path, std::iter::empty(), &env);
            assert_eq!(
                migrated.profile_models["default"],
                FileModelPreference::PianissimoOriginal
            );
            let persisted: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["profile_models"]["default"], "pianissimo-sv");
            assert_eq!(persisted["model_lineup_version"], MODEL_LINEUP_VERSION);

            // A second load neither re-runs nor rewrites the migration.
            let before = fs::read_to_string(&path).unwrap();
            let again = load_at_with_machine(&path, std::iter::empty(), &env);
            assert_eq!(again.profile_models, migrated.profile_models);
            assert_eq!(fs::read_to_string(&path).unwrap(), before);
        });
    }

    #[test]
    fn lineup_migration_preserves_unknown_keys_profiles_and_glossaries() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","future_key":{"a":[1,2]},"profile_glossary_migrated":true,"profile_glossaries":{"default":"Sagascript"},"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}],"profile_models":{"default":"kb-whisper-small","english":"auto"}}"#).unwrap();
            let ready = |_: WhisperModel| true;
            let env = LineupEnv {
                pianissimo_supported: true,
                pianissimo_downloaded: true,
                whisper_downloaded: &ready,
            };
            let migrated = load_at_with_machine(&path, std::iter::empty(), &env);
            assert_eq!(
                migrated.profile_models["default"],
                FileModelPreference::PianissimoOriginal
            );
            let persisted: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
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
            let env = LineupEnv {
                pianissimo_supported: true,
                pianissimo_downloaded: true,
                whisper_downloaded: &ready,
            };
            let migrated = load_at_with_machine(&path, std::iter::empty(), &env);
            assert!(
                !migrated.profile_models.contains_key("default")
                    || migrated.profile_models["default"] == FileModelPreference::Auto
            );
            assert_eq!(migrated.model_lineup_version, MODEL_LINEUP_VERSION);
        });
    }

    #[test]
    fn swedish_lineup_migration_keeps_an_undownloaded_target_out_of_the_way() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-base"}}"#).unwrap();
            let only_base = |model: WhisperModel| model == WhisperModel::KbWhisperBase;
            let env = LineupEnv {
                pianissimo_supported: true,
                pianissimo_downloaded: false,
                whisper_downloaded: &only_base,
            };
            let migrated = load_at_with_machine(&path, std::iter::empty(), &env);
            assert_eq!(
                migrated.profile_models["default"],
                FileModelPreference::Whisper(WhisperModel::KbWhisperBase)
            );
            // The marker is recorded even though nothing moved, so it never re-runs.
            let persisted: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(persisted["model_lineup_version"], MODEL_LINEUP_VERSION);
        });
    }

    #[test]
    fn global_glossary_migrates_once_to_each_profile_without_deleting_rollback_source() {
        with_temp_settings(|path| {
            fs::write(&path, r#"{"language":"sv","hotkey":"Super+S","initial_prompt":"OpenRouter","hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}],"profile_glossaries":{"default":"merge = merch"}}"#).unwrap();
            let migrated = startup_load(&path);
            assert!(migrated.profile_glossary_migrated);
            assert!(migrated.profile_glossaries["default"].contains("OpenRouter"));
            assert!(migrated.profile_glossaries["english"].contains("OpenRouter"));
            assert!(migrated.profile_glossaries["default"].contains("merge = merch"));
            assert_eq!(
                fs::read_to_string(global_glossary_path_for_settings(&path)).unwrap(),
                "OpenRouter\n"
            );
            let reloaded = startup_load(&path);
            assert_eq!(reloaded.profile_glossaries, migrated.profile_glossaries);
            assert!(!reloaded
                .effective_glossary_source(Some("english"))
                .contains("merge = merch"));
        });
    }

    #[test]
    fn field_update_before_startup_runs_every_pending_step() {
        with_temp_settings(|path| {
            // A 1.3 file, updated (say by the CLI) before the app starts:
            // the update records the whole migration, machine steps included.
            fs::write(&path, r#"{"language":"sv","auto_select_model":false,"whisper_model":"kb-whisper-small","engine_prewarm":"on_key_down","hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}]}"#).unwrap();
            let updated = update_at(&path, |settings| settings.beam_size = 5).unwrap();
            assert_eq!(updated.schema_version, SCHEMA_VERSION);
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], SCHEMA_VERSION);
            assert_eq!(raw["beam_size"], 5);
            assert_eq!(raw["profile_models"]["default"], "pianissimo-sv");
            assert_eq!(raw["model_lineup_version"], MODEL_LINEUP_VERSION);
            assert_eq!(raw["engine_prewarm"], "on_app_start");
            assert_eq!(raw["engine_defaults_version"], ENGINE_DEFAULTS_VERSION);

            // Nothing is left for the startup load to migrate or write.
            let written = fs::read(&path).unwrap();
            let loaded = startup_load(&path);
            assert_eq!(loaded.beam_size, 5);
            assert_file_bytes(&path, &written);
        });
    }

    #[test]
    fn a_choice_made_between_repair_and_restart_survives_the_restart() {
        with_temp_settings(|path| {
            // A 1.4.0 file with one invalid value: the startup load repairs it
            // and runs the pending lineup and engine steps in memory only.
            let original: &[u8] = br#"{"language":"sv","auto_select_model":false,"whisper_model":"kb-whisper-small","engine_prewarm":"on_key_down","engine_idle_unload_minutes":10,"profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-small"},"auto_paste":"invalid-bool"}"#;
            fs::write(&path, original).unwrap();
            let repaired = startup_load(&path);
            assert_eq!(repaired.engine_prewarm, EnginePrewarm::OnAppStart);
            assert_eq!(
                repaired.profile_models["default"],
                FileModelPreference::PianissimoOriginal
            );
            assert_file_bytes(&path, original);

            // Before the next start, the user picks the values those steps
            // move away from: one through `update`, one through `try_update`.
            let updated = update_at(&path, |settings| {
                settings.engine_prewarm = EnginePrewarm::OnKeyDown;
            })
            .unwrap();
            // The update starts from what the startup load showed.
            assert_eq!(
                updated.profile_models["default"],
                FileModelPreference::PianissimoOriginal
            );
            try_update_at(&path, |settings| {
                settings.profile_models.insert(
                    "default".into(),
                    FileModelPreference::Whisper(WhisperModel::KbWhisperSmall),
                );
                Ok(())
            })
            .unwrap();
            assert_file_bytes(recovery_copy_path(&path, 0), original);

            // The next start (or the settings watcher's reload) keeps both.
            let restarted = startup_load(&path);
            assert_eq!(restarted.engine_prewarm, EnginePrewarm::OnKeyDown);
            assert_eq!(
                restarted.profile_models["default"],
                FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
            );
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], SCHEMA_VERSION);
            assert_eq!(raw["engine_prewarm"], "on_key_down");
            assert_eq!(raw["profile_models"]["default"], "kb-whisper-small");
        });
    }

    #[test]
    fn current_file_without_schema_version_is_not_rewritten_at_startup() {
        with_temp_settings(|path| {
            // As 1.4.3 writes it: every legacy marker, no schema_version.
            let written = br#"{"language":"sv","profile_glossary_migrated":true,"model_lineup_version":1,"engine_defaults_version":1,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-small"},"future_key":7}"#;
            fs::write(&path, written).unwrap();
            let loaded = startup_load(&path);
            assert_file_bytes(&path, written);
            assert_eq!(loaded.schema_version, SCHEMA_VERSION);
            assert_eq!(
                loaded.profile_models["default"],
                FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
            );
            // The next write records it, and keeps every other key.
            update_at(&path, |settings| settings.beam_size = 2).unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], SCHEMA_VERSION);
            assert_eq!(raw["future_key"], 7);
            assert_eq!(raw["model_lineup_version"], MODEL_LINEUP_VERSION);
            assert_eq!(raw["engine_defaults_version"], ENGINE_DEFAULTS_VERSION);
            assert_eq!(raw["profile_glossary_migrated"], true);
        });
    }

    #[test]
    fn newer_release_file_is_read_as_is_and_written_as_this_version() {
        with_temp_settings(|path| {
            let written = br#"{"schema_version":9,"language":"sv","profile_glossary_migrated":true,"model_lineup_version":1,"engine_defaults_version":1,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"auto"},"newer_key":{"a":1}}"#;
            fs::write(&path, written).unwrap();
            assert_eq!(startup_load(&path).language, Language::Swedish);
            assert_file_bytes(&path, written);
            // This build only vouches for its own version, so the newer
            // release checks the file again after this write.
            update_at(&path, |settings| settings.beam_size = 2).unwrap();
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], SCHEMA_VERSION);
            assert_eq!(raw["newer_key"]["a"], 1);
        });
    }

    #[test]
    fn invalid_profile_model_is_reset_in_memory_and_preserved_before_any_rewrite() {
        // A model id from a newer release, in an old file and in a current one.
        let unversioned: &[u8] =
            br#"{"language":"sv","hotkey":"Super+S","profile_models":{"default":"not-a-model"}}"#;
        let current: &[u8] = br#"{"schema_version":6,"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"model_lineup_version":1,"engine_defaults_version":1,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"},{"id":"english","name":"English","shortcut":"Super+E","language":"en"}],"profile_models":{"default":"kb-whisper-small","english":"whisper-xl-2027"},"future_key":7}"#;
        for original in [unversioned, current] {
            with_temp_settings(|path| {
                fs::write(&path, original).unwrap();
                let problem = match parse_settings_object(original) {
                    Ok(object) => check_settings_object(object).unwrap_err().to_string(),
                    Err(error) => panic!("{error}"),
                };
                assert_eq!(problem, "invalid value for profile_models");

                // Only the unreadable entry takes its default, in memory.
                let loaded = startup_load(&path);
                assert_eq!(loaded.default_profile().language, Language::Swedish);
                assert_eq!(loaded.default_profile().shortcut, "Super+S");
                let unreadable = if original == unversioned {
                    "default"
                } else {
                    "english"
                };
                assert_eq!(loaded.profile_models[unreadable], FileModelPreference::Auto);
                if original == current {
                    assert_eq!(
                        loaded.profile_models["default"],
                        FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
                    );
                }
                // The startup migration never rewrites a file it had to repair.
                assert_file_bytes(&path, original);
                assert!(!recovery_copy_path(&path, 0).exists());

                // The next save first preserves the exact original bytes.
                let updated = update_at(&path, |settings| settings.beam_size = 9).unwrap();
                assert_eq!(
                    updated.profile_models[unreadable],
                    FileModelPreference::Auto
                );
                assert_file_bytes(recovery_copy_path(&path, 0), original);
                let raw: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                assert_eq!(raw["beam_size"], 9);
                assert_eq!(raw["language"], "sv");
                assert_eq!(raw["profile_models"][unreadable], "auto");
                if original == current {
                    assert_eq!(raw["profile_models"]["default"], "kb-whisper-small");
                    assert_eq!(raw["future_key"], 7);
                }
            });
        }
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

        let loaded = load_at_with_machine(&destination, [legacy.clone()], &ready_machine());

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

        let loaded = startup_load(&destination);

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
            update_locked(
                &update_destination,
                [update_legacy],
                &ready_machine(),
                |settings| {
                    settings.language = Language::Norwegian;
                    Ok(())
                },
            )
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
    fn load_from_invalid_enum_value_resets_that_field() {
        with_temp_settings(|path| {
            let dir = path.parent().unwrap();
            fs::create_dir_all(dir).unwrap();
            // "de" is not a valid Language variant
            fs::write(&path, r#"{"language":"de","beam_size":4}"#).unwrap();

            let s = load_from(&path);
            let d = Settings::default();
            // Only the field that cannot be read takes its default.
            assert_eq!(s.language, d.language);
            assert_eq!(s.beam_size, 4);
        });
    }

    // -- lenient per-field parsing (#365) --

    /// A current settings file with a non-default value in every field.
    fn every_field_set() -> serde_json::Map<String, serde_json::Value> {
        match serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "language": "sv",
            "whisper_model": "kb-whisper-medium",
            "file_transcription_model": "kb-whisper-large",
            "pianissimo_dictation": false,
            "engine_prewarm": "off",
            "engine_idle_unload_minutes": 30,
            "hotkey_mode": "toggle",
            "show_overlay": false,
            "auto_paste": false,
            "auto_select_model": false,
            "hotkey": "Option+Space",
            "hotkey_profiles": [
                {"id": "default", "name": "Svenska", "shortcut": "Option+Space", "language": "sv"},
                {"id": "english", "name": "English", "shortcut": "Option+E", "language": "en"}
            ],
            "profile_models": {"default": "kb-whisper-medium", "english": "base.en"},
            "profile_glossary_migrated": true,
            "beam_size": 5,
            "temperature_fallback": false,
            "vad_enabled": true,
            "has_completed_onboarding": true,
            "model_lineup_version": MODEL_LINEUP_VERSION,
            "engine_defaults_version": ENGINE_DEFAULTS_VERSION,
            "future_key": {"keep": [1, 2]}
        }) {
            serde_json::Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    /// [`every_field_set`] with `field` replaced by `value`, as file bytes.
    fn every_field_set_but(field: &str, value: serde_json::Value) -> Vec<u8> {
        let mut map = every_field_set();
        map.insert(field.to_string(), value);
        serde_json::to_vec_pretty(&map).unwrap()
    }

    /// The settings [`every_field_set`] holds with `fields` at their defaults.
    fn every_field_set_without(fields: &[&str]) -> serde_json::Value {
        let mut map = every_field_set();
        for field in fields {
            map.remove(*field);
        }
        let settings: Settings = serde_json::from_value(serde_json::Value::Object(map)).unwrap();
        serde_json::to_value(settings).unwrap()
    }

    #[track_caller]
    fn assert_only_reset(loaded: &Settings, fields: &[&str]) {
        assert_eq!(
            serde_json::to_value(loaded).unwrap(),
            every_field_set_without(fields)
        );
    }

    #[test]
    fn unknown_whisper_model_resets_only_that_field_and_an_update_loses_nothing_else() {
        with_temp_settings(|path| {
            // A model removed from WhisperModel, or one a newer release added.
            let original = every_field_set_but("whisper_model", "kb-whisper-xl-2027".into());
            fs::write(&path, &original).unwrap();

            assert_only_reset(&load_from(&path), &["whisper_model"]);
            // Startup uses it too, but never rewrites a file it had to repair.
            assert_only_reset(&startup_load(&path), &["whisper_model"]);
            assert_file_bytes(&path, &original);
            assert!(!recovery_copy_path(&path, 0).exists());

            let updated = update_at(&path, |settings| settings.beam_size = 9).unwrap();
            assert_eq!(updated.beam_size, 9);
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let mut expected = every_field_set();
            expected.insert("whisper_model".into(), "base".into());
            expected.insert("beam_size".into(), 9.into());
            assert_eq!(raw, serde_json::Value::Object(expected));
            // The dropped value is recoverable from the exact original bytes.
            assert_file_bytes(recovery_copy_path(&path, 0), &original);
        });
    }

    #[test]
    fn unknown_language_variant_resets_only_the_language() {
        with_temp_settings(|path| {
            fs::write(&path, every_field_set_but("language", "de".into())).unwrap();
            let loaded = load_from(&path);
            assert_only_reset(&loaded, &["language"]);
            // The default profile keeps its own language.
            assert_eq!(loaded.default_profile().language, Language::Swedish);
        });
    }

    #[test]
    fn wrong_typed_fields_each_reset_alone() {
        with_temp_settings(|path| {
            let mut map = every_field_set();
            map.insert("show_overlay".into(), "no".into());
            map.insert("beam_size".into(), "five".into());
            fs::write(&path, serde_json::to_vec(&map).unwrap()).unwrap();
            assert_only_reset(&load_from(&path), &["beam_size", "show_overlay"]);
        });
    }

    #[test]
    fn an_unreadable_profile_is_dropped_alone_and_preserved_before_a_save() {
        with_temp_settings(|path| {
            // A language a newer release added, in the second of two profiles.
            let mut map = every_field_set();
            map["hotkey_profiles"][1]["language"] = "de".into();
            let original = serde_json::to_vec(&map).unwrap();
            fs::write(&path, &original).unwrap();

            let mut kept = every_field_set();
            kept["hotkey_profiles"] = serde_json::json!([kept["hotkey_profiles"][0].clone()]);
            let kept: Settings = serde_json::from_value(serde_json::Value::Object(kept)).unwrap();
            let kept = serde_json::to_value(kept).unwrap();
            assert_eq!(serde_json::to_value(load_from(&path)).unwrap(), kept);
            assert_eq!(serde_json::to_value(startup_load(&path)).unwrap(), kept);
            assert_file_bytes(&path, &original);

            update_at(&path, |settings| settings.beam_size = 9).unwrap();
            assert_file_bytes(recovery_copy_path(&path, 0), &original);
            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["hotkey_profiles"], kept["hotkey_profiles"]);
            assert_eq!(raw["beam_size"], 9);
        });
    }

    #[test]
    fn an_unreadable_primary_profile_is_repaired_not_dropped() {
        with_temp_settings(|path| {
            // `hotkey` and `language` mirror the primary profile, whose
            // language a newer release added.
            let mut map = every_field_set();
            map["hotkey_profiles"][0]["language"] = "tlh".into();
            let original = serde_json::to_vec(&map).unwrap();
            fs::write(&path, &original).unwrap();

            for loaded in [load_from(&path), startup_load(&path)] {
                // Only the unreadable language is reset, to `language`.
                assert_only_reset(&loaded, &[]);
                let bindings: Vec<String> = loaded
                    .resolved_hotkey_bindings()
                    .into_iter()
                    .map(|(_, shortcut, _)| shortcut)
                    .collect();
                assert_eq!(bindings, ["Option+Space", "Option+E"]);
                assert_eq!(loaded.default_profile().shortcut, loaded.hotkey);
            }
            assert_file_bytes(&path, &original);

            update_at(&path, |settings| settings.beam_size = 9).unwrap();
            assert_file_bytes(recovery_copy_path(&path, 0), &original);
        });
    }

    #[test]
    fn invalid_values_are_reported_by_field_name_only() {
        let map = match serde_json::from_slice(SCHEMA_INVALID_SETTINGS).unwrap() {
            serde_json::Value::Object(map) => map,
            _ => unreachable!(),
        };
        let problem = check_settings_object(map).unwrap_err().to_string();
        assert_eq!(problem, "invalid value for auto_paste");
        assert_eq!(
            invalid_fields(&["beam_size".into(), "language".into()]),
            "invalid values for beam_size, language"
        );
    }

    #[test]
    fn a_strictly_valid_file_loads_unchanged() {
        with_temp_settings(|path| {
            let bytes = serde_json::to_vec_pretty(&every_field_set()).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert_only_reset(&load_from(&path), &[]);
            assert_only_reset(&startup_load(&path), &[]);
            assert_file_bytes(&path, &bytes);
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

    // -- invalid settings files are preserved before any replacement (#340) --

    /// The `index`th recovery-copy name: `settings.json.bak`, then
    /// `settings.json.bak.1`, `settings.json.bak.2`, ...
    fn recovery_copy_path(path: &Path, index: usize) -> PathBuf {
        let first = path.with_extension("json.bak");
        if index == 0 {
            return first;
        }
        let mut name = first.into_os_string();
        name.push(format!(".{index}"));
        PathBuf::from(name)
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[track_caller]
    fn assert_file_bytes(path: impl AsRef<Path>, expected: &[u8]) {
        let path = path.as_ref();
        let actual = fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert!(
            actual == expected,
            "{} holds:\n{}\nexpected exactly:\n{}",
            path.display(),
            String::from_utf8_lossy(&actual),
            String::from_utf8_lossy(expected)
        );
    }

    const SCHEMA_INVALID_SETTINGS: &[u8] = br#"{
  "show_overlay": false,
  "language": "sv",
  "beam_size": 3,
  "future_key": {"keep": [1, 2]},
  "auto_paste": "invalid-bool"
}
"#;

    #[test]
    fn schema_error_preserves_original_bytes_before_field_update() {
        with_temp_settings(|path| {
            fs::write(&path, SCHEMA_INVALID_SETTINGS).unwrap();

            let updated = update_at(&path, |settings| settings.beam_size = 9).unwrap();
            assert_eq!(updated.beam_size, 9);

            let recovery = recovery_copy_path(&path, 0);
            assert!(
                recovery.exists(),
                "a schema-invalid settings file must be preserved before it is replaced"
            );
            assert_file_bytes(&recovery, SCHEMA_INVALID_SETTINGS);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&recovery).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }

            let raw: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(raw["beam_size"], 9);
            assert_eq!(raw["auto_paste"], true);
            assert_eq!(raw["future_key"], serde_json::json!({"keep": [1, 2]}));
            // A valid file is no longer invalid: the next save makes no copy.
            update_at(&path, |settings| settings.beam_size = 4).unwrap();
            assert!(!recovery_copy_path(&path, 1).exists());
        });
    }

    #[test]
    fn repeated_invalid_files_never_overwrite_an_earlier_recovery_copy() {
        with_temp_settings(|path| {
            let first_bad: &[u8] = br#"{"show_overlay": false, "language": "sv""#;
            let second_bad: &[u8] = br#"["not", "an", "object"]"#;
            let third_bad: &[u8] = b"{\"language\": \"sv\xff\"}";

            fs::write(&path, first_bad).unwrap();
            update_at(&path, |settings| settings.beam_size = 9).unwrap();
            assert_file_bytes(recovery_copy_path(&path, 0), first_bad);

            fs::write(&path, second_bad).unwrap();
            update_at(&path, |settings| settings.beam_size = 8).unwrap();
            // An earlier recovery copy must never be overwritten.
            assert_file_bytes(recovery_copy_path(&path, 0), first_bad);
            assert_file_bytes(recovery_copy_path(&path, 1), second_bad);

            // Bytes that are not UTF-8 are invalid settings, not a missing file.
            fs::write(&path, third_bad).unwrap();
            update_at(&path, |settings| settings.beam_size = 7).unwrap();
            assert_file_bytes(recovery_copy_path(&path, 2), third_bad);

            // Meeting the same invalid bytes again (for example on a retry)
            // reuses the existing copy instead of making a duplicate.
            fs::write(&path, first_bad).unwrap();
            update_at(&path, |settings| settings.beam_size = 6).unwrap();
            fs::write(&path, second_bad).unwrap();
            save_to(&path, &Settings::default()).unwrap();
            assert!(!recovery_copy_path(&path, 3).exists());
            assert_file_bytes(recovery_copy_path(&path, 0), first_bad);
            assert_file_bytes(recovery_copy_path(&path, 1), second_bad);
            assert_file_bytes(recovery_copy_path(&path, 2), third_bad);
            assert_eq!(load_from(&path).beam_size, Settings::default().beam_size);
        });
    }

    #[test]
    fn failed_recovery_copy_refuses_the_write() {
        with_temp_settings(|path| {
            let syntax_invalid: &[u8] = b"this is not json{{{";
            for original in [SCHEMA_INVALID_SETTINGS, syntax_invalid] {
                fs::write(&path, original).unwrap();
                let _failing = FailRecoveryCopy::enable();

                let error = update_at(&path, |settings| settings.beam_size = 9).unwrap_err();
                assert!(error.contains("refusing to overwrite"), "{error}");
                assert!(try_update_at(&path, |settings| {
                    settings.beam_size = 9;
                    Ok(())
                })
                .is_err());
                assert!(save_to(&path, &Settings::default()).is_err());

                assert_file_bytes(&path, original);
                assert!(!recovery_copy_path(&path, 0).exists());
                assert!(!global_glossary_path_for_settings(&path).exists());
            }
        });
    }

    #[test]
    fn unreadable_settings_file_is_never_overwritten() {
        // `chmod 000` cannot produce a read error for root (CAP_DAC_OVERRIDE),
        // so the portable case puts a directory at the settings path: reading
        // it fails with EISDIR on Unix and access denied on Windows, neither
        // of which is NotFound.
        with_temp_settings(|path| {
            let dir = path.parent().unwrap().to_path_buf();
            fs::create_dir(&path).unwrap();
            fs::write(path.join("keep"), b"original").unwrap();
            // Create the lock file up front so the listing only changes if
            // something is written.
            with_settings_lock(&path, || Ok(())).unwrap();
            let before = dir_names(&dir);

            let save_error = save_to(&path, &Settings::default()).unwrap_err();
            assert_eq!(dir_names(&dir), before, "nothing may be written");
            let update_error = update_at(&path, |settings| settings.beam_size = 9).unwrap_err();
            let try_update_error = try_update_at(&path, |settings| {
                settings.beam_size = 9;
                Ok(())
            })
            .unwrap_err();
            for error in [save_error, update_error, try_update_error] {
                assert!(
                    error.contains("could not be read") && error.contains("refusing to overwrite"),
                    "{error}"
                );
            }
            assert_eq!(dir_names(&dir), before, "nothing may be written");
            assert_eq!(fs::read(path.join("keep")).unwrap(), b"original");
        });

        // Where permissions do apply (not root), an unreadable regular file
        // is the real-world case: renaming over it would otherwise succeed.
        #[cfg(unix)]
        with_temp_settings(|path| {
            use std::os::unix::fs::PermissionsExt;
            fs::write(&path, br#"{"language":"sv"}"#).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::read(&path).is_ok() {
                return; // Running as root: permissions do not block reads.
            }
            let error = update_at(&path, |settings| settings.beam_size = 9).unwrap_err();
            assert!(error.contains("refusing to overwrite"), "{error}");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_file_bytes(&path, br#"{"language":"sv"}"#);
        });
    }

    #[test]
    fn startup_migrations_do_not_overwrite_an_invalid_file() {
        let schema_invalid: &[u8] = br#"{"model_lineup_version":0,"engine_defaults_version":0,"language":"sv","initial_prompt":"OpenRouter","show_overlay":false,"auto_paste":"invalid-bool","future_key":1}"#;
        let syntax_invalid: &[u8] = br#"{"model_lineup_version":0,"engine_defaults_version":0,"#;
        for original in [schema_invalid, syntax_invalid] {
            with_temp_settings(|path| {
                fs::write(&path, original).unwrap();
                let ready = |_: WhisperModel| true;
                let env = LineupEnv {
                    pianissimo_supported: true,
                    pianissimo_downloaded: true,
                    whisper_downloaded: &ready,
                };

                // What `load()` runs at startup. A schema error resets only
                // the bad field in memory; a syntax error loads defaults.
                let loaded = load_at_with_machine(&path, std::iter::empty(), &env);
                let language = if original == schema_invalid {
                    Language::Swedish
                } else {
                    Settings::default().language
                };
                assert_eq!(loaded.language, language);

                assert_file_bytes(&path, original);
                assert!(!recovery_copy_path(&path, 0).exists());
                assert!(!global_glossary_path_for_settings(&path).exists());
            });
        }
    }

    #[test]
    fn a_failing_migration_step_never_silently_replaces_the_file() {
        // Strictly valid, with the profile-models step (3) pending.
        let original: &[u8] =
            br#"{"language":"sv","beam_size":3,"show_overlay":false,"future_key":1}"#;
        for fallible in [false, true] {
            with_temp_settings(|path| {
                fs::write(&path, original).unwrap();
                let _failing = migrations::FailStep::enable(3);

                // The startup load uses defaults in memory and leaves the file alone.
                assert_eq!(startup_load(&path).beam_size, Settings::default().beam_size);
                assert_file_bytes(&path, original);
                assert!(!recovery_copy_path(&path, 0).exists());

                // A field update can only start from defaults, so it first
                // preserves the file, like one that does not parse (#340).
                let updated = if fallible {
                    try_update_at(&path, |settings| {
                        settings.beam_size = 9;
                        Ok(())
                    })
                } else {
                    update_at(&path, |settings| settings.beam_size = 9)
                }
                .unwrap();
                assert_eq!(updated.beam_size, 9);
                assert_file_bytes(recovery_copy_path(&path, 0), original);
                let raw: serde_json::Value =
                    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                assert_eq!(raw["beam_size"], 9);
                assert_eq!(raw["future_key"], 1);
            });
        }
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
                assert!(fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink());
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
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected",
                ))
            };
            let error =
                install_replacement(&path, dir.path(), b"new", "settings", &fail).unwrap_err();
            assert!(error.contains("preserve settings permissions"), "{error}");
            assert_eq!(fs::read(&path).unwrap(), b"original");
            assert_eq!(names(dir.path()), vec!["settings.json"]);
        }
    }

    #[cfg(windows)]
    #[test]
    fn blocked_replace_keeps_original_and_leaves_no_temp_or_backup() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"original").unwrap();
        // Open without FILE_SHARE_DELETE so the replace cannot move the target.
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(atomic_write(&path, b"new", "settings").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
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
        let icacls = |args: &[&str]| {
            let out = Command::new("icacls")
                .arg(&path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
        };
        icacls(&["/inheritance:r", "/grant:r", &format!("{user}:F")]);
        // Drop SYSTEM so the restricted ACL differs from an inherited one.
        icacls(&["/remove:g", "*S-1-5-18"]);
        let before = acl(&path);
        let fresh = dir.path().join("fresh.json");
        fs::write(&fresh, b"{}").unwrap();
        assert_ne!(
            before,
            acl(&fresh),
            "precondition: restricted ACL must differ from inherited"
        );
        fs::remove_file(&fresh).unwrap();

        atomic_write(&path, b"{\"a\":1}", "settings").unwrap();
        atomic_write(&path, b"{\"a\":2}", "settings").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"a":2}"#);
        assert_eq!(acl(&path), before);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }
}
