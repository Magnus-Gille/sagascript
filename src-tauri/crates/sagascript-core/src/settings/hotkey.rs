/// Validate a hotkey string against the format accepted by Tauri's
/// global-shortcut crate and Sagascript's platform safety rules.
///
/// Format: `[Modifier+]*Key` (case-insensitive).
pub fn validate_hotkey(value: &str) -> Result<(), String> {
    validate_hotkey_for_platform(value, CURRENT_PLATFORM)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HotkeyPlatform {
    MacOS,
    Other,
}

const CURRENT_PLATFORM: HotkeyPlatform = if cfg!(target_os = "macos") {
    HotkeyPlatform::MacOS
} else {
    HotkeyPlatform::Other
};

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

    if modifier_tokens.is_empty() {
        return Err(format!(
            "Invalid hotkey '{}': at least one modifier is required. \
             Example: 'Control+Space', 'Option+Space'",
            value
        ));
    }

    let uses_command = modifier_tokens.iter().any(|token| {
        matches!(
            token.to_lowercase().as_str(),
            "super"
                | "command"
                | "cmd"
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
            // Like Cut, Bold Text is a ubiquitous editor command. A global
            // bare Command+B hotkey intercepts it in the active application,
            // and can leave toggle-mode recording running with no obvious way
            // to stop it from that editor.
            "b" | "keyb" if uses_command && modifier_tokens.len() == 1 => {
                Some(("B", "Bold Text"))
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
    fn macos_bold_shortcut_is_reserved_for_every_command_alias() {
        for shortcut in [
            "Command+B",
            "Cmd+KeyB",
            "Super+B",
            "CmdOrCtrl+B",
            "CommandOrControl+b",
            "  cOmMaNd + keyB  ",
        ] {
            let error = validate_hotkey_for_platform(shortcut, HotkeyPlatform::MacOS).unwrap_err();
            assert!(
                error.contains("reserved for Bold Text on macOS"),
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
    fn command_q_policy_is_platform_specific() {
        assert!(validate_hotkey_for_platform("Command+Q", HotkeyPlatform::Other).is_ok());
    }
}
