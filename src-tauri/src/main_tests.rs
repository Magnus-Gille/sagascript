use super::*;
#[cfg(test)]
use sagascript_core::settings::validate_hotkey;

/// Select the shortcut that is safe to register at startup. Invalid persisted
/// values remain visible in Settings, but never become active; the default
/// stays operational so an upgrade cannot strand dictation entirely.
#[cfg(test)]
fn startup_hotkey_candidate(requested: &str) -> (String, Option<String>) {
    match validate_hotkey(requested) {
        Ok(()) => (requested.to_string(), None),
        Err(error) => {
            let fallback = sagascript_core::settings::Settings::default().hotkey;
            debug_assert!(validate_hotkey(&fallback).is_ok());
            (fallback, Some(error))
        }
    }
}

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

#[test]
fn elapsed_ms_if_success_only_marks_successful_operations_complete() {
    let start = Instant::now();
    assert!(elapsed_ms_if_success::<(), ()>(&Ok(()), start).is_some());
    assert!(elapsed_ms_if_success::<(), ()>(&Err(()), start).is_none());
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

#[test]
fn startup_keeps_a_valid_hotkey() {
    let requested = "Option+Space";
    let (candidate, error) = startup_hotkey_candidate(requested);

    assert_eq!(candidate, requested);
    assert!(error.is_none());
}

#[test]
fn safe_fallback_requires_invalid_settings_and_exact_registered_binding() {
    let registered = hotkey::OperationalHotkey::registered(SAFE_FALLBACK_HOTKEY);

    assert!(!should_use_safe_fallback(
        SAFE_FALLBACK_HOTKEY,
        true,
        &registered
    ));
    for shortcut in [
        SAFE_FALLBACK_HOTKEY,
        "shift+control+Space",
        "Ctrl+Shift+Space",
    ] {
        assert!(should_use_safe_fallback(shortcut, false, &registered));
    }
    for shortcut in [
        "Control+Shift+Enter",
        "Control+Shift+Meta+Space",
        "not-a-hotkey",
    ] {
        assert!(!should_use_safe_fallback(shortcut, false, &registered));
    }

    for operational_hotkey in [
        hotkey::OperationalHotkey::Inactive,
        hotkey::OperationalHotkey::Unknown,
        hotkey::OperationalHotkey::registered("Control+Shift+Enter"),
        hotkey::OperationalHotkey::registered("control+shift+space"),
    ] {
        assert!(!should_use_safe_fallback(
            "shift+control+Space",
            false,
            &operational_hotkey
        ));
    }
}

#[test]
fn safe_fallback_never_activates_for_valid_settings() {
    let registered = hotkey::OperationalHotkey::registered(SAFE_FALLBACK_HOTKEY);

    assert!(!should_use_safe_fallback(
        "shift+control+Space",
        true,
        &registered
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn startup_replaces_reserved_hotkey_with_safe_operational_fallback() {
    let (candidate, error) = startup_hotkey_candidate("Super+Q");

    assert_eq!(
        candidate,
        sagascript_core::settings::Settings::default().hotkey
    );
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("reserved for Quit on macOS")));
}

#[cfg(target_os = "macos")]
#[test]
fn startup_replaces_saved_cut_hotkey_with_safe_operational_fallback() {
    let (candidate, error) = startup_hotkey_candidate("Super+X");

    assert_eq!(
        candidate,
        sagascript_core::settings::Settings::default().hotkey
    );
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("reserved for Cut on macOS")));
}

#[cfg(target_os = "macos")]
#[test]
fn startup_replaces_saved_bold_hotkey_with_safe_operational_fallback() {
    let (candidate, error) = startup_hotkey_candidate("Super+B");

    assert_eq!(
        candidate,
        sagascript_core::settings::Settings::default().hotkey
    );
    assert!(error
        .as_deref()
        .is_some_and(|message| message.contains("reserved for Bold Text on macOS")));
}

// -- tray_label --

/// The texts the hotkey path puts in the `error` event and the controller's
/// last error. #360 changed them: a timeout and a join error lost their
/// "Transcription failed: " prefix, and a cancellation is "Transcription
/// cancelled." instead of the old "Pianissimo transcription cancelled" or
/// "Cancelled". Pinned so a later change is deliberate.
#[test]
fn the_hotkey_path_reports_a_failed_dictation_with_these_texts() {
    use sagascript_core::error::DictationError;

    assert_eq!(
        dictation_failure_text(&DictationError::Cancelled),
        "Transcription cancelled."
    );
    assert_eq!(
        dictation_failure_text(&dictation_timed_out()),
        "Transcription timed out after 60s (inference aborted)"
    );
    assert_eq!(
        dictation_failure_text(&dictation_task_failed("task 7 panicked")),
        "Task join error: task 7 panicked"
    );
    assert_eq!(
        dictation_failure_text(&DictationError::TranscriptionFailed("no model".into())),
        "Transcription failed: no model"
    );
}

/// The Swift host reports the model file by absolute path; the event the
/// overlay and Settings show must not carry the user's home directory.
#[test]
fn an_engine_failure_reaches_the_error_event_without_the_users_path() {
    use sagascript_core::error::DictationError;

    let engine = DictationError::engine(
        "engine_model_load_failed",
        false,
        "Pianissimo engine: Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: boom",
    );
    assert_eq!(
        dictation_failure_text(&engine),
        "Pianissimo engine: Failed to load model at <path>/foo.mlmodelc: boom"
    );
}
