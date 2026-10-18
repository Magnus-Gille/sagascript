use sagascript_core::sync::lock_or_recover;
use tauri::Manager;

use super::{SharedStatusItem, STATUS_ITEM_LOCK};
use crate::hotkey;

/// Pure state -> (tooltip, title, menu_text) mapping for the tray, extracted
/// so the "hotkey unavailable" sticky-warning behavior can be unit tested
/// without a running Tauri app. `hotkey_failed` must win over every normal
/// state (idle/recording/transcribing/loading_model): once the hotkey is
/// known to be unregistered, no ordinary state transition is allowed to
/// silently paper back over "Idle" — that's the whole point of making the
/// warning sticky.
fn tray_label(state: &str, hotkey_failed: bool) -> (&'static str, &'static str, &'static str) {
    if hotkey_failed {
        return ("Sagascript - Hotkey unavailable", "!", "Hotkey unavailable");
    }
    match state {
        "recording" => ("Sagascript - Recording...", "●", "Recording..."),
        "loading_model" => ("Sagascript - Loading model...", "…", "Loading model..."),
        "transcribing" => ("Sagascript - Transcribing...", "…", "Transcribing..."),
        _ => ("Sagascript", "S", "Idle"),
    }
}

/// Update the tray tooltip, title, and status menu item to reflect current
/// state. Consults the process-wide hotkey health flag on every call so a
/// broken hotkey registration renders as a sticky "Hotkey unavailable"
/// warning that later state changes (recording -> idle, model-preload status,
/// etc.) cannot silently overwrite.
pub(crate) fn update_tray_status(app: &tauri::AppHandle, state: &str) {
    let hotkey_failed = app.state::<hotkey::HotkeyHealth>().is_failed();
    let (tooltip, title, menu_text) = tray_label(state, hotkey_failed);

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
        let _ = tray.set_title(Some(title));
    }

    set_status_menu_text(app, &format!("Sagascript - {menu_text}"));
}

/// Truncate transcription text for tray display, cutting on a char boundary.
fn truncate_for_tray(text: &str) -> String {
    if text.len() > 60 {
        let cut = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 57)
            .last()
            .unwrap_or(0);
        format!("{}...", &text[..cut])
    } else {
        text.to_string()
    }
}

/// Update the tray status menu item and tooltip to show the last transcription.
///
/// Guarded the same way as [`update_tray_status`]: if the hotkey is currently
/// unregistered, showing "Last: <transcription>" would silently overwrite the
/// sticky "Hotkey unavailable" warning (the exact trap called out in review —
/// this call always follows an `update_tray_status(_, "idle")` in the success
/// path). Delegate to `update_tray_status` in that case instead of duplicating
/// the warning text here.
pub(crate) fn update_tray_last_result(app: &tauri::AppHandle, text: &str) {
    if app.state::<hotkey::HotkeyHealth>().is_failed() {
        update_tray_status(app, "idle");
        return;
    }

    let display = truncate_for_tray(text);

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&format!("Sagascript\nLast: {display}")));
    }

    set_status_menu_text(app, &format!("\u{2713} {display}"));
}

/// Helper to update the status menu item text
fn set_status_menu_text(app: &tauri::AppHandle, text: &str) {
    let guard = lock_or_recover(&app.state::<SharedStatusItem>(), STATUS_ITEM_LOCK).clone();
    if let Some(item) = guard {
        let _ = item.set_text(text);
    }
}

#[cfg(test)]
#[path = "status_tests.rs"]
mod tests;
