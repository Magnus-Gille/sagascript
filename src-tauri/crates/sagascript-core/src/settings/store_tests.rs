use super::*;
use crate::settings::{
    EnginePrewarm, FileModelPreference, HotkeyMode, Language, WhisperModel,
    ENGINE_DEFAULTS_VERSION, MODEL_LINEUP_VERSION,
};
use sagascript_testsupport::{fake_settings, TempDir};
use std::fs;
use std::sync::mpsc;
use std::thread;

fn every_model_ready(_: WhisperModel) -> bool {
    true
}

/// Pianissimo supported and every model downloaded.
fn ready_machine() -> LineupEnv<'static> {
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

/// [`try_update`] of `path` (no legacy locations) on a machine with
/// Pianissimo supported and every model downloaded.
#[cfg(test)]
fn try_update_at<F>(path: &Path, mutate: F) -> Result<Settings, String>
where
    F: FnOnce(&mut Settings) -> Result<(), String>,
{
    update_locked(path, std::iter::empty(), &ready_machine(), mutate)
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

#[cfg(test)]
thread_local! {
    pub(super) static FAIL_RECOVERY_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

/// Runs `f` on a settings file of its own ([`fake_settings`]), whose
/// directory is removed afterwards, also when `f` panics.
fn with_temp_settings<F: FnOnce(PathBuf)>(f: F) {
    let settings = fake_settings();
    f(settings.path().to_path_buf());
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
    let root = TempDir::new();
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
    let root = TempDir::new();
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
    let root = TempDir::new();
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
    let root = TempDir::new();
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
fn testsupport_settings_file_name_matches_the_store() {
    assert_eq!(
        sagascript_testsupport::SETTINGS_FILE_NAME,
        SETTINGS_FILENAME
    );
}

#[test]
fn migration_copies_legacy_settings_without_overwriting_destination() {
    let root = TempDir::new();
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
}

#[test]
fn migration_disables_previously_enabled_auto_paste() {
    let root = TempDir::new();
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
}

#[test]
fn xdg_move_preserves_auto_paste_for_the_same_bundle_identity() {
    let root = TempDir::new();
    let legacy = root.join(APP_IDENTIFIER).join(SETTINGS_FILENAME);
    let destination = root.join("xdg").join(SETTINGS_FILENAME);
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, r#"{"auto_paste":true}"#).unwrap();

    assert!(copy_legacy_settings(&legacy, &destination).unwrap());
    let migrated: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&destination).unwrap()).unwrap();
    assert_eq!(migrated["auto_paste"], true);
}

#[test]
fn first_xdg_load_copies_settings_and_externalizes_embedded_glossaries() {
    let root = TempDir::new();
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
}

#[test]
fn embedded_invalid_profile_dictionary_id_is_skipped_during_externalization() {
    let root = TempDir::new();
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
}

#[cfg(unix)]
#[test]
fn migration_writes_through_a_dangling_settings_symlink() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new();
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
}

#[test]
fn concurrent_first_run_update_waits_for_atomic_migration() {
    let root = TempDir::new();
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
            migrate_legacy_identifier_settings_locked(&migration_destination, [migration_legacy]);
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
}

#[test]
fn load_returns_defaults_when_file_missing() {
    let root = TempDir::new();
    let nonexistent = root.join("missing").join(SETTINGS_FILENAME);
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

    // Two missing levels, as when the root did not exist either: the save
    // must create the whole chain, not just the last directory.
    let root = TempDir::new();
    let path = root.join("outer").join("config").join(SETTINGS_FILENAME);
    save_to(&path, &Settings::default()).unwrap();
    let mode = fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[cfg(unix)]
#[test]
fn unchanged_glossary_save_does_not_replace_the_file() {
    use std::os::unix::fs::MetadataExt;

    let root = TempDir::new();
    let settings_path = root.join(SETTINGS_FILENAME);
    let glossary_path = root.join(GLOBAL_GLOSSARY_FILENAME);
    fs::write(&glossary_path, "OpenRouter = open router\n").unwrap();
    let original_inode = fs::metadata(&glossary_path).unwrap().ino();
    let settings = Settings {
        initial_prompt: "OpenRouter = open router".to_string(),
        ..Default::default()
    };

    save_to(&settings_path, &settings).unwrap();

    assert_eq!(fs::metadata(&glossary_path).unwrap().ino(), original_inode);
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

/// #458: the copy `update_locked` returns is what a whole-copy setter puts in
/// the controller. On a machine that cannot run Pianissimo it must be demoted
/// like a load, while the file keeps the user's choice.
#[test]
fn update_returns_a_copy_demoted_for_an_unsupported_machine() {
    with_temp_settings(|path| {
        fs::write(
            &path,
            r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"model_lineup_version":1,"engine_defaults_version":1,"schema_version":6,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"pianissimo-sv"},"file_transcription_model":"pianissimo-sv","pianissimo_dictation":true}"#,
        )
        .unwrap();
        let all_downloaded = |_: WhisperModel| true;
        let unsupported = LineupEnv {
            pianissimo_supported: false,
            pianissimo_downloaded: false,
            whisper_downloaded: &all_downloaded,
        };
        let updated = update_locked(&path, std::iter::empty(), &unsupported, |settings| {
            settings.beam_size = 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            updated.profile_models["default"],
            FileModelPreference::Auto,
            "the returned copy must not select an engine this machine cannot run"
        );
        assert_eq!(updated.file_transcription_model, FileModelPreference::Auto);
        assert!(!updated.pianissimo_dictation);
        // The file keeps the user's choice, as a load leaves it.
        let on_disk: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk["profile_models"]["default"], "pianissimo-sv");
        assert_eq!(on_disk["file_transcription_model"], "pianissimo-sv");
        assert_eq!(on_disk["pianissimo_dictation"], true);
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
    let actual =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
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
    let original: &[u8] = br#"{"language":"sv","beam_size":3,"show_overlay":false,"future_key":1}"#;
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
        let error = install_replacement(&path, dir.path(), b"new", "settings", &fail).unwrap_err();
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
