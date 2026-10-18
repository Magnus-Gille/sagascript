use std::{
    ffi::OsString,
    fs::OpenOptions,
    path::{Path, PathBuf},
};

use fs2::FileExt;

use serde_json::Value;

use crate::settings::{
    lenient,
    migrations::{self, Migration, Object},
    FileModelPreference, LineupEnv, MachineProbe, Settings, SettingsError, SCHEMA_VERSION,
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
pub fn profile_glossary_path(profile_id: &str) -> Result<PathBuf, SettingsError> {
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
fn copy_legacy_settings(source: &Path, destination: &Path) -> Result<bool, SettingsError> {
    if destination.exists() || !source.is_file() {
        return Ok(false);
    }
    let parent = destination
        .parent()
        .ok_or_else(|| SettingsError::storage("Settings destination has no parent directory"))?;
    create_private_dir_all(parent).map_err(|error| {
        SettingsError::storage(format!("Failed to create settings directory: {error}"))
    })?;
    let contents = std::fs::read_to_string(source).map_err(|error| {
        SettingsError::storage(format!("Failed to read legacy settings: {error}"))
    })?;
    let mut value: serde_json::Value = serde_json::from_str(&contents).map_err(|error| {
        SettingsError::storage(format!("Failed to parse legacy settings: {error}"))
    })?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| SettingsError::storage("Legacy settings root is not an object"))?;

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
    let migrated = serde_json::to_string_pretty(&value).map_err(|error| {
        SettingsError::storage(format!("Failed to serialize migrated settings: {error}"))
    })?;
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

/// Load settings from disk, migrating an older settings file (see
/// [`migrations`]). Returns defaults if the file is missing or unreadable.
/// Partial JSON files are handled by `#[serde(default)]` on Settings.
///
/// `machine` supplies what the load needs to know about the machine it runs on
/// (what is downloaded, whether Pianissimo runs): the store never probes it
/// itself, so settings do not depend on the transcription code (#372).
/// `crate::machine::load` is this with the probes of this machine.
pub(crate) fn load_with(machine: &dyn MachineProbe) -> Settings {
    load_at_with_machine(&settings_path(), legacy_settings_paths(), machine)
}

/// Test seam for `tests/settings_fixtures.rs`: [`load_with`] for an explicit
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
    .unwrap_or_else(|error: SettingsError| {
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
pub fn save(settings: &Settings) -> Result<(), SettingsError> {
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
///
/// `machine` is the injected machine state of [`load_with`];
/// `crate::machine::update` is this with the probes of this machine.
pub(crate) fn update_with<F>(
    machine: &dyn MachineProbe,
    mutate: F,
) -> Result<Settings, SettingsError>
where
    F: FnOnce(&mut Settings),
{
    try_update_with(machine, |settings| {
        mutate(settings);
        Ok(())
    })
}

/// Like [`update_with`], but lets validation fail while the cross-process lock
/// is held so read/validate/write profile mutations cannot race another writer.
/// `crate::machine::try_update` is this with the probes of this machine.
pub(crate) fn try_update_with<E, F>(machine: &dyn MachineProbe, mutate: F) -> Result<Settings, E>
where
    E: From<SettingsError>,
    F: FnOnce(&mut Settings) -> Result<(), E>,
{
    let path = settings_path();
    update_locked(&path, legacy_settings_paths(), machine, mutate)
}

// Shims (#372): the loads and updates that probe *this* machine moved to
// `crate::machine`, above settings and transcription, so this module no longer
// imports the transcription code to answer them. These re-exports keep the old
// paths `settings::store::{load, update, try_update}` working for the CLI and
// the app. They are the one upward import left in `settings/`, and a loop at
// run time: `transcription::pianissimo_backend::{engine_config, shared_client}`
// call `store::load()` themselves, i.e. this line, so transcription reaches
// `machine::load()` and `machine` reaches transcription's probes again.
//
// Removal prerequisites, in order (tests/module_layering.rs says the same):
//   1. the CLI and the app call `sagascript_core::machine::{load, update,
//      try_update}`;
//   2. `engine_config` and `shared_client` receive the `Settings` instead of
//      loading them. That changes when the settings are read, so it is a
//      behaviour change for its own change, not part of the layering move;
//   3. delete this line and empty the expected list in that test.
pub use crate::machine::{load, try_update, update};

/// A field update under the settings lock. The latest file is loaded as the
/// startup load does, with every pending migration step, machine state
/// included: the save records `schema_version` and the steps' markers, so a
/// step left pending here would run at the next start over the value the
/// user has just chosen. A file that cannot be loaded is updated from
/// defaults, so the save first preserves its bytes (see [`save_over`]).
fn update_locked<E, F>(
    path: &Path,
    sources: impl IntoIterator<Item = PathBuf>,
    machine: &dyn MachineProbe,
    mutate: F,
) -> Result<Settings, E>
where
    E: From<SettingsError>,
    F: FnOnce(&mut Settings) -> Result<(), E>,
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

fn with_settings_lock<T, E, F>(path: &Path, operation: F) -> Result<T, E>
where
    E: From<SettingsError>,
    F: FnOnce() -> Result<T, E>,
{
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    create_private_dir_all(&dir)
        .map_err(|e| SettingsError::storage(format!("Failed to create settings dir: {e}")))?;

    let lock_path = path.with_extension("json.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|e| SettingsError::storage(format!("Failed to open settings lock: {e}")))?;
    lock_file
        .lock_exclusive()
        .map_err(|e| SettingsError::storage(format!("Failed to lock settings: {e}")))?;

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
fn save_to(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
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
) -> Result<(), SettingsError> {
    let dir = path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Ensure directory exists
    create_private_dir_all(&dir)
        .map_err(|e| SettingsError::storage(format!("Failed to create settings dir: {e}")))?;

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
            // The copy is named, not located: the app scrubs a folder out of
            // an error's text (ADR 0006), and the copy lies next to the file.
            Err(error) => Err(SettingsError::storage(format!(
                "{error} (the invalid original settings file is preserved as {}, next to the settings file)",
                recovered
                    .copy
                    .file_name()
                    .map_or_else(|| "(unnamed)".into(), |name| name.to_string_lossy())
            ))),
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
) -> Result<(), SettingsError> {
    migrations::canonical_onboarding_key(map);

    save_external_glossaries(path, settings)?;

    // The personal dictionaries have human-editable files of their own.
    // Remove the legacy embedded fields after their external writes succeed.
    map.remove("initial_prompt");
    map.remove("profile_glossaries");

    // Merge settings fields into the map
    let settings_value = serde_json::to_value(settings)
        .map_err(|e| SettingsError::storage(format!("Serialize error: {e}")))?;
    if let serde_json::Value::Object(mut settings_map) = settings_value {
        settings_map.remove("initial_prompt");
        settings_map.remove("profile_glossaries");
        for (k, v) in settings_map {
            map.insert(k, v);
        }
    }

    let json = serde_json::to_string_pretty(&map)
        .map_err(|e| SettingsError::storage(format!("Serialize error: {e}")))?;

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
    SettingsError,
> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((serde_json::Map::new(), None))
        }
        Err(error) => {
            return Err(SettingsError::storage(format!(
                "Settings file {} could not be read ({error}); refusing to overwrite it",
                path.display()
            )))
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
        SettingsError::storage(format!(
            "Settings file {} is invalid ({problem}) and a recovery copy could not be saved ({error}); refusing to overwrite it",
            path.display()
        ))
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
fn preserve_invalid_settings(path: &Path, contents: &[u8]) -> Result<PathBuf, SettingsError> {
    #[cfg(test)]
    if tests::FAIL_RECOVERY_COPY.with(std::cell::Cell::get) {
        return Err(SettingsError::storage("injected recovery copy failure"));
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
            Err(error) => {
                return Err(SettingsError::storage(format!(
                    "Failed to create {}: {error}",
                    candidate.display()
                )))
            }
        }
    }
    Err(SettingsError::storage(format!(
        "{MAX_RECOVERY_COPIES} recovery copies of {} already exist",
        path.display()
    )))
}

fn write_recovery_copy(
    mut file: std::fs::File,
    candidate: &Path,
    dir: &Path,
    contents: &[u8],
) -> Result<(), SettingsError> {
    use std::io::Write;

    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
        drop(file);
        // Created exclusively above, so this incomplete copy is ours to remove.
        let _ = std::fs::remove_file(candidate);
        return Err(SettingsError::storage(format!(
            "Failed to write {}: {error}",
            candidate.display()
        )));
    }
    sync_directory(dir).map_err(|error| {
        SettingsError::storage(format!(
            "Saved {} but failed to sync its directory: {error}",
            candidate.display()
        ))
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

pub(super) fn validate_profile_file_id(profile_id: &str) -> Result<(), SettingsError> {
    if profile_id.is_empty()
        || profile_id.len() > 32
        || !profile_id.starts_with(|character: char| character.is_ascii_alphanumeric())
        || !profile_id.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '_')
        })
    {
        return Err(SettingsError::InvalidProfileFileId {
            id: profile_id.to_string(),
        });
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

fn save_external_glossaries(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
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

fn atomic_write_if_changed(path: &Path, contents: &[u8], label: &str) -> Result<(), SettingsError> {
    if std::fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    atomic_write(path, contents, label)
}

/// Atomically replace a regular file while preserving a user-managed symlink.
fn atomic_write(path: &Path, contents: &[u8], label: &str) -> Result<(), SettingsError> {
    let destination = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path).map_err(|error| {
                SettingsError::storage(format!("Failed to resolve {label} symlink: {error}"))
            })?;
            let target = if target.is_absolute() {
                target
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(target)
            };
            if target.exists() {
                std::fs::canonicalize(&target).map_err(|error| {
                    SettingsError::storage(format!(
                        "Failed to canonicalize {label} symlink target: {error}"
                    ))
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
    create_private_dir_all(&parent).map_err(|error| {
        SettingsError::storage(format!("Failed to create {label} directory: {error}"))
    })?;
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
) -> Result<(), SettingsError> {
    use std::io::Write;

    // Exclusive, uniquely named, owner-only (0600 on Unix) temporary file in the
    // destination directory; removed automatically if anything below fails.
    let mut tmp = tempfile::Builder::new()
        .prefix(".sagascript-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| {
            SettingsError::storage(format!("Failed to create {label} temporary file: {error}"))
        })?;
    // Distinguish an absent target from an unreadable one; fail closed on the latter.
    let existing = match std::fs::metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(SettingsError::storage(format!(
                "Failed to inspect existing {label}: {error}"
            )));
        }
    };
    tmp.write_all(contents)
        .map_err(|error| SettingsError::storage(format!("Failed to write {label}: {error}")))?;
    // Keep an existing target's permissions (e.g. a deliberately restrictive or
    // shared mode); a new file stays owner-only. Applied before the final fsync.
    if let Some(existing) = existing.as_ref().filter(|metadata| metadata.is_file()) {
        copy_permissions(tmp.as_file(), existing).map_err(|error| {
            SettingsError::storage(format!("Failed to preserve {label} permissions: {error}"))
        })?;
    }
    tmp.as_file().sync_all().map_err(|error| {
        SettingsError::storage(format!("Failed to sync {label} before install: {error}"))
    })?;
    install_temp_file(tmp, destination, existing.is_some(), label)?;
    sync_directory(parent).map_err(|error| {
        SettingsError::storage(format!(
            "Installed {label} but failed to sync its directory: {error}"
        ))
    })
}

/// Unix: rename(2) replaces atomically.
#[cfg(unix)]
fn install_temp_file(
    tmp: tempfile::NamedTempFile,
    destination: &Path,
    _target_exists: bool,
    label: &str,
) -> Result<(), SettingsError> {
    tmp.persist(destination).map(|_| ()).map_err(|error| {
        SettingsError::storage(format!("Failed to install {label}: {}", error.error))
    })
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
) -> Result<(), SettingsError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !target_exists {
        return tmp.persist(destination).map(|_| ()).map_err(|error| {
            SettingsError::storage(format!("Failed to install {label}: {}", error.error))
        });
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
            .map_err(|error| {
                SettingsError::storage(format!("Failed to reserve {label} backup name: {error}"))
            })?
            .into_temp_path();
        let path = reserved.to_path_buf();
        reserved.close().map_err(|error| {
            SettingsError::storage(format!("Failed to reserve {label} backup name: {error}"))
        })?;
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
            return Err(SettingsError::storage(format!(
                "Failed to install {label}: {os_error}; restoring the original from {} also failed ({restore}); new contents kept at {}",
                backup.display(),
                kept.map_or_else(|| "(unknown)".to_string(), |p| p.display().to_string())
            )));
        }
    }
    // Never let cleanup delete the only remaining copy.
    if !destination.exists() {
        let kept = temp_path.keep().ok();
        return Err(SettingsError::storage(format!(
            "Failed to install {label}: {os_error}; the original is missing, new contents kept at {}",
            kept.map_or_else(|| "(unknown)".to_string(), |p| p.display().to_string())
        )));
    }
    let _ = std::fs::remove_file(&backup);
    Err(SettingsError::storage(format!(
        "Failed to install {label}: {os_error}"
    )))
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
#[path = "store_tests.rs"]
mod tests;
