/// Validate a hotkey string against the format accepted by Tauri's
/// global-shortcut crate and Sagascript's platform safety rules.
///
/// Format: `[Modifier+]*Key` (case-insensitive).
pub fn validate_hotkey(value: &str) -> Result<(), String> {
    validate_hotkey_for_platform(value, CURRENT_PLATFORM)
}

/// Canonical representation used for equality and duplicate detection.
/// Tauri accepts several aliases for the same physical shortcut; storing the
/// user's spelling is useful for display, but comparisons must collapse those
/// aliases so two profiles cannot claim the same key combination.
pub fn canonical_hotkey(value: &str) -> Result<String, String> {
    validate_hotkey(value)?;
    Ok(canonical_hotkey_for_platform(value, CURRENT_PLATFORM))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeyPlatform {
    MacOS,
    Windows,
    Other,
}

const CURRENT_PLATFORM: HotkeyPlatform = if cfg!(target_os = "macos") {
    HotkeyPlatform::MacOS
} else if cfg!(target_os = "windows") {
    HotkeyPlatform::Windows
} else {
    HotkeyPlatform::Other
};

fn canonical_hotkey_for_platform(value: &str, platform: HotkeyPlatform) -> String {
    let tokens: Vec<String> = value
        .split('+')
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let (modifiers, key) = tokens.split_at(tokens.len() - 1);
    let mut modifiers: Vec<&str> = modifiers
        .iter()
        .map(|modifier| match modifier.as_str() {
            "control" | "ctrl" => "control",
            "alt" | "option" => "alt",
            "super" | "command" | "cmd" | "meta" => "super",
            "commandorcontrol" | "commandorctrl" | "cmdorctrl" | "cmdorcontrol" => {
                if platform == HotkeyPlatform::MacOS { "super" } else { "control" }
            }
            "shift" => "shift",
            _ => unreachable!("validated modifier"),
        })
        .collect();
    modifiers.sort_unstable();
    modifiers.dedup();

    let raw_key = key[0].as_str();
    let key = match raw_key {
        "up" => "arrowup".to_string(),
        "down" => "arrowdown".to_string(),
        "left" => "arrowleft".to_string(),
        "right" => "arrowright".to_string(),
        "esc" => "escape".to_string(),
        other if other.len() == 1 && other.as_bytes()[0].is_ascii_lowercase() => {
            format!("key{other}")
        }
        other if other.len() == 1 && other.as_bytes()[0].is_ascii_digit() => {
            format!("digit{other}")
        }
        other => other.to_string(),
    };
    let mut parts: Vec<String> = modifiers.into_iter().map(str::to_string).collect();
    parts.push(key);
    parts.join("+")
}

fn validate_hotkey_for_platform(value: &str, platform: HotkeyPlatform) -> Result<(), String> {
    const MODIFIERS: &[&str] = &[
        "shift",
        "control",
        "ctrl",
        "alt",
        "option",
        "super",
        "command",
        "cmd",
        // `global-hotkey` 0.8 (keyboard-types 0.8) renamed the Command/Win
        // modifier from `super` to `meta`, and shortcut events are reported
        // with that spelling. Accept it so events match saved profiles.
        "meta",
        "commandorcontrol",
        "commandorctrl",
        "cmdorctrl",
        "cmdorcontrol",
    ];

    const KEYS: &[&str] = &[
        // Letters
        "keya",
        "keyb",
        "keyc",
        "keyd",
        "keye",
        "keyf",
        "keyg",
        "keyh",
        "keyi",
        "keyj",
        "keyk",
        "keyl",
        "keym",
        "keyn",
        "keyo",
        "keyp",
        "keyq",
        "keyr",
        "keys",
        "keyt",
        "keyu",
        "keyv",
        "keyw",
        "keyx",
        "keyy",
        "keyz",
        "a",
        "b",
        "c",
        "d",
        "e",
        "f",
        "g",
        "h",
        "i",
        "j",
        "k",
        "l",
        "m",
        "n",
        "o",
        "p",
        "q",
        "r",
        "s",
        "t",
        "u",
        "v",
        "w",
        "x",
        "y",
        "z",
        // Digits
        "digit0",
        "digit1",
        "digit2",
        "digit3",
        "digit4",
        "digit5",
        "digit6",
        "digit7",
        "digit8",
        "digit9",
        "0",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        // Function keys
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "f11",
        "f12",
        "f13",
        "f14",
        "f15",
        "f16",
        "f17",
        "f18",
        "f19",
        "f20",
        "f21",
        "f22",
        "f23",
        "f24",
        // Navigation
        "home",
        "end",
        "pageup",
        "pagedown",
        "arrowup",
        "arrowdown",
        "arrowleft",
        "arrowright",
        "up",
        "down",
        "left",
        "right",
        // Editing
        "backspace",
        "delete",
        "enter",
        "tab",
        "space",
        "escape",
        "esc",
        // Special characters
        "backquote",
        "`",
        "backslash",
        "\\",
        "bracketleft",
        "[",
        "bracketright",
        "]",
        "comma",
        ",",
        "equal",
        "=",
        "minus",
        "-",
        "period",
        ".",
        "quote",
        "'",
        "semicolon",
        ";",
        "slash",
        "/",
        // ISO section key: `§` left of `1` on Swedish, UK and other Apple ISO
        // keyboards (macOS virtual key 0x0A), `<`/`\` next to left Shift on
        // PC ISO keyboards. Only registrable once the patched `global-hotkey`
        // (tauri-apps/global-hotkey#216) is in use; see src-tauri/Cargo.toml.
        "intlbackslash",
        // Lock & control
        "capslock",
        "numlock",
        "scrolllock",
        "pause",
        "pausebreak",
        "printscreen",
        "insert",
        // Numpad
        "numpad0",
        "numpad1",
        "numpad2",
        "numpad3",
        "numpad4",
        "numpad5",
        "numpad6",
        "numpad7",
        "numpad8",
        "numpad9",
        "num0",
        "num1",
        "num2",
        "num3",
        "num4",
        "num5",
        "num6",
        "num7",
        "num8",
        "num9",
        "numpadadd",
        "numadd",
        "numpadplus",
        "numplus",
        "numpadsubtract",
        "numsubtract",
        "numpadmultiply",
        "nummultiply",
        "numpaddivide",
        "numdivide",
        "numpaddecimal",
        "numdecimal",
        "numpadequal",
        "numequal",
        "numpadenter",
        "numenter",
        // Media
        "mediaplay",
        "mediapause",
        "mediaplaypause",
        "mediastop",
        "mediatracknext",
        "mediatrackprevious",
        "mediatrackprev",
        "audiovolumeup",
        "volumeup",
        "audiovolumedown",
        "volumedown",
        "audiovolumemute",
        "volumemute",
    ];

    let tokens: Vec<&str> = value.split('+').map(str::trim).collect();

    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return Err(
            "Invalid hotkey: empty or malformed. Example: 'Control+Shift+Space'".to_string(),
        );
    }

    // Last token must be a key; preceding tokens must be modifiers.
    let (modifier_tokens, key_token) = tokens.split_at(tokens.len() - 1);
    let key = key_token[0].to_lowercase();

    if !KEYS.contains(&key.as_str()) {
        if MODIFIERS.contains(&key.as_str()) {
            return Err(format!(
                "Invalid hotkey '{}': '{}' is a modifier, not a key. \
                 A hotkey must end with a key (e.g. Space, A, F1). \
                 Example: 'Control+Shift+Space'",
                value, key_token[0]
            ));
        }
        return Err(format!(
            "Invalid hotkey '{}': unknown key '{}'. \
             Examples of valid keys: Space, A, F1, Enter, Tab, ArrowUp",
            value, key_token[0]
        ));
    }

    for &token in modifier_tokens {
        let lower = token.to_lowercase();
        if !MODIFIERS.contains(&lower.as_str()) {
            if KEYS.contains(&lower.as_str()) {
                return Err(format!(
                    "Invalid hotkey '{}': '{}' is a key, not a modifier. \
                     Modifiers must come before the key. \
                     Valid modifiers: Control, Shift, Alt/Option, Command/Super, CmdOrCtrl",
                    value, token
                ));
            }
            return Err(format!(
                "Invalid hotkey '{}': unknown modifier '{}'. \
                 Valid modifiers: Control, Shift, Alt/Option, Command/Super, CmdOrCtrl",
                value, token
            ));
        }
    }

    let function_key_number = key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok());

    // Reject parsed keys that cannot reach either the platform shortcut
    // backend or Sagascript's native macOS bare-function-key monitor.
    if function_key_number.is_some_and(|number| (13..=24).contains(&number)) {
        let unsupported_error = match platform {
            HotkeyPlatform::MacOS
                if !modifier_tokens.is_empty()
                    && function_key_number.is_some_and(|number| number >= 21) =>
            {
                Some("F21-F24 are supported without modifiers on macOS, but the modified forms cannot be registered reliably")
            }
            HotkeyPlatform::MacOS | HotkeyPlatform::Windows | HotkeyPlatform::Other => None,
        };
        if let Some(error) = unsupported_error {
            return Err(format!("Invalid hotkey '{}': {}.", value, error));
        }
    }

    let bare_function_key_range = match platform {
        HotkeyPlatform::MacOS => Some(13..=24),
        HotkeyPlatform::Windows => Some(13..=24),
        HotkeyPlatform::Other => None,
    };
    let is_allowed_bare_function_key = modifier_tokens.is_empty()
        && function_key_number.is_some_and(|number| {
            bare_function_key_range.is_some_and(|range| range.contains(&number))
        });

    if modifier_tokens.is_empty() && !is_allowed_bare_function_key {
        return Err(format!(
            "Invalid hotkey '{}': at least one modifier is required. \
             Example: 'Control+Space', 'Option+Space'. {} may be used without modifiers.",
            value,
            match platform {
                HotkeyPlatform::MacOS => "F13-F24",
                HotkeyPlatform::Windows => "F13-F24",
                HotkeyPlatform::Other => "No keys",
            }
        ));
    }

    let uses_command = modifier_tokens.iter().any(|token| {
        matches!(
            token.to_lowercase().as_str(),
            "super"
                | "command"
                | "cmd"
                | "meta"
                | "commandorcontrol"
                | "commandorctrl"
                | "cmdorctrl"
                | "cmdorcontrol"
        )
    });

    if platform == HotkeyPlatform::MacOS {
        let reserved_shortcut = match key.as_str() {
            // Keep the existing stricter Quit guard: any shortcut containing a
            // Command alias and Q is unsafe to expose as a global hotkey.
            "q" | "keyq" if uses_command => Some(("Q", "Quit")),
            // Cut is only Command+X itself. Modified variants such as
            // Command+Shift+X are distinct macOS shortcuts and remain valid.
            "x" | "keyx" if uses_command && modifier_tokens.len() == 1 => {
                Some(("X", "Cut"))
            }
            _ => None,
        };
        if let Some((key, action)) = reserved_shortcut {
            return Err(format!(
                "Invalid hotkey '{value}': Command+{key} is reserved for {action} on macOS. \
                 Choose a different shortcut, such as 'Control+Shift+Space'."
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_quit_shortcut_is_reserved_for_every_command_alias() {
        for shortcut in [
            "Command+Q",
            "Cmd+KeyQ",
            "Super+Q",
            "Meta+Q",
            "CmdOrCtrl+Q",
            "CommandOrControl+q",
            "Control+Super+Shift+Q",
            "  cOmMaNd + keyQ  ",
        ] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).unwrap_err();
            assert!(
                error.contains("reserved for Quit on macOS"),
                "unexpected error for {shortcut}: {error}"
            );
        }
    }

    #[test]
    fn macos_cut_shortcut_is_reserved_for_every_command_alias() {
        for shortcut in [
            "Command+X",
            "Cmd+KeyX",
            "Super+X",
            "CmdOrCtrl+X",
            "CommandOrControl+x",
            "  cOmMaNd + keyX  ",
        ] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).unwrap_err();
            assert!(
                error.contains("reserved for Cut on macOS"),
                "unexpected error for {shortcut}: {error}"
            );
        }
    }

    #[test]
    fn macos_non_quit_shortcuts_remain_valid() {
        for shortcut in [
            "Control+Q",
            "Shift+Q",
            "Command+A",
            "Super+Shift+X",
            "Command+Shift+X",
            "Control+Super+X",
        ] {
            assert!(
                validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).is_ok(),
                "should remain valid: {shortcut}"
            );
        }
    }

    #[test]
    fn iso_section_key_is_accepted_with_modifiers_on_every_platform() {
        for platform in [
            HotkeyPlatform::MacOS,
            HotkeyPlatform::Windows,
            HotkeyPlatform::Other,
        ] {
            for shortcut in [
                "Command+IntlBackslash",
                "Control+intlbackslash",
                "Shift+Alt+IntlBackslash",
            ] {
                assert!(
                    validate_hotkey_for_platform(shortcut, platform).is_ok(),
                    "should be valid: {shortcut}"
                );
            }
            let error = validate_hotkey_for_platform("IntlBackslash", platform).unwrap_err();
            assert!(
                error.contains("modifier is required"),
                "unexpected error for bare IntlBackslash: {error}"
            );
        }
        assert_eq!(
            canonical_hotkey_for_platform("Cmd+IntlBackslash", HotkeyPlatform::MacOS),
            "super+intlbackslash"
        );
    }

    #[test]
    fn meta_is_the_event_spelling_of_the_command_modifier() {
        // global-hotkey reports a registered `Super+IntlBackslash` shortcut
        // back as `meta+IntlBackslash`; both must resolve to the same profile.
        for platform in [
            HotkeyPlatform::MacOS,
            HotkeyPlatform::Windows,
            HotkeyPlatform::Other,
        ] {
            assert!(validate_hotkey_for_platform("meta+IntlBackslash", platform).is_ok());
            assert_eq!(
                canonical_hotkey_for_platform("meta+IntlBackslash", platform),
                canonical_hotkey_for_platform("Super+IntlBackslash", platform)
            );
            assert_eq!(
                canonical_hotkey_for_platform("shift+meta+Space", platform),
                canonical_hotkey_for_platform("Command+Shift+Space", platform)
            );
        }
    }

    #[test]
    fn command_q_policy_is_platform_specific() {
        assert!(validate_hotkey_for_platform("Command+Q", HotkeyPlatform::Other).is_ok());
    }

    #[test]
    fn canonical_hotkey_collapses_aliases_order_case_and_key_names() {
        assert_eq!(
            canonical_hotkey_for_platform("Shift+OPTION+A", HotkeyPlatform::MacOS),
            canonical_hotkey_for_platform("Alt + Shift + KeyA", HotkeyPlatform::MacOS)
        );
        assert_eq!(
            canonical_hotkey_for_platform("CmdOrCtrl+Space", HotkeyPlatform::MacOS),
            canonical_hotkey_for_platform("Command+Space", HotkeyPlatform::MacOS)
        );
        assert_eq!(
            canonical_hotkey_for_platform("F13", HotkeyPlatform::MacOS),
            canonical_hotkey_for_platform("f13", HotkeyPlatform::MacOS)
        );
    }

    #[test]
    fn bare_extended_function_keys_follow_platform_registration_support() {
        for shortcut in ["F13", "f17", "F20", "F21", "F24"] {
            assert!(
                validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).is_ok(),
                "macOS should accept {shortcut} without a modifier"
            );
        }

        for shortcut in ["F13", "f20", "F21", "F24"] {
            assert!(
                validate_hotkey_for_platform(shortcut, HotkeyPlatform::Windows).is_ok(),
                "Windows should accept {shortcut} without a modifier"
            );
        }
    }

    #[test]
    fn macos_only_accepts_f21_through_f24_without_modifiers() {
        for shortcut in ["Shift+F21", "Control+f22", "Shift+F23", "Command+F24"] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).unwrap_err();
            assert!(
                error.contains("supported without modifiers on macOS"),
                "unexpected error for {shortcut}: {error}"
            );
        }

        assert!(validate_hotkey_for_platform("F24", HotkeyPlatform::MacOS).is_ok());
        assert!(validate_hotkey_for_platform("Control+F24", HotkeyPlatform::Windows).is_ok());
    }

    #[test]
    fn linux_preserves_modified_extended_function_keys_but_rejects_bare_ones() {
        for shortcut in ["Control+F13", "Shift+F20", "Alt+F24"] {
            assert!(
                validate_hotkey_for_platform(shortcut, HotkeyPlatform::Other).is_ok(),
                "modified shortcut should remain valid on Linux: {shortcut}"
            );
        }

        for shortcut in ["F13", "F24"] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::Other).unwrap_err();
            assert!(
                error.contains("modifier is required"),
                "unexpected error for {shortcut}: {error}"
            );
        }
    }

    #[test]
    fn ordinary_bare_keys_and_f1_through_f12_still_require_modifiers() {
        for shortcut in ["Space", "A", "7", "F1", "f12", "ArrowUp"] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).unwrap_err();
            assert!(
                error.contains("modifier is required"),
                "unexpected error for {shortcut}: {error}"
            );
        }

        for shortcut in ["Control+Space", "Option+A", "Shift+F12"] {
            assert!(validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).is_ok());
        }
    }
}
