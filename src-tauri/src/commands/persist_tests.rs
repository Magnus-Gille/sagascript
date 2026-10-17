//! The table test for `persist_and_sync` (#370): after any setter, the settings
//! file and the controller hold the same settings.
//!
//! Settings are global user state, and the store reads its location from
//! `SAGASCRIPT_SETTINGS_PATH`. ADR 0004 (rule 5) forbids setting that in a test
//! process with other tests running, so the table runs in a child of this test
//! binary whose environment points at a fake settings file
//! (`sagascript_testsupport::{fake_settings, isolated_cli_env}`). The parent
//! checks that the child ran every row; a filter that matches nothing exits
//! with success too.
//!
//! Each row calls the real setter (a `State` from Tauri's mock runtime), after
//! the file was changed behind the controller's back the way the CLI does. A
//! setter that mirrored only its own field, or forgot to mirror at all, leaves
//! the controller with the stale value and the comparison fails.

use std::collections::BTreeSet;
use std::process::Command;
use std::sync::Mutex;

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
const REPORT: &str = "persist-table.report";
/// The settings field the harness changes behind the controller's back before
/// each row, except for the row that sets it itself.
const CANARY_SETTER: &str = "set_engine_idle_unload_minutes";

struct Row {
    /// The commands the row stands for.
    commands: &'static [&'static str],
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

/// One row per setter. A new `set_*` command needs a row here or a reason in
/// [`NOT_SETTINGS`] (`every_setter_command_has_a_row_or_a_reason`).
const ROWS: &[Row] = &[
    Row {
        commands: &["set_language"],
        arrange: nothing,
        run: |app, _| persist_language(&controller(app), Language::Swedish).map(drop),
        landed: |s| s.default_profile().language == Language::Swedish,
    },
    Row {
        commands: &["set_onboarding_completed"],
        arrange: nothing,
        run: |app, rt| rt.block_on(set_onboarding_completed(controller(app))),
        landed: |s| s.has_completed_onboarding,
    },
    Row {
        commands: &["set_file_transcription_model"],
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_file_transcription_model(
                controller(app),
                model_id(WhisperModel::KbWhisperSmall),
            ))
        },
        landed: |s| {
            s.file_transcription_model == FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
        },
    },
    Row {
        commands: &["set_profile_model"],
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_profile_model(
                controller(app),
                "default".into(),
                model_id(WhisperModel::KbWhisperMedium),
            ))
        },
        landed: |s| {
            s.profile_models.get("default")
                == Some(&FileModelPreference::Whisper(WhisperModel::KbWhisperMedium))
        },
    },
    Row {
        commands: &["set_whisper_model"],
        arrange: nothing,
        run: |app, rt| {
            rt.block_on(set_whisper_model(
                controller(app),
                WhisperModel::KbWhisperBase,
            ))
        },
        landed: |s| {
            s.profile_models.get("default")
                == Some(&FileModelPreference::Whisper(WhisperModel::KbWhisperBase))
        },
    },
    Row {
        commands: &["set_pianissimo_dictation"],
        arrange: nothing,
        run: |app, rt| rt.block_on(set_pianissimo_dictation(controller(app), false)),
        landed: |s| s.profile_models.get("default") == Some(&FileModelPreference::Auto),
    },
    Row {
        commands: &["set_auto_select_model"],
        arrange: nothing,
        run: |app, rt| rt.block_on(set_auto_select_model(controller(app), false)),
        landed: |s| {
            s.profile_models.get("default") == Some(&FileModelPreference::Whisper(s.whisper_model))
        },
    },
    Row {
        commands: &["set_auto_paste"],
        arrange: nothing,
        // Disabling needs no Accessibility permission, so this row is the same
        // on every platform.
        run: |app, rt| rt.block_on(set_auto_paste(controller(app), false)),
        landed: |s| !s.auto_paste,
    },
    Row {
        commands: &["set_show_overlay"],
        arrange: nothing,
        run: |app, _| persist_show_overlay(&controller(app), false).map(drop),
        landed: |s| !s.show_overlay,
    },
    Row {
        commands: &["set_initial_prompt", "set_profile_glossary"],
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
        arrange: nothing,
        run: |app, rt| rt.block_on(set_beam_size(controller(app), 7)),
        landed: |s| s.beam_size == 7,
    },
    Row {
        commands: &["set_temperature_fallback"],
        arrange: nothing,
        run: |app, rt| rt.block_on(set_temperature_fallback(controller(app), false)),
        landed: |s| !s.temperature_fallback,
    },
    Row {
        commands: &["set_vad_enabled"],
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
        arrange: nothing,
        run: |app, rt| rt.block_on(set_engine_idle_unload_minutes(controller(app), 11)),
        landed: |s| s.engine_idle_unload_minutes == 11,
    },
    Row {
        commands: &["set_hotkey_mode"],
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
        arrange: nothing,
        run: |app, _| {
            let mut profiles = store::load().resolved_hotkey_profiles();
            profiles[0].set_primary_shortcut("Control+Alt+D".into());
            persist_hotkey(app, HotkeyChange::Profiles(profiles))
        },
        landed: |s| s.hotkey == "Control+Alt+D",
    },
];

/// `set_*` commands that do not write the settings file, and why.
const NOT_SETTINGS: &[(&str, &str)] = &[(
    "set_update_result_pending",
    "marks an update guard in process memory; no setting is involved",
)];

/// Commands that write settings without a `set_` name.
const OTHER_WRITERS: &[&str] = &["apply_training_glossary", "retry_hotkey_registration"];

fn as_json(settings: &Settings) -> serde_json::Value {
    serde_json::to_value(settings).expect("settings serialize")
}

/// The child's work: runs every row against the fake settings file the process
/// environment names.
fn run_table() {
    let settings_file = store::settings_path();
    assert!(
        store::settings_path_is_overridden(),
        "the table must run on a fake settings file, never the user's"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let app = tauri::test::mock_app();
    app.manage::<SharedController>(Mutex::new(AppController::new(Settings::default())));

    let mut ran = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let name = row.commands.join("+");
        // The file moves on without the controller (a CLI call, a watcher that
        // has not fired yet).
        let canary = 40 + index as u32;
        store::update(|s| s.engine_idle_unload_minutes = canary)
            .unwrap_or_else(|error| panic!("{name}: cannot write the fake file: {error}"));
        (row.arrange)();

        (row.run)(&app, &runtime)
            .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

        let on_disk = store::load_from(&settings_file);
        let in_memory = lock_controller(&controller(&app)).settings().clone();
        assert_eq!(
            as_json(&in_memory),
            as_json(&on_disk),
            "{name}: the controller and the file differ after the setter"
        );
        assert!(
            (row.landed)(&on_disk),
            "{name}: the new value is not in the settings"
        );
        if !row.commands.contains(&CANARY_SETTER) {
            assert_eq!(
                in_memory.engine_idle_unload_minutes, canary,
                "{name}: a field the setter did not touch was not refreshed from the file"
            );
        }
        ran.push(name);
    }
    let dir = settings_file
        .parent()
        .expect("the settings file has a directory");
    std::fs::write(dir.join(REPORT), ran.join("\n")).expect("the report is writable");
}

#[test]
fn every_setter_leaves_the_file_and_the_controller_equal() {
    if std::env::var_os(CHILD).is_some() {
        run_table();
        return;
    }
    let fake = fake_settings();
    let this_test = module_path!()
        .split_once("::")
        .map(|(_, path)| format!("{path}::every_setter_leaves_the_file_and_the_controller_equal"))
        .expect("a module path below the crate");
    let mut command = Command::new(std::env::current_exe().expect("the test binary"));
    isolated_cli_env(&mut command, fake.path())
        .env(CHILD, "1")
        .args(["--exact", &this_test, "--test-threads=1", "--nocapture"]);
    let output = command.output().expect("the test binary starts");
    assert!(
        output.status.success(),
        "the table failed in the child:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = std::fs::read_to_string(fake.dir().join(REPORT)).expect("the child ran the table");
    let expected: Vec<String> = ROWS.iter().map(|row| row.commands.join("+")).collect();
    assert_eq!(
        report.lines().map(str::to_owned).collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn every_setter_command_has_a_row_or_a_reason() {
    let covered: BTreeSet<&str> = ROWS
        .iter()
        .flat_map(|row| row.commands.iter().copied())
        .collect();
    let registered = contract::registered_command_names();
    for command in &registered {
        let writes_settings =
            command.starts_with("set_") || OTHER_WRITERS.contains(&command.as_str());
        if !writes_settings {
            continue;
        }
        let excused = NOT_SETTINGS.iter().any(|(name, _)| name == command);
        assert!(
            covered.contains(command.as_str()) || excused,
            "{command} writes settings but has no row in the persist_and_sync table: add one, \
             or a reason to NOT_SETTINGS"
        );
    }
    for command in covered
        .iter()
        .copied()
        .chain(NOT_SETTINGS.iter().map(|(name, _)| *name))
    {
        assert!(
            registered.iter().any(|name| name == command),
            "the table names {command}, which is not a registered command"
        );
    }
}
