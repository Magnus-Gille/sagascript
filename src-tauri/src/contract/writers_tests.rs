use super::*;
use crate::contract::command_map::src_dir;

fn writers_in(sources: &[&str]) -> SettingsWriters {
    let files: Vec<syn::File> = sources
        .iter()
        .map(|source| syn::parse_str(source).expect("fixture parses"))
        .collect();
    analyse(&files)
}

#[test]
fn a_command_that_reaches_a_persist_entry_point_is_a_settings_writer() {
    let found = writers_in(&[r#"
        #[tauri::command]
        pub async fn set_direct() -> Result<(), String> { persist_and_sync(&c, |s| s.x = 1)?; Ok(()) }
        #[tauri::command]
        pub async fn apply_through_a_helper() -> Result<(), String> { helper(); Ok(()) }
        fn helper() { crate::commands::try_persist_and_replace(&c, |_| Ok::<(), String>(())); }
        #[tauri::command]
        pub async fn toggle_through_a_command() -> Result<(), String> { set_direct().await }
        #[tauri::command]
        pub async fn get_something() -> Result<(), String> { read(); Ok(()) }
        #[tauri::command]
        pub async fn save_by_hand() -> Result<(), String> { sagascript_core::settings::store::update(|s| s.x = 2)?; Ok(()) }
        fn not_a_command() { persist_and_sync(&c, |s| s.x = 3); }
    "#]);
    assert_eq!(
        found.commands,
        [
            "apply_through_a_helper",
            "set_direct",
            "toggle_through_a_command"
        ]
    );
    assert_eq!(found.bypassing, ["save_by_hand"]);
}

#[test]
fn the_scan_reads_every_source_file_but_tests_and_the_persist_module() {
    let root = src_dir();
    let scanned: Vec<String> = scanned_sources()
        .iter()
        .map(|path| {
            path.strip_prefix(&root)
                .expect("below src")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    for wanted in [
        "main.rs",
        "overlay.rs",
        "commands/hotkey.rs",
        "platform/microphone.rs",
    ] {
        assert!(
            scanned.iter().any(|path| path == wanted),
            "{wanted} is scanned: {scanned:?}"
        );
    }
    for skipped in [
        "commands/persist.rs",
        "commands/hotkey_tests.rs",
        "commands/test_fixtures.rs",
        "contract/command_map.rs",
        "contract/writers.rs",
    ] {
        assert!(
            !scanned.iter().any(|path| path == skipped),
            "{skipped} is not scanned"
        );
    }
}

#[test]
fn the_helper_may_live_in_another_file() {
    let found = writers_in(&[
        r#"
        #[tauri::command]
        pub async fn set_elsewhere() -> Result<(), String> { crate::overlay::apply(); Ok(()) }
        "#,
        r#"
        pub fn apply() { persist_and_sync(&c, |s| s.x = 1); }
        "#,
    ]);
    assert_eq!(found.commands, ["set_elsewhere"]);
}

#[test]
fn a_command_that_writes_through_a_method_is_a_writer() {
    let found = writers_in(&[r#"
        struct Saver;
        impl Saver {
            fn save(&self) { persist_and_sync(&c, |s| s.x = 1); }
        }
        trait Persists {
            fn persist(&self) { persist_and_sync(&c, |s| s.x = 2); }
        }
        #[tauri::command]
        pub async fn set_through_a_method() -> Result<(), String> { Saver.save(); Ok(()) }
        #[tauri::command]
        pub async fn set_through_a_default_method() -> Result<(), String> { thing.persist(); Ok(()) }
    "#]);
    assert_eq!(
        found.commands,
        ["set_through_a_default_method", "set_through_a_method"]
    );
}

#[test]
fn a_function_named_as_a_value_is_a_call() {
    let found = writers_in(&[r#"
        fn apply(settings: &mut Settings) { persist_and_sync(&c, |s| s.x = 1); }
        #[tauri::command]
        pub async fn set_by_reference() -> Result<(), String> { run(apply); Ok(()) }
    "#]);
    assert_eq!(found.commands, ["set_by_reference"]);
}

#[test]
fn a_renamed_import_of_an_entry_point_is_still_one() {
    let found = writers_in(&[r#"
        use super::persist_and_sync as keep;
        use super::{try_persist_and_replace as replace_all};
        #[tauri::command]
        pub async fn set_renamed() -> Result<(), String> { keep(&c, |s| s.x = 1); Ok(()) }
        #[tauri::command]
        pub async fn set_renamed_in_a_group() -> Result<(), String> { replace_all(&c, |_| Ok::<(), String>(())); Ok(()) }
    "#]);
    assert_eq!(found.commands, ["set_renamed", "set_renamed_in_a_group"]);
}

#[test]
fn a_call_inside_a_macro_is_seen() {
    let found = writers_in(&[r#"
        #[tauri::command]
        pub async fn set_in_dbg() -> Result<(), String> { dbg!(persist_and_sync(&c, |s| s.x = 1)); Ok(()) }
        #[tauri::command]
        pub async fn set_in_a_log() -> Result<(), String> {
            info!(id = %id, "saved {}", try_persist_and_replace(&c, |_| Ok::<(), String>(())).is_ok());
            Ok(())
        }
        #[tauri::command]
        pub async fn set_in_nested_groups() -> Result<(), String> {
            vec![[1, 2], [3, { helper() }]];
            Ok(())
        }
        #[tauri::command]
        pub async fn set_by_path_in_a_macro() -> Result<(), String> {
            dbg!(crate::commands::persist_and_sync(&c, |s| s.x = 1));
            Ok(())
        }
        #[tauri::command]
        pub async fn get_nothing() -> Result<(), String> { dbg!(read(), other(1)); Ok(()) }
        fn helper() { persist_and_sync(&c, |s| s.x = 3); }
    "#]);
    assert_eq!(
        found.commands,
        [
            "set_by_path_in_a_macro",
            "set_in_a_log",
            "set_in_dbg",
            "set_in_nested_groups"
        ]
    );
}

#[test]
fn every_spelling_of_a_store_write_is_a_bypass() {
    let found = writers_in(&[r#"
        use sagascript_core::settings::store::update as write_settings;
        use sagascript_core::machine as machine_state;
        use sagascript_core::settings::store::{save_to, try_update};
        fn via_machine() { sagascript_core::machine::try_update(|s| s.x = 1); }
        fn via_machine_update() { sagascript_core::machine::update(|s| s.x = 1); }
        fn via_update_with() { store::update_with(&probe, |s| s.x = 1); }
        fn via_try_update_with() { sagascript_core::settings::store::try_update_with(&probe, |_| Ok(())); }
        fn via_save() { sagascript_core::settings::store::save(&settings); }
        fn via_alias() { write_settings(|s| s.x = 1); }
        fn via_module_alias() { machine_state::update(|s| s.x = 1); }
        fn via_import() { try_update(|s| s.x = 1); }
        fn via_save_to() { save_to(&path, &settings); }
        fn via_macro() { dbg!(sagascript_core::machine::update(|s| s.x = 1)); }
        fn reads_only() { sagascript_core::machine::load(); store::load_from(&path); }
    "#]);
    assert_eq!(
        found.bypassing,
        [
            "via_alias",
            "via_import",
            "via_machine",
            "via_machine_update",
            "via_macro",
            "via_module_alias",
            "via_save",
            "via_save_to",
            "via_try_update_with",
            "via_update_with"
        ]
    );
}

/// The only function outside persist.rs that writes the settings file. It is
/// the startup/hot-reload permission gate in main.rs: it takes auto-paste off
/// in the file when Accessibility is missing and hands the result to its caller,
/// which sets up or replaces the controller itself. A new writer is added here
/// with its reason, or goes through `persist_and_sync`.
const STORE_WRITERS_OUTSIDE_PERSIST: &[&str] = &["load_settings_with_permission_gate"];

#[test]
fn nothing_but_the_permission_gate_writes_the_store_outside_persist() {
    assert_eq!(settings_writers().bypassing, STORE_WRITERS_OUTSIDE_PERSIST);
}
