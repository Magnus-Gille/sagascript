use super::*;

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
