//! Update bookkeeping for the GUI: the pending-result guard, the recovery draft
//! file and the update preparation handshake.

use std::sync::Arc;

use tauri::State;

use super::{lock_controller, SharedController};
use crate::command_error::CommandError;

#[tauri::command]
pub async fn set_update_result_pending(
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    result_id: String,
    pending: bool,
) -> Result<(), CommandError> {
    Ok(activity.set_result_pending(&result_id, pending)?)
}

#[tauri::command]
pub async fn get_update_result_pending(
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    result_id: String,
) -> Result<bool, CommandError> {
    if result_id.is_empty() || result_id.len() > 128 {
        return Err(crate::update_activity::ActivityError::InvalidResultId.into());
    }
    Ok(activity.is_result_pending(&result_id))
}

#[tauri::command]
pub async fn acknowledge_update_result(
    controller: State<'_, SharedController>,
    expected_text: String,
) -> Result<bool, CommandError> {
    Ok(lock_controller(&controller).acknowledge_update_result(&expected_text)?)
}

fn update_recovery_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, CommandError> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map(|directory| directory.join("update-recovery.json"))
        .map_err(|error| {
            CommandError::Io(format!("Could not locate local recovery storage: {error}"))
        })
}

#[tauri::command]
pub async fn save_update_recovery(
    app: tauri::AppHandle,
    payload: serde_json::Value,
) -> Result<(), CommandError> {
    if payload
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(CommandError::InvalidInput(
            "Unsupported recovery draft format.".into(),
        ));
    }
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| CommandError::Unavailable(error.to_string()))?
            .as_secs();
        Ok(crate::update_recovery::save(path, 1, timestamp, &payload)?)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed(format!("Could not finish saving recovery draft: {error}"))
    })?
}

fn update_recovery_payload(
    snapshot: Option<crate::update_recovery::RecoverySnapshot>,
) -> Result<Option<serde_json::Value>, CommandError> {
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    if snapshot.schema_version != 1 {
        return Err(CommandError::InvalidInput(
            "Unsupported recovery draft envelope; the stored draft was retained.".into(),
        ));
    }
    if !snapshot.payload.is_object() {
        return Err(CommandError::InvalidInput(
            "Recovery draft payload must be an object; the stored draft was retained.".into(),
        ));
    }
    Ok(Some(snapshot.payload))
}

#[tauri::command]
pub async fn load_update_recovery(
    app: tauri::AppHandle,
) -> Result<Option<serde_json::Value>, CommandError> {
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        update_recovery_payload(crate::update_recovery::load(path)?)
    })
    .await
    .map_err(|error| {
        CommandError::task_failed(format!("Could not finish loading recovery draft: {error}"))
    })?
}

#[tauri::command]
pub async fn clear_update_recovery(app: tauri::AppHandle) -> Result<(), CommandError> {
    let path = update_recovery_path(&app)?;
    tauri::async_runtime::spawn_blocking(move || Ok(crate::update_recovery::clear(path)?))
        .await
        .map_err(|error| {
            CommandError::task_failed(format!("Could not finish clearing recovery draft: {error}"))
        })?
}

#[tauri::command]
pub fn complete_update_preparation(
    preparation: State<'_, crate::update_activity::UpdatePreparation>,
    nonce: String,
    error: Option<String>,
) -> Result<(), CommandError> {
    Ok(preparation.complete(&nonce, error.map_or(Ok(()), Err))?)
}

#[cfg(test)]
#[path = "update_recovery_tests.rs"]
mod tests;
