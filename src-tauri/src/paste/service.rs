#[cfg(target_os = "linux")]
use arboard::Clipboard;
use std::borrow::Cow;
#[cfg(target_os = "macos")]
#[path = "macos_clipboard.rs"]
mod macos_clipboard;
#[cfg(any(target_os = "windows", test))]
#[path = "windows_clipboard.rs"]
mod windows_clipboard;
// enigo is the input simulator on macOS/Windows. On Linux its X11 backend leaves
// the Control modifier unmapped (paste silently fails), so we shell out to
// xdotool instead and don't depend on enigo there.
#[cfg(not(target_os = "linux"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};
use tracing::info;
#[cfg(target_os = "macos")]
use tracing::warn;

use sagascript_core::error::DictationError;

/// Service for pasting transcribed text into the active application
/// Uses clipboard + simulated Cmd+V (macOS) or Ctrl+V (Windows/Linux)
pub struct PasteService;

fn paste_payload(text: &str) -> Cow<'_, str> {
    if text.is_empty() || text.chars().last().is_some_and(char::is_whitespace) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(format!("{text} "))
    }
}

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
        let text = paste_payload(text);

        #[cfg(target_os = "linux")]
        let mut clipboard = Clipboard::new()
            .map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;

        // On macOS, preserve every pasteboard item and declared representation
        // (RTF, images, file URLs, custom app formats, etc.), not just plain text.
        #[cfg(target_os = "macos")]
        let saved_pasteboard = macos_clipboard::snapshot();

        // Linux currently uses arboard's portable text API.
        #[cfg(target_os = "linux")]
        let saved_text = clipboard.get_text().ok();

        // Set new text. On macOS the native write returns the pasteboard
        // generation created by our clear, closing the race that a later
        // changeCount sample could accidentally attribute to another app.
        #[cfg(target_os = "macos")]
        let owned_change_count = match macos_clipboard::set_temporary_text(text.as_ref()) {
            Ok(generation) => generation,
            Err(error) => {
                if let Some(snapshot) = saved_pasteboard {
                    // A failed write may already have cleared the pasteboard.
                    // Restore only if no other app has taken ownership since.
                    let _ = macos_clipboard::restore_if_unchanged(
                        snapshot,
                        error.owned_generation,
                    );
                }
                return Err(DictationError::PasteError(format!(
                    "Failed to set clipboard: {}",
                    error.message
                )));
            }
        };

        #[cfg(target_os = "linux")]
        clipboard
            .set_text(text.as_ref())
            .map_err(|e| DictationError::PasteError(format!("Failed to set clipboard: {e}")))?;
        #[cfg(target_os = "windows")]
        let saved_windows = windows_clipboard::set_temporary_text(text.as_ref())
            .map_err(|error| DictationError::PasteError(format!("Failed to set clipboard: {error}")))?;

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
        #[cfg(not(target_os = "windows"))]
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Windows SendInput inherits physically held modifiers. In particular,
        // releasing E before Alt would otherwise dispatch Alt+Ctrl+V. Wait on
        // the paste worker, without releasing keys the user is still holding.
        #[cfg(target_os = "windows")]
        wait_for_windows_modifiers()?;

        // Simulate paste keystroke
        info!("Simulating paste keystroke...");
        simulate_paste()?;

        // Schedule clipboard restore
        #[cfg(target_os = "linux")]
        let saved = saved_text;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));

            #[cfg(target_os = "macos")]
            if let Some(snapshot) = saved_pasteboard {
                // Do not clobber clipboard content copied by the user or target
                // application while the synthetic paste was in flight.
                let _ = macos_clipboard::restore_if_unchanged(snapshot, owned_change_count);
            }

            #[cfg(target_os = "windows")]
            match windows_clipboard::restore_if_unchanged(saved_windows) {
                Ok(false) => tracing::debug!("Clipboard restore skipped: generation changed or no text snapshot"),
                Err(error) => tracing::warn!("Clipboard restore failed: {error}"),
                Ok(true) => {}
            }

            #[cfg(target_os = "linux")]
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
    use super::{paste_payload, validate_accessibility};
    use sagascript_core::error::DictationError;

    #[test]
    fn denied_background_paste_stops_after_copy_without_prompt_or_restore() {
        assert!(matches!(
            validate_accessibility(false),
            Err(DictationError::AccessibilityPermissionDenied)
        ));
        assert!(validate_accessibility(true).is_ok());
    }

    #[test]
    fn paste_payload_adds_one_separator_between_dictations() {
        assert_eq!(paste_payload("Första meningen."), "Första meningen. ");
        assert_eq!(paste_payload("Nästa fras"), "Nästa fras ");
    }

    #[test]
    fn paste_payload_preserves_existing_trailing_whitespace() {
        assert_eq!(paste_payload("Redan klart. "), "Redan klart. ");
        assert_eq!(paste_payload("Ny rad\n"), "Ny rad\n");
        assert_eq!(paste_payload(""), "");
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
    #[cfg(target_os = "windows")]
    let paste_key = Key::Other(0x56); // VK_V, independent of layout/Unicode packets
    #[cfg(target_os = "macos")]
    let paste_key = Key::Unicode('v');
    let click_result = enigo
        .key(paste_key, Direction::Click)
        .map_err(|e| DictationError::PasteError(format!("Key click failed: {e}")));
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| DictationError::PasteError(format!("Key release failed: {e}")))?;

    info!("Paste keystroke simulated");
    click_result
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetForegroundWindow() -> isize;
}

#[cfg(target_os = "windows")]
fn wait_for_windows_modifiers() -> Result<(), DictationError> {
    let target = unsafe { GetForegroundWindow() };
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(crate::paste_completion::WINDOWS_MODIFIER_WAIT_MS);
    while [0x10, 0x11, 0x12, 0x5B, 0x5C].iter().any(|key| unsafe { GetAsyncKeyState(*key) < 0 }) {
        if std::time::Instant::now() >= deadline {
            return Err(DictationError::PasteError("Release the shortcut keys and paste with Ctrl+V. The recognized text is on the clipboard.".into()));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if target == 0 || unsafe { GetForegroundWindow() } != target {
        return Err(DictationError::PasteError("The focused window changed. Select the intended text field and paste with Ctrl+V.".into()));
    }
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
