use super::*;
use crate::hotkey::transition::Registrar;
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Default)]
struct FakeRegistrar {
    calls: Vec<String>,
    register_results: VecDeque<Result<(), String>>,
    unregister_results: VecDeque<Result<(), String>>,
}

impl Registrar for FakeRegistrar {
    fn register(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.calls.push(format!("register:{}", shortcuts.join(",")));
        self.register_results.pop_front().unwrap_or(Ok(()))
    }

    fn unregister(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.calls
            .push(format!("unregister:{}", shortcuts.join(",")));
        self.unregister_results.pop_front().unwrap_or(Ok(()))
    }
}

fn command_context<'a>(
    old_operational: &'a OperationalHotkey,
    new_shortcuts: &'a [String],
) -> HotkeyChangeContext<'a> {
    HotkeyChangeContext {
        old_primary: "old-primary",
        new_primary: "new-primary",
        old_operational,
        new_shortcuts,
    }
}

fn registered(shortcut: &str) -> OperationalHotkey {
    OperationalHotkey::Registered(vec![shortcut.to_string()])
}

fn assert_pending_persistence(health: &HotkeyHealth) {
    assert_eq!(
        health.status().error.as_deref(),
        Some("hotkey profile change is pending persistence")
    );
    assert_eq!(health.operational_hotkey(), registered("new"));
}

fn assert_lifecycle_configuration_released(controller: &SharedController) {
    let mut controller = lock_controller(controller);
    assert!(
        controller.begin_hotkey_configuration_change(),
        "a later recording/configuration attempt must not be blocked by a leaked lease"
    );
    controller.end_hotkey_configuration_change();
}

#[test]
fn healthy_identical_shortcuts_keep_the_persist_only_fast_path() {
    let shortcuts = vec!["Control+Shift+Space".to_string()];
    let registered = OperationalHotkey::Registered(shortcuts.clone());

    assert!(healthy_shortcuts_are_identical(
        &shortcuts,
        &shortcuts,
        &registered,
        false
    ));
    assert!(!healthy_shortcuts_are_identical(
        &shortcuts,
        &shortcuts,
        &registered,
        true
    ));
    assert!(!healthy_shortcuts_are_identical(
        &["Control+Alt+K".to_string()],
        &shortcuts,
        &registered,
        false
    ));
    assert!(!healthy_shortcuts_are_identical(
        &shortcuts,
        &shortcuts,
        &OperationalHotkey::Unknown,
        false
    ));
}

#[test]
fn command_registration_failure_mapping_keeps_cleanup_and_restore_diagnostics() {
    let (cleanup_health, cleanup_message) = command_registration_failure_diagnostics(
        crate::hotkey::transition::TransitionFailureCause::RegisterNew {
            error: "new denied".into(),
            rollback: crate::hotkey::transition::RollbackOutcome::CleanupFailed(
                "cleanup denied".into(),
            ),
        },
    );
    assert_eq!(
        cleanup_health.as_deref(),
        Some("new registration failed: new denied; partial registration cleanup failed: cleanup denied; operational state is unknown")
    );
    assert_eq!(
        cleanup_message,
        "Failed to register hotkey profiles: new denied; cleanup failed: cleanup denied"
    );

    let (restore_health, restore_message) = command_registration_failure_diagnostics(
        crate::hotkey::transition::TransitionFailureCause::RegisterNew {
            error: "new denied".into(),
            rollback: crate::hotkey::transition::RollbackOutcome::RestoreFailed(
                "restore denied".into(),
            ),
        },
    );
    assert_eq!(
        restore_health.as_deref(),
        Some("new registration failed: new denied; restoring previous hotkeys also failed: restore denied")
    );
    assert_eq!(
        restore_message,
        "Failed to register hotkey profiles: new denied"
    );
}

#[test]
fn command_persistence_rollback_keeps_health_diagnostics() {
    use crate::hotkey::transition::RollbackOutcome;

    assert_eq!(
        persistence_rollback_health_error(&RollbackOutcome::CleanupFailed("cleanup denied".into()))
            .as_deref(),
        Some("failed to unregister unpersisted hotkeys; operational state is unknown")
    );
    assert_eq!(
        persistence_rollback_health_error(&RollbackOutcome::RestoreFailed("restore denied".into()))
            .as_deref(),
        Some("failed to restore previous hotkeys: restore denied")
    );
    assert_eq!(
        persistence_rollback_health_error(&RollbackOutcome::Restored(OperationalHotkey::Inactive)),
        None
    );
}

#[test]
fn actual_command_helper_rolls_back_after_persistence_failure_and_emits_final_status() {
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar {
        register_results: [Ok(()), Ok(())].into(),
        unregister_results: [Ok(()), Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            assert_pending_persistence(&health);
            Err::<(), _>("disk full")
        },
        |change| emitted.push(change),
    );

    assert_eq!(persist_calls, 1);
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
    assert_eq!(
        result.expect_err("persistence must fail").to_string(),
        "Failed to persist hotkey profiles: disk full"
    );
    assert_eq!(health.operational_hotkey(), registered("old"));
    assert!(health.status().ok);
    assert_eq!(health.status().shortcut, "old-primary");
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
    assert_eq!(emitted[0].status.error, None);
    assert_eq!(emitted[0].status.shortcuts, vec!["old".to_string()]);
}

#[test]
fn actual_command_helper_records_cleanup_failure_and_does_not_restore_old() {
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar {
        register_results: [Ok(()), Err("restore old failed".into())].into(),
        unregister_results: [Ok(()), Err("cleanup new failed".into())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            assert_pending_persistence(&health);
            Err::<(), _>("disk full")
        },
        |change| emitted.push(change),
    );

    assert_eq!(persist_calls, 1);
    assert_eq!(
        registrar.calls,
        ["unregister:old", "register:new", "unregister:new"]
    );
    assert_eq!(
        result.expect_err("persistence must fail").to_string(),
        "Failed to persist hotkey profiles: disk full"
    );
    assert_eq!(health.operational_hotkey(), OperationalHotkey::Unknown);
    assert_eq!(
        health.status().error.as_deref(),
        Some("failed to unregister unpersisted hotkeys; operational state is unknown")
    );
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
}

#[test]
fn actual_command_helper_restore_failure_leaves_inactive_and_reports_it() {
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar {
        register_results: [Ok(()), Err("restore old failed".into())].into(),
        unregister_results: [Ok(()), Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            assert_pending_persistence(&health);
            Err::<(), _>("disk full")
        },
        |change| emitted.push(change),
    );

    assert_eq!(persist_calls, 1);
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
    assert_eq!(
        result.expect_err("persistence must fail").to_string(),
        "Failed to persist hotkey profiles: disk full"
    );
    assert_eq!(health.operational_hotkey(), OperationalHotkey::Inactive);
    assert_eq!(
        health.status().error.as_deref(),
        Some("failed to restore previous hotkeys: restore old failed")
    );
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
}

#[test]
fn actual_command_helper_persistence_failure_from_inactive_stays_inactive() {
    let health = HotkeyHealth::new("old-primary");
    health.record("old-primary", None, OperationalHotkey::Inactive);
    let mut registrar = FakeRegistrar {
        register_results: [Ok(())].into(),
        unregister_results: [Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let old_operational = OperationalHotkey::Inactive;
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            assert_pending_persistence(&health);
            Err::<(), _>("disk full")
        },
        |change| emitted.push(change),
    );

    assert_eq!(persist_calls, 1);
    assert_eq!(registrar.calls, ["register:new", "unregister:new"]);
    assert!(result.is_err());
    assert_eq!(health.operational_hotkey(), OperationalHotkey::Inactive);
    assert_eq!(health.status().error, None);
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
}

#[test]
fn actual_command_helper_success_commits_and_emits_new_health_once() {
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar::default();
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            assert_pending_persistence(&health);
            Ok::<_, &'static str>("persisted")
        },
        |change| emitted.push(change),
    );

    assert_eq!(result.unwrap(), "persisted");
    assert_eq!(persist_calls, 1);
    assert_eq!(registrar.calls, ["unregister:old", "register:new"]);
    assert_eq!(health.operational_hotkey(), registered("new"));
    assert!(health.status().ok);
    assert_eq!(health.status().shortcut, "new-primary");
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
    assert_eq!(emitted[0].status.shortcuts, vec!["new".to_string()]);
}

#[test]
fn actual_command_helper_registration_failure_does_not_persist() {
    let health = HotkeyHealth::new("old-primary");
    health.record("old-primary", None, registered("old"));
    let mut registrar = FakeRegistrar {
        register_results: [Err("new registration failed".into()), Ok(())].into(),
        unregister_results: [Ok(()), Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let old_operational = registered("old");
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = commit_registered_hotkey_change(
        command_context(&old_operational, &new_shortcuts),
        &health,
        &mut registrar,
        || {
            persist_calls += 1;
            Ok::<_, &'static str>(())
        },
        |change| emitted.push(change),
    );

    assert_eq!(persist_calls, 0);
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
    assert_eq!(
        result.expect_err("registration must fail").to_string(),
        "Failed to register hotkey profiles: new registration failed"
    );
    assert_eq!(health.operational_hotkey(), registered("old"));
    assert_eq!(health.status().error, None);
    assert!(
        emitted.is_empty(),
        "identical restored status is deduplicated"
    );
}

#[test]
fn actual_command_helper_old_unregister_failure_does_not_persist_and_releases_lifecycle() {
    let controller = Mutex::new(crate::app_controller::AppController::new(
        sagascript_core::settings::Settings::default(),
    ));
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar {
        unregister_results: [Err("old unregister failed".into())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = {
        let _recording_lease = acquire_hotkey_configuration(&controller).unwrap();
        commit_registered_hotkey_change(
            command_context(&old_operational, &new_shortcuts),
            &health,
            &mut registrar,
            || {
                persist_calls += 1;
                Ok::<_, &'static str>(())
            },
            |change| emitted.push(change),
        )
    };

    assert_eq!(persist_calls, 0);
    assert_eq!(registrar.calls, ["unregister:old"]);
    let error = result.expect_err("old unregister failure must reject the change");
    assert_eq!(error.code(), "hotkey_change_failed");
    assert_eq!(
        error.to_string(),
        "Failed to unregister active hotkeys: old unregister failed"
    );
    assert_eq!(health.operational_hotkey(), OperationalHotkey::Unknown);
    assert_eq!(
        health.status().error.as_deref(),
        Some("failed to unregister active hotkeys: old unregister failed; operational state is unknown")
    );
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
    assert_lifecycle_configuration_released(&controller);
}

#[test]
fn actual_command_helper_cleanup_failure_does_not_persist_and_releases_lifecycle() {
    let controller = Mutex::new(crate::app_controller::AppController::new(
        sagascript_core::settings::Settings::default(),
    ));
    let health = HotkeyHealth::new("old-primary");
    let old_operational = registered("old");
    health.record("old-primary", None, old_operational.clone());
    let mut registrar = FakeRegistrar {
        register_results: [Err("new registration failed".into())].into(),
        unregister_results: [Ok(()), Err("cleanup failed".into())].into(),
        ..FakeRegistrar::default()
    };
    let new_shortcuts = vec!["new".to_string()];
    let mut persist_calls = 0;
    let mut emitted = Vec::new();

    let result = {
        let _recording_lease = acquire_hotkey_configuration(&controller).unwrap();
        commit_registered_hotkey_change(
            command_context(&old_operational, &new_shortcuts),
            &health,
            &mut registrar,
            || {
                persist_calls += 1;
                Ok::<_, &'static str>(())
            },
            |change| emitted.push(change),
        )
    };

    assert_eq!(persist_calls, 0);
    assert_eq!(
        registrar.calls,
        ["unregister:old", "register:new", "unregister:new"]
    );
    let error = result.expect_err("new registration and cleanup failures must reject the change");
    assert_eq!(error.code(), "hotkey_change_failed");
    assert_eq!(
        error.to_string(),
        "Failed to register hotkey profiles: new registration failed; cleanup failed: cleanup failed"
    );
    assert_eq!(health.operational_hotkey(), OperationalHotkey::Unknown);
    assert_eq!(
        health.status().error.as_deref(),
        Some("new registration failed: new registration failed; partial registration cleanup failed: cleanup failed; operational state is unknown")
    );
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
    assert_lifecycle_configuration_released(&controller);
}

#[test]
fn the_monitor_reinstall_answers_decide_whether_registration_goes_on() {
    use crate::commands::main_thread::MainThreadReply::{Disconnected, Done, TimedOut};

    assert!(after_monitor_install(Done(Ok(()))).is_ok());
    // A failed install is logged; registration then reports the monitor.
    assert!(after_monitor_install(Done(Err("no event loop".into()))).is_ok());
    // The main thread is gone (shutdown, or the closure panicked): nothing to
    // wait for.
    assert!(after_monitor_install(Disconnected).is_ok());

    // The install is still queued. Registering now would say "restart
    // Sagascript" about a monitor that is about to exist: ask for another try.
    let error = after_monitor_install(TimedOut).expect_err("a timeout ends the call");
    assert_eq!(error.code(), "busy");
    assert!(error.retryable());
    assert!(error.to_string().contains("try again"), "{error}");
    assert!(!error.to_string().contains("restart"), "{error}");
}

/// A profile of the scenarios below: on its own shortcut, Swedish unless said.
fn profile(id: &str, shortcut: &str) -> HotkeyProfile {
    HotkeyProfile {
        id: id.into(),
        name: format!("Profile {id}"),
        shortcut: shortcut.into(),
        language: sagascript_core::settings::Language::Swedish,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

/// Settings with the profiles `default` (Swedish) and, optionally, others.
fn settings_with(profiles: Vec<HotkeyProfile>) -> Settings {
    Settings {
        hotkey: profiles[0].shortcut.clone(),
        hotkey_profiles: profiles,
        ..Settings::default()
    }
}

/// `apply_hotkey_change` used to turn every refusal of the settings into
/// `hotkey_invalid`, so the Settings window could not tell a language change a
/// dictionary blocks from a shortcut that cannot be used. A refusal now rejects
/// with the code of its `SettingsError` and its text, unwrapped (ADR 0006,
/// #360 part 2b).
#[test]
fn a_refused_hotkey_change_rejects_with_the_code_and_text_of_its_settings_error() {
    use sagascript_core::settings::{FileModelPreference, Language, WhisperModel};

    let default_profile = profile("default", "Control+Shift+Space");

    let mut blocked = settings_with(vec![default_profile.clone()]);
    blocked
        .profile_glossaries
        .insert("default".into(), "Gille = Jille".into());
    let mut english = default_profile.clone();
    english.language = Language::English;

    let mut inactive = settings_with(vec![default_profile.clone()]);
    inactive
        .profile_glossaries
        .insert("extra".into(), "Gille".into());

    let mut incompatible = settings_with(vec![default_profile.clone()]);
    incompatible.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::TinyEn),
    );

    let mut banana = default_profile.clone();
    banana.shortcut = "Banana".into();

    let cases: Vec<(&str, Settings, HotkeyChange, &str)> = vec![
        (
            "a language change a personal dictionary blocks",
            blocked,
            HotkeyChange::Profiles(vec![english]),
            "dictionary_blocks_language_change",
        ),
        (
            "an empty profile list",
            settings_with(vec![default_profile.clone()]),
            HotkeyChange::Profiles(Vec::new()),
            "profile_invalid",
        ),
        (
            "a profile id that cannot name a file",
            settings_with(vec![default_profile.clone()]),
            HotkeyChange::Profiles(vec![profile("Not Valid", "Control+Shift+K")]),
            "profile_invalid",
        ),
        (
            "a new profile that already has a dictionary",
            inactive,
            HotkeyChange::Profiles(vec![
                default_profile.clone(),
                profile("extra", "Control+Shift+K"),
            ]),
            "inactive_profile_dictionary",
        ),
        (
            "a model that cannot serve its profile's language",
            incompatible,
            HotkeyChange::Profiles(vec![default_profile.clone()]),
            "model_incompatible",
        ),
        (
            "two profiles on one shortcut",
            settings_with(vec![default_profile.clone()]),
            HotkeyChange::Profiles(vec![
                default_profile.clone(),
                profile("other", "Control+Shift+Space"),
            ]),
            "hotkey_invalid",
        ),
        (
            "a shortcut that does not parse",
            settings_with(vec![default_profile.clone()]),
            HotkeyChange::Profiles(vec![banana]),
            "hotkey_invalid",
        ),
    ];
    for (what, stored, change, code) in cases {
        let refusal = change
            .prepare(&stored)
            .expect_err(&format!("{what}: the settings refuse it"));
        let error = prepare_candidate(&change, &stored)
            .expect_err(&format!("{what}: the command rejects it"));
        assert_eq!(error.code(), code, "{what}");
        assert_eq!(error.to_string(), refusal.to_string(), "{what}");
        assert!(!error.retryable(), "{what}");
        assert!(
            matches!(error, CommandError::Settings(_)),
            "{what}: {error:?}"
        );

        // The persistence half names the same code for the same refusal.
        let mut fresh = stored.clone();
        let persisted = change
            .apply_registered(&mut fresh, &[])
            .expect_err(&format!("{what}: persisting rejects it"));
        assert_eq!(persisted.code(), code, "{what}");
        assert_eq!(persisted.to_string(), refusal.to_string(), "{what}");
    }
}

/// Only a failure of the shortcuts themselves is a `hotkey_change_failed`: the
/// registration moved on while the settings were being saved.
#[test]
fn a_shortcut_configuration_that_changed_underneath_is_a_failed_change() {
    let stored = settings_with(vec![profile("default", "Control+Shift+Space")]);
    let change = HotkeyChange::Mode(HotkeyMode::Toggle);
    let mut fresh = stored;
    let error = change
        .apply_registered(&mut fresh, &["Control+Alt+K".to_string()])
        .expect_err("the registered shortcuts are not the ones the change resolves to");
    assert_eq!(error.code(), "hotkey_change_failed");
}
