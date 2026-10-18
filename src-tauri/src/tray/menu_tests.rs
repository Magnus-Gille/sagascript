use super::*;
use sagascript_core::settings::Language;

#[test]
fn profile_menu_label_explains_language_shortcut_and_selection() {
    let profile = sagascript_core::settings::HotkeyProfile {
        id: "swedish".to_string(),
        name: "Svenska".to_string(),
        shortcut: "Super+Shift+S".to_string(),
        language: Language::Swedish,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    };

    assert_eq!(
        profile_menu_label(&profile, true),
        "✓ Svenska — Swedish · ⇧⌘S"
    );
    assert_eq!(
        profile_menu_label(&profile, false),
        "Svenska — Swedish · ⇧⌘S"
    );
}

#[test]
fn profile_menu_label_keeps_non_macos_shortcuts_readable() {
    let profile = sagascript_core::settings::HotkeyProfile {
        id: "english".to_string(),
        name: "English".to_string(),
        shortcut: "Control+Alt+Space".to_string(),
        language: Language::English,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    };

    assert_eq!(
        profile_menu_label(&profile, false),
        "English — English · ⌃⌥Space"
    );
}

#[test]
fn profile_menu_shortcut_formats_command_or_control_aliases() {
    let expected = if cfg!(target_os = "macos") {
        "⇧⌘Space"
    } else {
        "⌃⇧Space"
    };
    for shortcut in [
        "CommandOrControl+Shift+Space",
        "CommandOrCtrl+Shift+Space",
        "CmdOrCtrl+Shift+Space",
        "CmdOrControl+Shift+Space",
    ] {
        assert_eq!(format_menu_shortcut(shortcut), expected);
    }
}

#[test]
fn profile_menu_shortcut_formats_arrow_keys() {
    assert_eq!(format_menu_shortcut("Control+ArrowUp"), "⌃↑");
    assert_eq!(format_menu_shortcut("Alt+Left"), "⌥←");
}
