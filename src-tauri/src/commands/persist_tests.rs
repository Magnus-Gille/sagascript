//! Tests of `persist_and_sync` (#370): after a setter, the settings file and the
//! controller agree, in the way that setter always had.
//!
//! There are two ways a setter keeps the controller in step, and the table
//! pins which one each uses ([`Mirror`]). Ten setters mirror only the field they
//! change: the controller can be on shortcuts the OS has registered while the
//! file holds ones it could not register (the settings watcher keeps it there
//! on purpose), and an unrelated setter must not move it. The others replace the
//! whole copy, because they act on the settings they have just applied.
//!
//! Settings are global user state, and the store reads its location from
//! `SAGASCRIPT_SETTINGS_PATH`. ADR 0004 (rule 5) forbids setting that in a test
//! process with other tests running, so each test runs in a child of this test
//! binary whose environment points at a fake settings file
//! (`sagascript_testsupport::{fake_settings, isolated_cli_env}`), see
//! [`isolated`]. The parent checks the child's report; a filter that matches
//! nothing exits with success too.
//!
//! Each table row calls the real setter (a `State` from Tauri's mock runtime),
//! after the file was changed behind the controller's back the way the CLI does.

use std::collections::BTreeSet;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use sagascript_core::settings::{
    store, EnginePrewarm, FileModelPreference, HotkeyMode, Language, Settings, WhisperModel,
};
use sagascript_core::transcription::{suggest_glossary_candidates, Glossary};
use sagascript_testsupport::{fake_settings, isolated_cli_env};
use tauri::test::MockRuntime;
use tauri::Manager;

use super::*;
use crate::app_controller::AppController;
use crate::commands::*;
use crate::contract;
use crate::hotkey::configuration::HotkeyChange;

type App = tauri::App<MockRuntime>;

/// Set in the child process only.
const CHILD: &str = "SAGASCRIPT_PERSIST_TABLE_CHILD";

/// How the setter keeps the controller's copy in step with the file.
enum Mirror {
    /// Only the named settings fields (JSON keys): they must equal the file
    /// after the setter; every other field of the controller stays as it was,
    /// even if the file moved on behind it. The ten setters that mirrored a
    /// single field before `persist_and_sync` keep doing that.
    Field(&'static [&'static str]),
    /// The controller's whole copy is replaced by the file's: the setters that
    /// did that before `persist_and_sync`, because the controller adopts the
    /// settings they have just applied (a language change, a profile's model, a
    /// dictionary, shortcuts the OS has just registered).
    Whole,
}

struct Row {
    /// The commands the row stands for.
    commands: &'static [&'static str],
    /// What the setter does to the controller's copy.
    mirror: Mirror,
    /// Brings the file into a state in which the setter has something to change.
    arrange: fn(),
    /// Calls the setter the way the frontend would.
    run: fn(&App, &tokio::runtime::Runtime) -> Result<(), String>,
    /// Whether the value the setter was given is in the settings.
    landed: fn(&Settings) -> bool,
}

fn nothing() {}

fn model_id(model: WhisperModel) -> String {
    serde_json::to_value(model)
        .and_then(serde_json::from_value::<String>)
        .expect("a Whisper model has a stable id")
}

fn text(error: impl ToString) -> String {
    error.to_string()
}

fn controller(app: &App) -> tauri::State<'_, SharedController> {
    app.state::<SharedController>()
}

/// A hotkey change as `apply_hotkey_change` persists it, after the OS accepted
/// the shortcuts (registering them needs a real app handle).
fn persist_hotkey(app: &App, change: HotkeyChange) -> Result<(), String> {
    let (candidate, _notices) = change.prepare_with_notices(&store::load())?;
    persist_hotkey_change(&controller(app), &change, &candidate.resolved_shortcuts())
        .map(drop)
        .map_err(text)
}

fn other_prewarm() -> EnginePrewarm {
    EnginePrewarm::ALL
        .into_iter()
        .find(|mode| *mode != Settings::default().engine_prewarm)
        .expect("more than one prewarm mode")
}

/// One row per settings-writing command. A new one needs a row here
/// (`every_settings_writing_command_has_a_row`).
///
/// The rows use Norwegian and its models. Pianissimo is Swedish-only, so no
/// row can resolve a profile to it, and a profile change cannot start the
/// engine host on a machine that has the Pianissimo model installed.
const ROWS: &[Row] = &[
    Row {
        commands: &["set_language"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, _| persist_language(&controller(app), Language::Norwegian).map(drop),
        landed: |s| s.default_profile().language == Language::Norwegian,
    },
    Row {
        commands: &["set_onboarding_completed"],
        mirror: Mirror::Field(&["has_completed_onboarding"]),
        arrange: nothing,
        run: |app, rt| rt.block_on(set_onboarding_completed(controller(app))),
        landed: |s| s.has_completed_onboarding,
    },
    Row {
        commands: &["set_file_transcription_model"],
        mirror: Mirror::Field(&["file_transcription_model"]),
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_file_transcription_model(
                controller(app),
                model_id(WhisperModel::NbWhisperSmall),
            ))
        },
        landed: |s| {
            s.file_transcription_model == FileModelPreference::Whisper(WhisperModel::NbWhisperSmall)
        },
    },
    Row {
        commands: &["set_profile_model"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_profile_model(
                controller(app),
                "default".into(),
                model_id(WhisperModel::NbWhisperMedium),
            ))
        },
        landed: |s| {
            s.profile_models.get("default")
                == Some(&FileModelPreference::Whisper(WhisperModel::NbWhisperMedium))
        },
    },
    Row {
        commands: &["set_whisper_model"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_whisper_model(
                controller(app),
                WhisperModel::NbWhisperBase,
            ))
        },
        landed: |s| {
            s.profile_models.get("default")
                == Some(&FileModelPreference::Whisper(WhisperModel::NbWhisperBase))
        },
    },
    Row {
        commands: &["set_pianissimo_dictation"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, rt| rt.block_on(set_pianissimo_dictation(controller(app), false)),
        landed: |s| s.profile_models.get("default") == Some(&FileModelPreference::Auto),
    },
    Row {
        commands: &["set_auto_select_model"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, rt| rt.block_on(set_auto_select_model(controller(app), false)),
        landed: |s| {
            s.profile_models.get("default") == Some(&FileModelPreference::Whisper(s.whisper_model))
        },
    },
    Row {
        commands: &["set_auto_paste"],
        mirror: Mirror::Field(&["auto_paste"]),
        arrange: nothing,
        // Disabling needs no Accessibility permission, so this row is the same
        // on every platform.
        run: |app, rt| rt.block_on(set_auto_paste(controller(app), false)),
        landed: |s| !s.auto_paste,
    },
    Row {
        commands: &["set_show_overlay"],
        mirror: Mirror::Field(&["show_overlay"]),
        arrange: nothing,
        run: |app, _| persist_show_overlay(&controller(app), false).map(drop),
        landed: |s| !s.show_overlay,
    },
    Row {
        commands: &["set_initial_prompt", "set_profile_glossary"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_initial_prompt(
                controller(app),
                "Astrid = astrid".into(),
                None,
            ))
            .map_err(text)
        },
        landed: |s| {
            s.profile_glossaries.get("default").map(String::as_str) == Some("Astrid = astrid")
        },
    },
    Row {
        commands: &["apply_training_glossary"],
        mirror: Mirror::Field(&["profile_glossaries"]),
        arrange: nothing,
        run: |app, rt| {
            let heard = "Jag heter Magnus Jille.";
            let corrected = "Jag heter Magnus Gille.";
            let accepted = suggest_glossary_candidates(heard, corrected, &Glossary::default());
            rt.block_on(apply_training_glossary(
                controller(app),
                heard.into(),
                corrected.into(),
                "default".into(),
                accepted,
            ))
            .map_err(text)
        },
        landed: |s| {
            s.profile_glossaries
                .get("default")
                .is_some_and(|source| source.contains("Gille = Jille"))
        },
    },
    Row {
        commands: &["set_beam_size"],
        mirror: Mirror::Field(&["beam_size"]),
        arrange: nothing,
        run: |app, rt| rt.block_on(set_beam_size(controller(app), 7)),
        landed: |s| s.beam_size == 7,
    },
    Row {
        commands: &["set_temperature_fallback"],
        mirror: Mirror::Field(&["temperature_fallback"]),
        arrange: nothing,
        run: |app, rt| rt.block_on(set_temperature_fallback(controller(app), false)),
        landed: |s| !s.temperature_fallback,
    },
    Row {
        commands: &["set_vad_enabled"],
        mirror: Mirror::Field(&["vad_enabled"]),
        // Disabling is the direction that downloads nothing; the file starts
        // with it on so the setter has a value to change.
        arrange: || {
            store::update(|s| s.vad_enabled = true).expect("the fake settings file is writable");
        },
        run: |app, rt| rt.block_on(set_vad_enabled(controller(app), false)),
        landed: |s| !s.vad_enabled,
    },
    Row {
        commands: &["set_engine_prewarm"],
        mirror: Mirror::Field(&["engine_prewarm"]),
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_engine_prewarm(
                controller(app),
                other_prewarm().id().into(),
            ))
        },
        landed: |s| s.engine_prewarm == other_prewarm(),
    },
    Row {
        commands: &["set_engine_idle_unload_minutes"],
        mirror: Mirror::Field(&["engine_idle_unload_minutes"]),
        arrange: nothing,
        run: |app, rt| rt.block_on(set_engine_idle_unload_minutes(controller(app), 11)),
        landed: |s| s.engine_idle_unload_minutes == 11,
    },
    Row {
        commands: &["set_hotkey_mode"],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, _| persist_hotkey(app, HotkeyChange::Mode(HotkeyMode::Toggle)),
        landed: |s| s.hotkey_mode == HotkeyMode::Toggle,
    },
    Row {
        commands: &[
            "set_hotkey",
            "set_hotkey_profiles",
            "retry_hotkey_registration",
        ],
        mirror: Mirror::Whole,
        arrange: nothing,
        run: |app, _| {
            let mut profiles = store::load().resolved_hotkey_profiles();
            profiles[0].set_primary_shortcut("Control+Alt+D".into());
            persist_hotkey(app, HotkeyChange::Profiles(profiles))
        },
        landed: |s| s.hotkey == "Control+Alt+D",
    },
];

fn as_json(settings: &Settings) -> serde_json::Value {
    serde_json::to_value(settings).expect("settings serialize")
}

/// The file the process environment names, which must be a fake one.
fn fake_file() -> std::path::PathBuf {
    assert!(
        store::settings_path_is_overridden(),
        "this must run on a fake settings file, never the user's"
    );
    store::settings_path()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

/// A mock app that manages a controller holding `memory`.
fn app_with(memory: Settings) -> App {
    let app = tauri::test::mock_app();
    app.manage::<SharedController>(Mutex::new(AppController::new(memory)));
    app
}

/// Runs `body` on a fake settings file in a child process of this test binary
/// and returns what it reported, in the parent. In the child (`CHILD` is set)
/// it runs `body` in place and returns `None`.
///
/// The child is this same test, selected with `--exact`, with the environment
/// reduced to the ordinary OS variables plus `SAGASCRIPT_SETTINGS_PATH`
/// pointing at a fake file. The parent fails when the child fails. The body's
/// report is passed back in a file next to the settings, so a child that ran
/// no test (a filter that matched nothing succeeds) cannot pass for one that
/// did.
fn isolated(module: &str, test: &str, body: impl FnOnce() -> String) -> Option<String> {
    if std::env::var_os(CHILD).is_some() {
        let report = body();
        let file = fake_file();
        let dir = file.parent().expect("the settings file has a directory");
        std::fs::write(dir.join(format!("{test}.report")), report).expect("the report is writable");
        return None;
    }
    let fake = fake_settings();
    // `module` is the caller's `module_path!()`; the test filter has no crate name.
    let path = module
        .split_once("::")
        .map(|(_, path)| format!("{path}::{test}"))
        .expect("a module path below the crate");
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    isolated_cli_env(&mut command, fake.path())
        .env(CHILD, "1")
        .args(["--exact", &path, "--test-threads=1", "--nocapture"]);
    let output = command.output().expect("the test binary starts");
    assert!(
        output.status.success(),
        "{test} failed in the child:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = std::fs::read_to_string(fake.dir().join(format!("{test}.report")))
        .unwrap_or_else(|_| panic!("the child did not run {test}"));
    Some(report)
}

/// Moves the file ahead of the controller by `value` in a settings field the
/// row's setter does not own, and returns that field's JSON key. Two fields
/// take turns, so the idle-unload setter's row has a canary of its own too: a
/// setter that wrongly replaced the whole copy would show in it.
fn move_the_file_ahead(row: &Row, value: u32) -> Result<&'static str, String> {
    if row.commands.contains(&"set_engine_idle_unload_minutes") {
        store::update(|s| s.beam_size = value)?;
        Ok("beam_size")
    } else {
        store::update(|s| s.engine_idle_unload_minutes = value)?;
        Ok("engine_idle_unload_minutes")
    }
}

/// The child's work for the table: every row against the fake settings file.
fn run_table() -> String {
    let settings_file = fake_file();
    let runtime = runtime();
    let app = app_with(Settings::default());

    let mut ran = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = row.commands.join("+");
        // The file moves on without the controller (a CLI call, a watcher that
        // has not fired yet), in a field the row's setter does not own.
        let canary = move_the_file_ahead(row, 40 + index as u32)
            .unwrap_or_else(|error| panic!("{name}: cannot write the fake file: {error}"));
        (row.arrange)();
        let before = as_json(&lock_controller(&controller(&app)).settings().clone());

        (row.run)(&app, &runtime)
            .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

        let on_disk = store::load_from(&settings_file);
        let file = as_json(&on_disk);
        let after = as_json(&lock_controller(&controller(&app)).settings().clone());
        assert!(
            (row.landed)(&on_disk),
            "{name}: the new value is not in the settings"
        );
        match &row.mirror {
            Mirror::Whole => assert_eq!(
                after, file,
                "{name}: the controller and the file differ after the setter"
            ),
            Mirror::Field(own) => {
                assert_ne!(
                    before[canary], file[canary],
                    "{name}: the harness did not move the file ahead of the controller"
                );
                let fields = after.as_object().expect("settings are an object");
                for (key, value) in fields {
                    let expected = if own.contains(&key.as_str()) {
                        &file[key]
                    } else {
                        &before[key]
                    };
                    assert_eq!(
                        value, expected,
                        "{name}: `{key}` of the controller is not what a setter of {own:?} leaves"
                    );
                }
            }
        }
        ran.push(name);
    }
    ran.join("\n")
}

#[test]
fn every_setter_syncs_the_controller_the_way_it_always_did() {
    let Some(report) = isolated(
        module_path!(),
        "every_setter_syncs_the_controller_the_way_it_always_did",
        run_table,
    ) else {
        return;
    };
    let expected: Vec<String> = ROWS.iter().map(|row| row.commands.join("+")).collect();
    assert_eq!(
        report.lines().map(str::to_owned).collect::<Vec<_>>(),
        expected
    );
}

/// The writers come from the sources, not from a naming convention: a command
/// is a settings writer when it reaches `persist_and_sync` or a sibling,
/// directly or through a helper, a method or another command anywhere under
/// `src/`, through a renamed import, or from inside a macro. `apply_*` or
/// `toggle_*` writers are found as well as `set_*` ones. The scan's limits are
/// listed in contract/writers.rs and in the add-setting recipe.
#[test]
fn every_settings_writing_command_has_a_row() {
    let covered: BTreeSet<&str> = ROWS
        .iter()
        .flat_map(|row| row.commands.iter().copied())
        .collect();
    let writers = contract::settings_writers().commands;
    assert!(
        writers.len() > 10,
        "the scan found only {writers:?}; it is not seeing the sources"
    );
    for command in &writers {
        assert!(
            covered.contains(command.as_str()),
            "{command} writes settings but has no row in the persist_and_sync table: add one"
        );
    }
    let registered = contract::registered_command_names();
    for command in covered {
        assert!(
            registered.iter().any(|name| name == command),
            "the table names {command}, which is not a registered command"
        );
        assert!(
            writers.iter().any(|name| name == command),
            "the table has a row for {command}, which does not write settings (any more)"
        );
    }
}

#[path = "persist_scenarios_tests.rs"]
mod scenarios;

#[path = "persist_parity_tests.rs"]
mod parity;
