use super::*;

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
