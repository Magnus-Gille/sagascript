use tauri_plugin_global_shortcut::GlobalShortcutExt;

fn bare_extended_function_key_number(shortcut: &str) -> Option<u8> {
    // Strict parity with src/lib/hotkey.js canUseBareHotkey (/^F(\d{1,2})$/i):
    // at most two ASCII digits, so "F013" or "F1a" never qualify.
    let normalized = shortcut.trim().to_ascii_lowercase();
    let digits = normalized.strip_prefix('f')?;
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u8 = digits.parse().ok()?;
    (13..=24).contains(&number).then_some(number)
}

fn is_bare_extended_function_key(shortcut: &str) -> bool {
    bare_extended_function_key_number(shortcut).is_some()
}

fn uses_native_macos_monitor(shortcut: &str) -> bool {
    cfg!(target_os = "macos") && is_bare_extended_function_key(shortcut)
}

fn plugin_shortcuts(shortcuts: &[String]) -> Vec<&str> {
    shortcuts
        .iter()
        .map(String::as_str)
        .filter(|shortcut| !uses_native_macos_monitor(shortcut))
        .collect()
}

pub fn register_shortcuts(app: &tauri::AppHandle, shortcuts: &[String]) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let native_monitor_error = if shortcuts
        .iter()
        .any(|shortcut| uses_native_macos_monitor(shortcut))
    {
        if !super::bare_function_key_monitor_installed() {
            Some("The macOS F13-F24 event monitor is unavailable; restart Sagascript".to_string())
        } else if !crate::platform::macos::is_accessibility_trusted() {
            Some(
                "Bare F13-F24 shortcuts require Accessibility permission on macOS. Open Sagascript Settings and approve Accessibility first"
                    .to_string(),
            )
        } else {
            None
        }
    } else {
        None
    };

    #[cfg(target_os = "macos")]
    if let Some(error) = native_monitor_error {
        return Err(error);
    }

    let plugin_shortcuts = plugin_shortcuts(shortcuts);
    if !plugin_shortcuts.is_empty() {
        app.global_shortcut()
            .register_multiple(plugin_shortcuts)
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

pub fn unregister_shortcuts(app: &tauri::AppHandle, shortcuts: &[String]) -> Result<(), String> {
    let plugin_shortcuts = plugin_shortcuts(shortcuts);
    if plugin_shortcuts.is_empty() {
        Ok(())
    } else {
        app.global_shortcut()
            .unregister_multiple(plugin_shortcuts)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{is_bare_extended_function_key, plugin_shortcuts};

    #[test]
    fn identifies_exactly_bare_f13_through_f24() {
        for shortcut in ["F13", "f20", " F24 "] {
            assert!(is_bare_extended_function_key(shortcut), "{shortcut}");
        }
        for shortcut in [
            "F12", "F25", "Shift+F13", "F13+F14", "Space", "F013", "F0013", "F1a",
        ] {
            assert!(!is_bare_extended_function_key(shortcut), "{shortcut}");
        }
    }

    #[test]
    fn non_macos_builds_leave_shortcuts_for_the_plugin() {
        if !cfg!(target_os = "macos") {
            let shortcuts = vec!["F13".to_string(), "Control+Space".to_string()];
            assert_eq!(plugin_shortcuts(&shortcuts), vec!["F13", "Control+Space"]);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_routes_only_bare_extended_function_keys_around_the_plugin() {
        let shortcuts = vec![
            "F13".to_string(),
            " f24 ".to_string(),
            "Shift+F13".to_string(),
            "Control+Space".to_string(),
        ];
        assert_eq!(
            plugin_shortcuts(&shortcuts),
            vec!["Shift+F13", "Control+Space"]
        );
    }
}
