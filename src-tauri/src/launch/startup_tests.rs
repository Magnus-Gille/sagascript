// The `startup_*` tests use the test-only `startup_hotkey_candidate`, not `register_startup_hotkeys`;
// the `health` tests call the real `startup_hotkey_health`.
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
            (fallback, Some(error.to_string()))
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

fn health(
    validation: Option<&str>,
    registration: Option<&str>,
    cleanup: Option<&str>,
) -> (Option<String>, hotkey::OperationalHotkey) {
    startup_hotkey_health(
        validation.map(str::to_string),
        registration.map(str::to_string),
        cleanup.map(str::to_string),
        vec![SAFE_FALLBACK_HOTKEY.to_string()],
    )
}

fn registered() -> hotkey::OperationalHotkey {
    hotkey::OperationalHotkey::registered(SAFE_FALLBACK_HOTKEY)
}

#[test]
fn a_clean_startup_registration_is_healthy_and_registered() {
    assert_eq!(health(None, None, None), (None, registered()));
}

#[test]
fn a_valid_fallback_registration_reports_the_invalid_settings_but_is_registered() {
    assert_eq!(
        health(Some("bad profiles"), None, None),
        (
            Some("bad profiles; using safe fallback 'Control+Shift+Space'".to_string()),
            registered()
        )
    );
}

#[test]
fn a_failed_registration_is_inactive_and_reports_its_error() {
    assert_eq!(
        health(None, Some("in use"), None),
        (
            Some("in use".to_string()),
            hotkey::OperationalHotkey::Inactive
        )
    );
    assert_eq!(
        health(Some("bad profiles"), Some("in use"), None),
        (
            Some("bad profiles; fallback 'Control+Shift+Space' also failed: in use".to_string()),
            hotkey::OperationalHotkey::Inactive
        )
    );
}

#[test]
fn a_failed_cleanup_after_a_failed_registration_leaves_the_os_state_unknown() {
    assert_eq!(
        health(None, Some("in use"), Some("stuck")),
        (
            Some(
                "registration failed: in use; partial-registration cleanup failed: stuck"
                    .to_string()
            ),
            hotkey::OperationalHotkey::Unknown
        )
    );
    assert_eq!(
        health(Some("bad profiles"), Some("in use"), Some("stuck")),
        (
            Some("bad profiles; registration failed: in use; partial-registration cleanup failed: stuck".to_string()),
            hotkey::OperationalHotkey::Unknown
        )
    );
}

/// Cleanup only runs after a registration failure, so this combination does not
/// happen; if it ever did, the OS state is unknown and the error is shown (it
/// used to be a panic arm, which would have aborted startup).
#[test]
fn a_cleanup_failure_without_a_registration_error_is_unknown_not_registered() {
    assert_eq!(
        health(None, None, Some("stuck")),
        (
            Some(
                "partial-registration cleanup failed without a registration error: stuck"
                    .to_string()
            ),
            hotkey::OperationalHotkey::Unknown
        )
    );
    assert_eq!(
        health(Some("bad profiles"), None, Some("stuck")),
        (
            Some("bad profiles; partial-registration cleanup failed without a registration error: stuck".to_string()),
            hotkey::OperationalHotkey::Unknown
        )
    );
}
