use arboard::Clipboard;
#[cfg(target_os = "macos")]
#[path = "macos_clipboard.rs"]
mod macos_clipboard;
// enigo is the input simulator on macOS/Windows. On Linux its X11 backend leaves
// the Control modifier unmapped (paste silently fails), so we shell out to
// xdotool instead and don't depend on enigo there.
#[cfg(not(target_os = "linux"))]
use enigo::{Enigo, Keyboard, Settings as EnigoSettings, Key, Direction};
use tracing::info;
#[cfg(target_os = "macos")]
use tracing::warn;

use sagascript_core::error::DictationError;

/// Service for pasting transcribed text into the active application
/// Uses clipboard + simulated Cmd+V (macOS) or Ctrl+V (Windows/Linux)
pub struct PasteService;

impl PasteService {
    pub fn new() -> Self {
        Self
    }

    /// Paste text into the currently active application
    /// Saves and restores previous clipboard contents
    pub fn paste(&self, text: &str) -> Result<(), DictationError> {
        if text.is_empty() {
            return Ok(());
        }

        let mut clipboard =
            Clipboard::new().map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;

        // On macOS, preserve every pasteboard item and declared representation
        // (RTF, images, file URLs, custom app formats, etc.), not just plain text.
        #[cfg(target_os = "macos")]
        let saved_pasteboard = macos_clipboard::snapshot();

        // Other platforms currently use arboard's portable text API.
        #[cfg(not(target_os = "macos"))]
        let saved_text = clipboard.get_text().ok();

        // Set new text
        clipboard
            .set_text(text)
            .map_err(|e| DictationError::PasteError(format!("Failed to set clipboard: {e}")))?;

        #[cfg(target_os = "macos")]
        let owned_change_count = macos_clipboard::change_count();

        info!("Text copied to clipboard ({} chars)", text.len());

        // Check accessibility permission on macOS
        #[cfg(target_os = "macos")]
        {
            info!("Checking accessibility permission...");
            let trusted = crate::platform::macos::is_accessibility_trusted();
            info!("Accessibility trusted: {trusted}");
            if let Err(error) = validate_accessibility(trusted) {
                // Background dictation must never summon a system permission
                // prompt. Permission is requested only from an explicit UI action.
                warn!("Accessibility permission not granted — leaving text on clipboard.");
                return Err(error);
            }
        }

        // Small delay to let the previously-focused app regain focus
        info!("Waiting 50ms before paste simulation...");
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Simulate paste keystroke
        info!("Simulating paste keystroke...");
        simulate_paste()?;

        // Schedule clipboard restore
        #[cfg(not(target_os = "macos"))]
        let saved = saved_text;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));

            #[cfg(target_os = "macos")]
            if let Some(snapshot) = saved_pasteboard {
                // Do not clobber clipboard content copied by the user or target
                // application while the synthetic paste was in flight.
                let _ = macos_clipboard::restore_if_unchanged(snapshot, owned_change_count);
            }

            #[cfg(not(target_os = "macos"))]
            if let Some(text) = saved {
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
        });

        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn validate_accessibility(trusted: bool) -> Result<(), DictationError> {
    if trusted {
        Ok(())
    } else {
        Err(DictationError::AccessibilityPermissionDenied)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::validate_accessibility;
    use sagascript_core::error::DictationError;

    #[test]
    fn denied_background_paste_stops_after_copy_without_prompt_or_restore() {
        assert!(matches!(
            validate_accessibility(false),
            Err(DictationError::AccessibilityPermissionDenied)
        ));
        assert!(validate_accessibility(true).is_ok());
    }
}

#[cfg(not(target_os = "linux"))]
fn simulate_paste() -> Result<(), DictationError> {
    let mut enigo = Enigo::new(&EnigoSettings::default())
        .map_err(|e| DictationError::PasteError(format!("Failed to create input simulator: {e}")))?;

    #[cfg(target_os = "macos")]
    let modifier = Key::Meta; // Cmd

    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| DictationError::PasteError(format!("Key press failed: {e}")))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| DictationError::PasteError(format!("Key click failed: {e}")))?;
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| DictationError::PasteError(format!("Key release failed: {e}")))?;

    info!("Paste keystroke simulated");
    Ok(())
}

/// Linux: simulate Ctrl+V via the `xdotool` CLI. enigo's X11 backend leaves the
/// Control modifier unmapped, so we shell out instead. Requires `xdotool` and an
/// X11 session (Wayland needs `ydotool`, which is not yet wired up).
#[cfg(target_os = "linux")]
fn simulate_paste() -> Result<(), DictationError> {
    use std::process::Command;

    let status = Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+v"])
        .status()
        .map_err(|e| {
            DictationError::PasteError(format!(
                "Failed to launch xdotool (install it with `apt install xdotool`): {e}"
            ))
        })?;

    if !status.success() {
        return Err(DictationError::PasteError(format!(
            "xdotool exited unsuccessfully ({status})"
        )));
    }

    info!("Paste keystroke simulated (xdotool)");
    Ok(())
}
