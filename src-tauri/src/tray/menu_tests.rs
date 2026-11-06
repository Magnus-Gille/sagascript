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

#[test]
fn menu_item_ids_choose_their_actions() {
    assert_eq!(menu_action("quit"), Some(MenuAction::Quit));
    assert_eq!(
        menu_action("settings"),
        Some(MenuAction::Settings {
            tab: None,
            site: "tray_menu_settings"
        })
    );
    assert_eq!(
        menu_action("transcribe_file"),
        Some(MenuAction::Settings {
            tab: Some("transcribe"),
            site: "tray_menu_transcribe_file"
        })
    );
    assert_eq!(
        menu_action("manage_profiles"),
        Some(MenuAction::Settings {
            tab: Some("dictate"),
            site: "tray_menu_manage_profiles"
        })
    );
    assert_eq!(
        menu_action("check_for_updates"),
        Some(MenuAction::CheckForUpdates)
    );
    // Status lines and the build identity are not actions.
    for inert in [
        "status",
        "update_status",
        "build_info",
        "profile_shortcut_x",
        "",
    ] {
        assert_eq!(menu_action(inert), None, "{inert}");
    }
}

#[test]
fn record_menu_item_opens_the_record_window() {
    assert_eq!(RECORD_ITEM_ID, "record");
    assert_eq!(menu_action(RECORD_ITEM_ID), Some(MenuAction::Record));
}
