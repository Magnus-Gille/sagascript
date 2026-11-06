use sagascript_core::sync::lock_or_recover;
use tauri::Manager;

use super::{SharedStatusItem, STATUS_ITEM_LOCK};
use crate::app_controller::AppState;
use crate::commands::{lock_controller, SharedController};
use crate::hotkey;
use crate::recording_session::SharedRecorder;

/// Pure state -> (tooltip, title, menu_text) mapping for the tray, extracted
/// so the sticky-warning behavior can be unit tested without a running Tauri
/// app. Two inputs outrank the dictation `state`:
///
/// * `recorder_active` (the audio recorder, SAGA-564, is starting or running)
///   wins over everything. Nothing may record without a visible sign, so no
///   other state, and not the hotkey warning, may paper over it. A failed
///   hotkey does not disappear either: its text is composed into the
///   recording text, so both facts stay on screen.
/// * `hotkey_failed` wins over every normal dictation state
///   (idle/recording/transcribing/loading_model): once the hotkey is known to
///   be unregistered, no ordinary state transition is allowed to silently
///   paper back over "Idle" -- that's the whole point of making the warning
///   sticky.
fn tray_label(
    state: &str,
    hotkey_failed: bool,
    recorder_active: bool,
) -> (&'static str, &'static str, &'static str) {
    if recorder_active {
        return if hotkey_failed {
            (
                "Sagascript - Recording audio... (hotkey unavailable)",
                "●",
                "Recording audio... (hotkey unavailable)",
            )
        } else {
            ("Sagascript - Recording audio...", "●", "Recording audio...")
        };
    }
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

/// Whether the audio recorder is starting or running. A recorder that is not
/// managed (never in the app) counts as idle.
fn recorder_active(app: &tauri::AppHandle) -> bool {
    app.try_state::<SharedRecorder>()
        .is_some_and(|recorder| recorder.is_active())
}

/// The `update_tray_status` name of a dictation state.
fn dictation_state_name(state: AppState) -> &'static str {
    match state {
        AppState::Recording => "recording",
        AppState::Transcribing => "transcribing",
        AppState::Idle | AppState::Error => "idle",
    }
}

/// Update the tray tooltip, title, and status menu item to reflect current
/// state. Consults the audio recorder and the process-wide hotkey health flag
/// on every call, so a running recording is always shown and a broken hotkey
/// registration renders as a sticky "Hotkey unavailable" warning that later
/// state changes (recording -> idle, model-preload status, etc.) cannot
/// silently overwrite. This is the one function that draws the status, which
/// is how a dictation's `update_tray_status(_, "idle")` cannot erase the
/// recorder's indicator.
pub(crate) fn update_tray_status(app: &tauri::AppHandle, state: &str) {
    let hotkey_failed = app.state::<hotkey::HotkeyHealth>().is_failed();
    let (tooltip, title, menu_text) = tray_label(state, hotkey_failed, recorder_active(app));

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
        let _ = tray.set_title(Some(title));
    }

    set_status_menu_text(app, &format!("Sagascript - {menu_text}"));
}

/// Redraw the tray after the audio recorder started or stopped: the status is
/// derived again from the recorder and from the dictation state the controller
/// reports. Tray calls belong on the main thread (ADR 0003): the callers run
/// elsewhere and go through `dispatch_to_main`.
pub(crate) fn refresh_tray_status(app: &tauri::AppHandle) {
    let state = {
        let controller: tauri::State<'_, SharedController> = app.state();
        let state = lock_controller(&controller).state();
        state
    };
    update_tray_status(app, dictation_state_name(state));
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
/// the warning text here. A running recording is guarded the same way: its
/// indicator must stay, so the last result waits for the recording to end.
pub(crate) fn update_tray_last_result(app: &tauri::AppHandle, text: &str) {
    if app.state::<hotkey::HotkeyHealth>().is_failed() || recorder_active(app) {
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
