//! CLI side of the CLI/app parity table (#373): every row of
//! `sagascript_core::test_support::settings_ops_table` is run through the real
//! `sagascript config` against a settings file of its own, and the persisted
//! settings are compared with what the shared core operation leaves.
//!
//! The app runs the same table through its setters (`commands/persist_parity_tests.rs`
//! in the app crate), so the two surfaces are equal to each other through core
//! without the app depending on this crate. Settings are the user's state: each
//! row has its own temporary file and directory, set on the child's environment
//! only (ADR 0004, rule 5).

use std::process::Command;

use sagascript_core::settings::store;
use sagascript_core::settings::SCHEMA_VERSION;
use sagascript_core::test_support::settings_ops_table::{
    assert_persisted, cases, differing_fields, Case, Op, ProfileEdit, Surface,
};
use sagascript_testsupport::{fake_settings, isolated_cli_env};

/// The arguments of `sagascript` that mean `op`.
fn args(op: &Op) -> Vec<String> {
    let set =
        |key: &str, value: String| vec!["config".to_string(), "set".into(), key.to_string(), value];
    match op {
        Op::Language(language) => set("language", language.id().into()),
        Op::WhisperModel(model) => set("whisper_model", model.id().into()),
        Op::AutoSelectModel(enabled) => set("auto_select_model", enabled.to_string()),
        Op::PianissimoDictation(enabled) => set("pianissimo_dictation", enabled.to_string()),
        Op::AutoPaste { enabled, .. } => set("auto_paste", enabled.to_string()),
        Op::VadEnabled(enabled) => set("vad_enabled", enabled.to_string()),
        Op::ShowOverlay(enabled) => set("show_overlay", enabled.to_string()),
        Op::BeamSize(beam) => set("beam_size", beam.to_string()),
        Op::TemperatureFallback(enabled) => set("temperature_fallback", enabled.to_string()),
        Op::EnginePrewarm(mode) => set("engine_prewarm", mode.id().into()),
        Op::EngineIdleUnloadMinutes(minutes) => {
            set("engine_idle_unload_minutes", minutes.to_string())
        }
        Op::FileTranscriptionModel(choice) => set("file_transcription_model", choice.id().into()),
        Op::HotkeyMode(mode) => set("hotkey_mode", mode.id().into()),
        Op::Hotkey(shortcut) => set("hotkey", (*shortcut).into()),
        Op::Profiles(edit) => {
            let mut args: Vec<String> = ["config", "profiles"].map(String::from).into();
            let mut flag = |name: &str, value: &str| {
                args.push(format!("--{name}"));
                args.push(value.to_string());
            };
            match edit {
                ProfileEdit::Create {
                    id,
                    name,
                    shortcut,
                    language,
                    model,
                } => {
                    flag("name", name);
                    flag("hotkey", shortcut);
                    flag("language", language.id());
                    if let Some(model) = model {
                        flag("model", model.id());
                    }
                    args.splice(2..2, ["create".to_string(), (*id).to_string()]);
                }
                ProfileEdit::Update {
                    id,
                    shortcut,
                    language,
                    model,
                } => {
                    if let Some(shortcut) = shortcut {
                        flag("hotkey", shortcut);
                    }
                    if let Some(language) = language {
                        flag("language", language.id());
                    }
                    if let Some(model) = model {
                        flag("model", model.id());
                    }
                    args.splice(2..2, ["update".to_string(), (*id).to_string()]);
                }
                ProfileEdit::Remove { id } => {
                    args.extend(["remove".to_string(), (*id).to_string()]);
                }
            }
            args
        }
        Op::ProfileModel { id, model } => {
            ["config", "profiles", "update", id, "--model", model.id()]
                .map(String::from)
                .into()
        }
        Op::DictionaryAdd {
            profile,
            term,
            aliases,
        } => {
            let mut args: Vec<String> = ["glossary", "add", term, "--profile", profile]
                .map(String::from)
                .into();
            for alias in *aliases {
                args.extend(["--alias".to_string(), alias.to_string()]);
            }
            args
        }
        Op::InitialPrompt(text) => set("initial_prompt", (*text).into()),
        Op::Reset(key) => ["config", "reset", key].map(String::from).into(),
    }
}

/// Run one row; returns the CLI's verdict.
fn run(case: &Case) {
    let settings = fake_settings();
    std::fs::write(settings.path(), case.file_text()).expect("write the starting settings");
    let before = store::load_from(settings.path());

    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    isolated_cli_env(&mut command, settings.path());
    // Every user directory points into the temp dir as well, so nothing
    // outside it can be read or written.
    let home = settings.dir().join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ] {
        command.env(key, &home);
    }
    let output = command
        .args(args(&case.op))
        .output()
        .expect("run the sagascript CLI");
    let ran = if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    };
    let after = store::load_from(settings.path());
    assert_persisted(case, Surface::Cli, ran, &before, &after);
}

#[test]
fn every_operation_persists_what_the_shared_operation_leaves() {
    let table = cases();
    assert!(table.len() > 40, "the table is {} rows", table.len());
    for case in &table {
        run(case);
    }
}

/// A row that starts from the file of an older release is only worth having if
/// the file has steps pending and loads as the settings the table starts from:
/// the surface runs the steps when it writes, and ends where the operation on
/// those settings ends.
#[test]
fn a_legacy_file_has_steps_pending_and_loads_as_its_start() {
    let mut files = 0;
    for case in cases() {
        let Some(text) = &case.legacy_file else {
            continue;
        };
        files += 1;
        let raw: serde_json::Value = serde_json::from_str(text).expect("the file is JSON");
        let pending_profile_models = raw["profile_models"] == serde_json::json!({});
        let pending_dictionaries =
            raw["profile_glossary_migrated"] == serde_json::Value::Bool(false);
        let newer = raw["schema_version"]
            .as_u64()
            .is_some_and(|version| version > u64::from(SCHEMA_VERSION));
        assert!(
            pending_profile_models || pending_dictionaries || newer,
            "{}: nothing is pending in {text}",
            case.name
        );

        let settings = fake_settings();
        std::fs::write(settings.path(), text).expect("write the legacy file");
        let loaded = store::load_from(settings.path());
        assert_eq!(
            differing_fields(&loaded, &case.start),
            Vec::<String>::new(),
            "{}: the file does not load as the table's start",
            case.name
        );
        assert_eq!(loaded.schema_version, SCHEMA_VERSION, "{}", case.name);
    }
    assert!(files >= 3, "the table has {files} legacy files");
}
