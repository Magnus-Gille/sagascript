use arboard::Clipboard;
use enigo::{Enigo, Keyboard, Settings as EnigoSettings, Key, Direction};
use tracing::{info, warn};

use crate::error::DictationError;

/// Service for pasting transcribed text into the active application
/// Uses clipboard + simulated Cmd+V (macOS) or Ctrl+V (Windows)
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

        // Save current clipboard text
        let saved_text = clipboard.get_text().ok();

        // Set new text
        clipboard
            .set_text(text)
            .map_err(|e| DictationError::PasteError(format!("Failed to set clipboard: {e}")))?;

        info!("Text copied to clipboard ({} chars)", text.len());

        // Check accessibility permission on macOS
        #[cfg(target_os = "macos")]
        {
            if !crate::platform::macos::is_accessibility_trusted() {
                warn!("Accessibility permission not granted, text copied to clipboard only");
                crate::platform::macos::request_accessibility_permission();
                return Err(DictationError::AccessibilityPermissionDenied);
            }
        }

        // Simulate paste keystroke
        simulate_paste()?;

        // Schedule clipboard restore
        let saved = saved_text;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Some(text) = saved {
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(text);
                }
            }
        });

        Ok(())
    }
}

fn simulate_paste() -> Result<(), DictationError> {
    let mut enigo = Enigo::new(&EnigoSettings::default())
        .map_err(|e| DictationError::PasteError(format!("Failed to create input simulator: {e}")))?;

    #[cfg(target_os = "macos")]
    let modifier = Key::Meta; // Cmd

    #[cfg(target_os = "windows")]
    let modifier = Key::Control;

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
