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

#[test]
fn elapsed_ms_if_success_only_marks_successful_operations_complete() {
    let start = Instant::now();
    assert!(elapsed_ms_if_success::<(), ()>(&Ok(()), start).is_some());
    assert!(elapsed_ms_if_success::<(), ()>(&Err(()), start).is_none());
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
