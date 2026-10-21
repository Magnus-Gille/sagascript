use super::*;

#[derive(Default)]
struct FakeRegistrar {
    calls: Vec<String>,
    register_results: std::collections::VecDeque<Result<(), String>>,
    unregister_results: std::collections::VecDeque<Result<(), String>>,
}

impl hotkey::transition::Registrar for FakeRegistrar {
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

fn registered(shortcut: &str) -> OperationalHotkey {
    OperationalHotkey::Registered(vec![shortcut.to_string()])
}

#[test]
fn invalid_watcher_settings_are_rejected_without_os_calls() {
    let mut registrar = FakeRegistrar::default();
    let result = transition_settings_hotkeys(
        &registered("old"),
        &["new".into()],
        Some("invalid profile".into()),
        &mut registrar,
    );

    assert!(matches!(
        result,
        Err(SettingsHotkeyTransitionError::Invalid(error)) if error == "invalid profile"
    ));
    assert!(registrar.calls.is_empty());
}

#[test]
fn watcher_unknown_previous_state_is_refused_without_os_calls() {
    let mut registrar = FakeRegistrar::default();
    let error = transition_settings_hotkeys(
        &OperationalHotkey::Unknown,
        &["new".into()],
        None,
        &mut registrar,
    )
    .expect_err("unknown previous state must be refused");
    let SettingsHotkeyTransitionError::Registration(failure) = error else {
        panic!("expected registration failure")
    };

    assert_eq!(failure.operational, OperationalHotkey::Unknown);
    assert_eq!(
        watcher_registration_diagnostic(failure.cause),
        "failed to unregister previous hotkeys: registration state is unknown after an earlier OS error; restart Sagascript"
    );
    assert!(registrar.calls.is_empty());
}

#[test]
fn watcher_old_unregister_failure_is_unknown_and_keeps_its_diagnostic() {
    let mut registrar = FakeRegistrar {
        unregister_results: [Err("old unregister".into())].into(),
        ..FakeRegistrar::default()
    };
    let error =
        transition_settings_hotkeys(&registered("old"), &["new".into()], None, &mut registrar)
            .expect_err("failed old unregister must stop the transition");
    let SettingsHotkeyTransitionError::Registration(failure) = error else {
        panic!("expected registration failure")
    };

    assert_eq!(failure.operational, OperationalHotkey::Unknown);
    assert_eq!(
        watcher_registration_diagnostic(failure.cause),
        "failed to unregister previous hotkeys: old unregister"
    );
    assert_eq!(registrar.calls, ["unregister:old"]);
}

#[test]
fn watcher_cleanup_failure_is_unknown_and_never_restores_old() {
    let mut registrar = FakeRegistrar {
        register_results: [Err("new registration".into())].into(),
        unregister_results: [Ok(()), Err("partial cleanup".into())].into(),
        ..FakeRegistrar::default()
    };
    let error =
        transition_settings_hotkeys(&registered("old"), &["new".into()], None, &mut registrar)
            .expect_err("new registration failure must run rollback");
    let SettingsHotkeyTransitionError::Registration(failure) = error else {
        panic!("expected registration failure")
    };

    assert_eq!(failure.operational, OperationalHotkey::Unknown);
    assert_eq!(
        watcher_registration_diagnostic(failure.cause),
        "failed to register new hotkey profiles: new registration; partial-registration cleanup failed: partial cleanup"
    );
    assert_eq!(
        registrar.calls,
        ["unregister:old", "register:new", "unregister:new"]
    );
}

#[test]
fn watcher_restore_failure_records_inactive_and_could_not_be_restored() {
    let mut registrar = FakeRegistrar {
        register_results: [Err("new registration".into()), Err("restore old".into())].into(),
        unregister_results: [Ok(()), Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let error =
        transition_settings_hotkeys(&registered("old"), &["new".into()], None, &mut registrar)
            .expect_err("new registration failure must run rollback");
    let SettingsHotkeyTransitionError::Registration(failure) = error else {
        panic!("expected registration failure")
    };

    assert_eq!(failure.operational, OperationalHotkey::Inactive);
    assert_eq!(
        watcher_registration_diagnostic(failure.cause),
        "failed to register new hotkey profiles: new registration; previous profiles could not be restored"
    );
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
}

#[test]
fn watcher_failure_restores_five_hotkey_fields_and_keeps_other_incoming_settings() {
    use sagascript_core::settings::{
        FileModelPreference, HotkeyMode, HotkeyProfile, Language, Settings, WhisperModel,
    };

    let mut previous = Settings::default();
    previous.hotkey = "Control+Shift+Space".into();
    previous.hotkey_mode = HotkeyMode::PushToTalk;
    previous.language = Language::Swedish;
    previous.hotkey_profiles = vec![HotkeyProfile {
        id: "default".into(),
        name: "Previous profile".into(),
        shortcut: previous.hotkey.clone(),
        language: Language::Swedish,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }];
    previous.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::Tiny),
    );
    previous.initial_prompt = "previous prompt".into();

    let mut incoming = Settings::default();
    incoming.hotkey = "Control+Alt+K".into();
    incoming.hotkey_mode = HotkeyMode::Toggle;
    incoming.language = Language::English;
    incoming.hotkey_profiles = vec![HotkeyProfile {
        id: "default".into(),
        name: "Incoming profile".into(),
        shortcut: incoming.hotkey.clone(),
        language: Language::English,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }];
    incoming.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::TinyEn),
    );
    incoming.initial_prompt = "new prompt".into();
    restore_previous_hotkey_settings(&mut incoming, &previous);

    assert_eq!(incoming.hotkey, previous.hotkey);
    assert_eq!(incoming.hotkey_mode, previous.hotkey_mode);
    assert_eq!(incoming.language, previous.language);
    assert_eq!(incoming.profile_models, previous.profile_models);
    assert_eq!(incoming.hotkey_profiles, previous.hotkey_profiles);
    assert_eq!(incoming.initial_prompt, "new prompt");
}

fn settings_with_hotkey(
    shortcut: &str,
    language: sagascript_core::settings::Language,
    mode: sagascript_core::settings::HotkeyMode,
    model: sagascript_core::settings::WhisperModel,
    prompt: &str,
) -> sagascript_core::settings::Settings {
    use sagascript_core::settings::{FileModelPreference, HotkeyProfile, Settings};

    Settings {
        hotkey: shortcut.to_string(),
        language,
        hotkey_mode: mode,
        hotkey_profiles: vec![HotkeyProfile::legacy_default(
            shortcut.to_string(),
            language,
        )],
        profile_models: std::collections::BTreeMap::from([(
            "default".into(),
            FileModelPreference::Whisper(model),
        )]),
        initial_prompt: prompt.to_string(),
        ..Settings::default()
    }
}

#[test]
fn actual_watcher_helper_restores_hotkey_fields_after_registration_failure() {
    use sagascript_core::settings::{HotkeyMode, Language, WhisperModel};

    let previous = settings_with_hotkey(
        "Control+Shift+Space",
        Language::Swedish,
        HotkeyMode::PushToTalk,
        WhisperModel::Tiny,
        "old prompt",
    );
    let mut incoming = settings_with_hotkey(
        "Control+Alt+K",
        Language::English,
        HotkeyMode::Toggle,
        WhisperModel::TinyEn,
        "new prompt",
    );
    let health = HotkeyHealth::new(&previous.hotkey);
    health.record(&previous.hotkey, None, registered(&previous.hotkey));
    let mut registrar = FakeRegistrar {
        register_results: [Err("new registration".into()), Ok(())].into(),
        unregister_results: [Ok(()), Ok(())].into(),
        ..FakeRegistrar::default()
    };
    let mut emitted = Vec::new();

    apply_watched_hotkey_change(
        &previous,
        &mut incoming,
        &health,
        &mut registrar,
        |change| emitted.push(change),
    );

    assert_eq!(
        registrar.calls,
        [
            "unregister:Control+Shift+Space",
            "register:Control+Alt+K",
            "unregister:Control+Alt+K",
            "register:Control+Shift+Space",
        ]
    );
    assert_eq!(incoming.hotkey, previous.hotkey);
    assert_eq!(incoming.language, previous.language);
    assert_eq!(incoming.profile_models, previous.profile_models);
    assert_eq!(incoming.hotkey_profiles, previous.hotkey_profiles);
    assert_eq!(incoming.hotkey_mode, previous.hotkey_mode);
    assert_eq!(incoming.initial_prompt, "new prompt");
    assert_eq!(health.operational_hotkey(), registered(&previous.hotkey));
    assert_eq!(
        health.status().error.as_deref(),
        Some(
            "failed to register new hotkey profiles: new registration; previous profiles restored"
        )
    );
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
}

#[test]
fn actual_watcher_helper_validation_restores_fields_without_os_calls() {
    use sagascript_core::settings::{HotkeyMode, Language, WhisperModel};

    let previous = settings_with_hotkey(
        "Control+Shift+Space",
        Language::Swedish,
        HotkeyMode::PushToTalk,
        WhisperModel::Tiny,
        "old prompt",
    );
    let mut incoming = settings_with_hotkey(
        "Banana",
        Language::English,
        HotkeyMode::Toggle,
        WhisperModel::TinyEn,
        "new prompt",
    );
    let health = HotkeyHealth::new(&previous.hotkey);
    health.record(&previous.hotkey, None, registered(&previous.hotkey));
    let mut registrar = FakeRegistrar::default();
    let mut emitted = Vec::new();

    apply_watched_hotkey_change(
        &previous,
        &mut incoming,
        &health,
        &mut registrar,
        |change| emitted.push(change),
    );

    assert!(registrar.calls.is_empty());
    assert_eq!(incoming.hotkey, previous.hotkey);
    assert_eq!(incoming.language, previous.language);
    assert_eq!(incoming.profile_models, previous.profile_models);
    assert_eq!(incoming.hotkey_profiles, previous.hotkey_profiles);
    assert_eq!(incoming.hotkey_mode, previous.hotkey_mode);
    assert_eq!(incoming.initial_prompt, "new prompt");
    assert_eq!(health.operational_hotkey(), registered(&previous.hotkey));
    assert!(health
        .status()
        .error
        .as_deref()
        .unwrap()
        .contains("previous hotkey profiles remain active"));
    assert_eq!(emitted.len(), 1);
    assert!(emitted[0].changed);
    assert_eq!(emitted[0].status, health.status());
}

#[test]
fn actual_watcher_helper_ignores_unchanged_valid_shortcuts() {
    use sagascript_core::settings::{HotkeyMode, Language, WhisperModel};

    let previous = settings_with_hotkey(
        "Control+Shift+Space",
        Language::Swedish,
        HotkeyMode::PushToTalk,
        WhisperModel::Tiny,
        "old prompt",
    );
    let mut unchanged = previous.clone();
    let health = HotkeyHealth::new(&previous.hotkey);
    health.record(&previous.hotkey, None, registered(&previous.hotkey));
    let before = health.status();
    let mut registrar = FakeRegistrar::default();
    let mut emitted = Vec::new();

    apply_watched_hotkey_change(
        &previous,
        &mut unchanged,
        &health,
        &mut registrar,
        |change| emitted.push(change),
    );

    assert!(registrar.calls.is_empty());
    assert!(emitted.is_empty());
    assert_eq!(health.status(), before);
}

#[cfg(target_os = "macos")]
use sagascript_testsupport::TempDir;

/// #359: a panic while hot-reloading one settings change used to kill the
/// watcher thread silently. The loop must log it, record it in the health
/// status and keep serving later changes.
#[test]
fn settings_watcher_survives_a_panicking_reload_and_records_it() {
    let health = hotkey::HotkeyHealth::new("Control+Shift+Space");
    let watcher = health.settings_watcher();
    let mut handled = Vec::new();
    let mut running_while_handling = Vec::new();

    run_settings_watcher_loop(1..=3, watcher, |event| {
        running_while_handling.push(watcher.status().running);
        if event == 2 {
            let _registration_change = health.registration_change();
            panic!("injected panic between unregister and register (#359)");
        }
        handled.push(event);
    });

    assert_eq!(handled, [1, 3], "the event after the panic was handled");
    assert_eq!(running_while_handling, [true, true, true]);
    let status = health.status();
    assert_eq!(status.settings_watcher.recovered_panics, 1);
    assert!(
        !status.settings_watcher.running,
        "the loop ended with its event source"
    );
    assert_eq!(
        health.operational_hotkey(),
        hotkey::OperationalHotkey::Unknown,
        "an interrupted re-registration is not reported as still registered"
    );
    assert!(!status.ok);
}

#[cfg(target_os = "macos")]
fn wait_for_settings_event(
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    settings_path: &std::path::Path,
) -> bool {
    let global_glossary_path = settings_path.parent().unwrap().join("glossary.txt");
    let profile_glossary_dir = settings_path.parent().unwrap().join("glossaries");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(Ok(event))
                if configuration_event_may_affect(
                    &event,
                    settings_path,
                    &global_glossary_path,
                    &profile_glossary_dir,
                ) =>
            {
                return true;
            }
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
    false
}

#[cfg(target_os = "macos")]
#[test]
fn macos_settings_watcher_survives_repeated_atomic_replacements() {
    use notify::{Config, PollWatcher, RecursiveMode, Watcher};

    let dir = TempDir::new();
    let settings_path = dir.join("sagascript-settings.json");
    std::fs::write(&settings_path, b"{}\n").unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = PollWatcher::new(
        tx,
        Config::default()
            .with_poll_interval(Duration::from_millis(100))
            .with_compare_contents(true),
    )
    .unwrap();
    watcher.watch(&dir, RecursiveMode::NonRecursive).unwrap();

    for value in [1, 2, 3, 4, 5] {
        let temporary = dir.join(format!("settings-{value}.tmp"));
        std::fs::write(&temporary, format!("{{\"value\":{value}}}\n")).unwrap();
        std::fs::rename(&temporary, &settings_path).unwrap();
        assert!(
            wait_for_settings_event(&rx, &settings_path),
            "watcher stopped reporting the settings path after atomic replacement {value}"
        );
    }

    drop(watcher);
}

#[test]
fn configuration_event_filter_accepts_settings_and_glossary_changes() {
    use notify::event::{CreateKind, ModifyKind};
    use notify::{Event, EventKind};

    let watch_dir = std::path::Path::new("/tmp/sagascript");
    let settings_path = watch_dir.join("sagascript-settings.json");
    let global_glossary_path = watch_dir.join("glossary.txt");
    let profile_glossary_dir = watch_dir.join("glossaries");

    for kind in [
        EventKind::Create(CreateKind::Any),
        EventKind::Modify(ModifyKind::Any),
    ] {
        for target in [
            settings_path.clone(),
            global_glossary_path.clone(),
            profile_glossary_dir.join("swedish.txt"),
        ] {
            let target_event = Event::new(kind).add_path(target);
            assert!(configuration_event_may_affect(
                &target_event,
                &settings_path,
                &global_glossary_path,
                &profile_glossary_dir,
            ));
        }
    }
}

#[test]
fn configuration_event_filter_rejects_unrelated_and_non_mutating_events() {
    use notify::event::{AccessKind, ModifyKind, RemoveKind};
    use notify::{Event, EventKind};

    let watch_dir = std::path::Path::new("/tmp/sagascript");
    let settings_path = watch_dir.join("sagascript-settings.json");
    let global_glossary_path = watch_dir.join("glossary.txt");
    let profile_glossary_dir = watch_dir.join("glossaries");
    let unrelated =
        Event::new(EventKind::Modify(ModifyKind::Any)).add_path(watch_dir.join("settings.tmp"));
    assert!(!configuration_event_may_affect(
        &unrelated,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));

    let unrelated_profile = Event::new(EventKind::Modify(ModifyKind::Any))
        .add_path(profile_glossary_dir.join("notes.md"));
    assert!(!configuration_event_may_affect(
        &unrelated_profile,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));

    let remove = Event::new(EventKind::Remove(RemoveKind::Any)).add_path(settings_path.clone());
    assert!(!configuration_event_may_affect(
        &remove,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));

    let remove_glossary = Event::new(EventKind::Remove(RemoveKind::Any))
        .add_path(profile_glossary_dir.join("swedish.txt"));
    assert!(configuration_event_may_affect(
        &remove_glossary,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));

    let access = Event::new(EventKind::Access(AccessKind::Any)).add_path(settings_path.clone());
    assert!(!configuration_event_may_affect(
        &access,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));

    let other = Event::new(EventKind::Other).add_path(settings_path.clone());
    assert!(!configuration_event_may_affect(
        &other,
        &settings_path,
        &global_glossary_path,
        &profile_glossary_dir,
    ));
}
