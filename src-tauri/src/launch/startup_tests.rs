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
fn startup_keeps_a_valid_hotkey() {
    let requested = "Option+Space";
    let (candidate, error) = startup_hotkey_candidate(requested);

    assert_eq!(candidate, requested);
    assert!(error.is_none());
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
