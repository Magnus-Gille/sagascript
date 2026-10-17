//! Update bookkeeping for the GUI: the pending-result guard, the recovery draft
//! file and the update preparation handshake.

use std::sync::Arc;

use tauri::State;

use super::{lock_controller, SharedController};

#[tauri::command]
pub async fn set_update_result_pending(
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    result_id: String,
    pending: bool,
) -> Result<(), String> {
    activity.set_result_pending(&result_id, pending)
}

#[tauri::command]
pub async fn get_update_result_pending(
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    result_id: String,
) -> Result<bool, String> {
    if result_id.is_empty() || result_id.len() > 128 {
        return Err("Invalid transcription result ID.".into());
    }
    Ok(activity.is_result_pending(&result_id))
}

#[tauri::command]
pub async fn acknowledge_update_result(
    controller: State<'_, SharedController>,
    expected_text: String,
) -> Result<bool, String> {
    lock_controller(&controller).acknowledge_update_result(&expected_text)
}

fn update_recovery_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("update-recovery.json"))
        .map_err(|error| format!("Could not locate local recovery storage: {error}"))
}

#[tauri::command]
pub async fn save_update_recovery(
    app: tauri::AppHandle,
    payload: serde_json::Value,
) -> Result<(), String> {
    if payload
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("Unsupported recovery draft format.".into());
    }
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs();
        crate::update_recovery::save(path, 1, timestamp, &payload)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Could not finish saving recovery draft: {error}"))?
}

fn update_recovery_payload(
    snapshot: Option<crate::update_recovery::RecoverySnapshot>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    if snapshot.schema_version != 1 {
        return Err("Unsupported recovery draft envelope; the stored draft was retained.".into());
    }
    if !snapshot.payload.is_object() {
        return Err(
            "Recovery draft payload must be an object; the stored draft was retained.".into(),
        );
    }
    Ok(Some(snapshot.payload))
}

#[tauri::command]
pub async fn load_update_recovery(
    app: tauri::AppHandle,
) -> Result<Option<serde_json::Value>, String> {
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::update_recovery::load(path)
            .map_err(|error| error.to_string())
            .and_then(update_recovery_payload)
    })
    .await
    .map_err(|error| format!("Could not finish loading recovery draft: {error}"))?
}

#[tauri::command]
pub async fn clear_update_recovery(app: tauri::AppHandle) -> Result<(), String> {
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::update_recovery::clear(path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Could not finish clearing recovery draft: {error}"))?
}

#[tauri::command]
pub fn complete_update_preparation(
    preparation: State<'_, crate::update_activity::UpdatePreparation>,
    nonce: String,
    error: Option<String>,
) -> Result<(), String> {
    preparation.complete(&nonce, error.map_or(Ok(()), Err))
}

#[cfg(test)]
#[path = "update_recovery_tests.rs"]
mod tests;
