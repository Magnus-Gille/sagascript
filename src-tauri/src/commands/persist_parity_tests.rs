//! App side of the CLI/app parity table (#373): every row of
//! `sagascript_core::test_support::settings_ops_table` runs through the app's
//! real setter (or the persistence half of it, where the rest needs the OS or
//! the network) on Tauri's mock runtime, and the persisted settings are compared
//! with what the shared core operation leaves.
//!
//! The CLI runs the same table through `sagascript config`
//! (`crates/sagascript-cli/tests/settings_parity.rs`), so the two surfaces are
//! equal to each other through core, and the app does not depend on the CLI
//! crate for it (that edge is what #374 removes).
//!
//! It uses the harness of the parent module: a child process of this test
//! binary on a fake settings file (`isolated`), a mock app with a controller
//! (`app_with`) and the setters called as the frontend would. A row's start is
//! written to the file and loaded the way the app loads it, so the controller
//! holds what the file holds.

use sagascript_core::machine;
use sagascript_core::settings::store;
use sagascript_core::test_support::settings_ops_table::{
    assert_persisted, cases, dictionary_text_with, reset_as_op, Case, Op, Surface,
};

use super::*;

/// Run `case`'s operation through the app. `Err` is the setter's refusal.
fn run_op(app: &App, runtime: &tokio::runtime::Runtime, op: &Op) -> Result<(), String> {
    let state = controller(app);
    match op {
        Op::Language(language) => persist_language(&state, *language).map(drop),
        Op::WhisperModel(model) => runtime.block_on(set_whisper_model(state, *model)),
        Op::AutoSelectModel(enabled) => runtime.block_on(set_auto_select_model(state, *enabled)),
        Op::PianissimoDictation(enabled) => {
            runtime.block_on(set_pianissimo_dictation(state, *enabled))
        }
        Op::AutoPaste {
            enabled,
            permission,
        } => persist_auto_paste(&state, *enabled, *permission).map(drop),
        Op::VadEnabled(enabled) => persist_vad_enabled(&state, *enabled).map(drop),
        Op::ShowOverlay(enabled) => persist_show_overlay(&state, *enabled).map(drop),
        Op::BeamSize(beam) => runtime.block_on(set_beam_size(state, *beam)),
        Op::TemperatureFallback(enabled) => {
            runtime.block_on(set_temperature_fallback(state, *enabled))
        }
        Op::EnginePrewarm(mode) => {
            runtime.block_on(set_engine_prewarm(state, mode.id().to_string()))
        }
        Op::EngineIdleUnloadMinutes(minutes) => {
            runtime.block_on(set_engine_idle_unload_minutes(state, *minutes))
        }
        Op::FileTranscriptionModel(choice) => {
            runtime.block_on(set_file_transcription_model(state, choice.id().to_string()))
        }
        Op::HotkeyMode(mode) => persist_hotkey(app, HotkeyChange::Mode(*mode)),
        Op::Hotkey(shortcut) => {
            let profiles = profiles_with_default_shortcut(
                lock_controller(&state).settings(),
                shortcut.to_string(),
            )
            .map_err(text)?;
            persist_hotkey(app, HotkeyChange::Profiles(profiles))
        }
        Op::Profiles(edit) => {
            // The frontend sends the whole resulting list and chooses a model
            // with a command of its own.
            let (profiles, model) = edit.resulting(&machine::load())?;
            persist_hotkey(app, HotkeyChange::Profiles(profiles))?;
            match model {
                Some((id, choice)) => runtime.block_on(set_profile_model(
                    controller(app),
                    id,
                    choice.id().to_string(),
                )),
                None => Ok(()),
            }
        }
        Op::ProfileModel { id, model } => runtime.block_on(set_profile_model(
            state,
            id.to_string(),
            model.id().to_string(),
        )),
        Op::DictionaryAdd {
            profile,
            term,
            aliases,
        } => {
            // The editor saves the text it read with the entry added to it.
            let read = machine::load()
                .profile_glossaries
                .get(*profile)
                .cloned()
                .unwrap_or_default();
            let source = dictionary_text_with(&read, term, aliases);
            runtime
                .block_on(set_profile_glossary(
                    state,
                    profile.to_string(),
                    source,
                    Some(read),
                ))
                .map_err(text)
        }
        Op::InitialPrompt(prompt) => runtime
            .block_on(set_initial_prompt(state, prompt.to_string(), None))
            .map_err(text),
        // The app has no reset: choosing the default value is the setter.
        Op::Reset(key) => run_op(app, runtime, &reset_as_op(key)),
    }
}

/// The directory of the fake settings file holds only this test's files, so
/// what the store keeps beside the file (dictionary files, the lock, recovery
/// copies) is removed between rows. The child's own directories
/// (`CHILD_ENV_DIR`) stay, and so does its report, which is written after the
/// last row.
fn clear_the_fake_directory() {
    let file = fake_file();
    let dir = file.parent().expect("the settings file has a directory");
    for entry in std::fs::read_dir(dir).expect("the directory is readable") {
        let path = entry.expect("an entry").path();
        let kept = path
            .extension()
            .is_some_and(|extension| extension == "report")
            || path.file_name().is_some_and(|name| name == CHILD_ENV_DIR);
        if kept {
            continue;
        }
        if path.is_dir() {
            std::fs::remove_dir_all(&path).expect("remove a directory");
        } else {
            std::fs::remove_file(&path).expect("remove a file");
        }
    }
}

fn run_row(runtime: &tokio::runtime::Runtime, case: &Case) {
    clear_the_fake_directory();
    let file = fake_file();
    std::fs::write(&file, case.file_text()).expect("write the starting settings");
    let before = store::load_from(&file);
    // The controller holds the settings as the app loaded them.
    let app = app_with(machine::load());

    let ran = run_op(&app, runtime, &case.op);

    let after = store::load_from(&file);
    assert_persisted(case, Surface::App, ran, &before, &after);
}

fn run_table_of_cases() -> String {
    let runtime = runtime();
    let table = cases();
    for case in &table {
        run_row(&runtime, case);
    }
    table
        .iter()
        .map(|case| case.name.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn every_setter_persists_what_the_shared_operation_leaves() {
    let Some(report) = isolated(
        module_path!(),
        "every_setter_persists_what_the_shared_operation_leaves",
        run_table_of_cases,
    ) else {
        return;
    };
    let expected: Vec<String> = cases().into_iter().map(|case| case.name).collect();
    assert!(expected.len() > 40, "the table is {} rows", expected.len());
    assert_eq!(
        report.lines().map(str::to_owned).collect::<Vec<_>>(),
        expected,
        "the child ran another table than the parent"
    );
}
