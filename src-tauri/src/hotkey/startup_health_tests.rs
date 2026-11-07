use super::*;

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
