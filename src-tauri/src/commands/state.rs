//! Read-only queries of the dictation state.

use tauri::State;

use super::{lock_controller, SharedController};
use crate::app_controller::AppState;

#[tauri::command]
pub async fn get_state(controller: State<'_, SharedController>) -> Result<AppState, String> {
    let ctrl = lock_controller(&controller);
    Ok(ctrl.state())
}

/// Last Pianissimo engine load state for the overlay (`unknown` before any load).
#[tauri::command]
pub async fn get_engine_load_state() -> Result<serde_json::Value, String> {
    Ok(sagascript_core::transcription::pianissimo_backend::current_load_state_payload())
}

#[tauri::command]
pub async fn get_last_transcription(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = lock_controller(&controller);
    Ok(ctrl.last_transcription().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn get_last_error(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = lock_controller(&controller);
    Ok(ctrl.last_error().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn is_model_ready(controller: State<'_, SharedController>) -> Result<bool, String> {
    let ctrl = lock_controller(&controller);
    Ok(ctrl.is_model_ready())
}
