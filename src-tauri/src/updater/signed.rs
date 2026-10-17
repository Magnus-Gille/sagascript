//! Signed download and install of an update (macOS), and the pure decisions
//! around it, which the unit tests cover on every platform.

#[cfg(target_os = "macos")]
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
use tauri::Manager;
#[cfg(target_os = "macos")]
use tracing::warn;

#[cfg(target_os = "macos")]
use super::{set_update_menu, UpdateMenuMode};
#[cfg(target_os = "macos")]
use crate::commands::{lock_controller, SharedController};
#[cfg(target_os = "macos")]
use crate::events::{self, EmitEvent};
#[cfg(target_os = "macos")]
use crate::{dispatch_to_main, update_activity, update_recovery};

#[cfg(target_os = "macos")]
pub(super) fn finish_update_check_error(app: &tauri::AppHandle, error: String) {
    warn!("Update check failed: {error}");
    set_update_menu(
        app,
        "Couldn't check for updates",
        "Try Again…",
        true,
        UpdateMenuMode::Idle,
    );
}

#[cfg(target_os = "macos")]
async fn prepare_update_recovery(app: &tauri::AppHandle) -> Result<(), String> {
    if app.get_webview_window("settings").is_some() {
        let preparation: tauri::State<'_, update_activity::UpdatePreparation> = app.state();
        let (nonce, response) = preparation.begin();
        if let Err(error) = app.emit_event_to("settings", events::event::UPDATE_PREPARING, &nonce) {
            preparation.cancel(&nonce);
            return Err(format!(
                "Could not ask Settings to save recovery drafts: {error}"
            ));
        }
        let result = tokio::time::timeout(Duration::from_secs(30), response).await;
        preparation.cancel(&nonce);
        return match result {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Settings closed before recovery drafts were saved.".into()),
            Err(_) => {
                Err("Timed out while saving recovery drafts. The update was not installed.".into())
            }
        };
    }

    // Background dictation can run before Settings has ever been opened. Keep
    // its last result without creating or focusing a window during an update.
    let last_text = {
        let controller: tauri::State<'_, SharedController> = app.state();
        let ctrl = lock_controller(&controller);
        if ctrl.is_update_result_pending("live-dictation") {
            ctrl.last_transcription().map(str::to_owned)
        } else {
            None
        }
    };
    let Some(last_text) = last_text.filter(|text| !text.trim().is_empty()) else {
        return Ok(());
    };
    let path = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not locate local recovery storage: {error}"))?
        .join("update-recovery.json");
    tauri::async_runtime::spawn_blocking(move || {
        let mut payload = update_recovery::load(&path)
            .map_err(|error| error.to_string())?
            .map_or_else(
                || serde_json::json!({
                    "schema_version": 1,
                    "saved_at": chrono::Utc::now().to_rfc3339(),
                    "dictation": null,
                    "files": [],
                    "meetings": []
                }),
                |snapshot| snapshot.payload,
            );
        if native_recovery_conflicts(&payload, &last_text) {
            return Err("A different unsaved dictation already exists in update recovery. Save or discard that draft before retrying the update.".into());
        }
        payload["dictation"] = serde_json::json!({ "text": last_text });
        payload["saved_at"] = serde_json::json!(chrono::Utc::now().to_rfc3339());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs();
        update_recovery::save(path, 1, timestamp, &payload).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Could not finish saving dictation draft: {error}"))?
}

#[cfg(any(target_os = "macos", test))]
fn native_recovery_conflicts(payload: &serde_json::Value, text: &str) -> bool {
    payload
        .get("dictation")
        .and_then(|dictation| dictation.get("text"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|existing| !existing.trim().is_empty() && existing != text)
}

#[cfg(target_os = "macos")]
pub(super) async fn install_signed_update(
    app: tauri::AppHandle,
    update: tauri_plugin_updater::Update,
    version: semver::Version,
) {
    use crate::update_activity::{ExclusiveError, UpdateActivity};

    set_update_menu(
        &app,
        format!("Downloading update v{version}…"),
        "Downloading update…",
        false,
        UpdateMenuMode::Installing,
    );

    let mut downloaded = 0_u64;
    let mut last_percent = None;
    let app_for_progress = app.clone();
    let progress_version = version.clone();
    let bytes = match update
        .download(
            move |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                let percent = total
                    .filter(|total| *total > 0)
                    .map(|total| (downloaded.saturating_mul(100) / total).min(100) as u8);
                if percent != last_percent {
                    last_percent = percent;
                    let action = percent.map_or_else(
                        || "Downloading update…".to_string(),
                        |percent| format!("Downloading update… {percent}%"),
                    );
                    set_update_menu(
                        &app_for_progress,
                        format!("Downloading update v{progress_version}…"),
                        action,
                        false,
                        UpdateMenuMode::Installing,
                    );
                }
            },
            || {},
        )
        .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!("Signed updater download/verification failed: {error}");
            set_update_menu(
                &app,
                "Update download failed",
                "Try Again…",
                true,
                UpdateMenuMode::Idle,
            );
            return;
        }
    };

    let activity: Arc<UpdateActivity> = app.state::<Arc<UpdateActivity>>().inner().clone();
    let work_wait_started = Instant::now();
    loop {
        match activity.try_exclusive(false) {
            Ok(update_permit) => {
                set_update_menu(
                    &app,
                    "Saving recoverable drafts…",
                    "Preparing update…",
                    false,
                    UpdateMenuMode::Installing,
                );
                if let Err(error) = prepare_update_recovery(&app).await {
                    warn!("Could not safely prepare update: {error}");
                    let _ = app.emit_event_to("settings", events::event::UPDATE_ABORTED, &error);
                    drop(update_permit);
                    set_update_menu(
                        &app,
                        "Could not save drafts for update",
                        "Try Again…",
                        true,
                        UpdateMenuMode::Idle,
                    );
                    return;
                }
                set_update_menu(
                    &app,
                    format!("Installing update v{version}…"),
                    "Installing update…",
                    false,
                    UpdateMenuMode::Installing,
                );
                if let Err(error) = update.install(&bytes) {
                    warn!("Signed updater installation failed: {error}");
                    let _ = app.emit_event_to(
                        "settings",
                        events::event::UPDATE_ABORTED,
                        &error.to_string(),
                    );
                    drop(update_permit);
                    set_update_menu(
                        &app,
                        "Update installation failed",
                        "Try Again…",
                        true,
                        UpdateMenuMode::Idle,
                    );
                    return;
                }

                set_update_menu(
                    &app,
                    format!("Update v{version} installed; restarting Sagascript…"),
                    "Restarting…",
                    false,
                    UpdateMenuMode::Installing,
                );
                dispatch_to_main(&app, move |app| {
                    let _update_permit = update_permit;
                    app.restart();
                });
                return;
            }
            Err(ExclusiveError::ResultsPending) => {
                warn!("Update activity unexpectedly rejected the recovery path");
                set_update_menu(
                    &app,
                    "Update preparation blocked",
                    "Try Again…",
                    true,
                    UpdateMenuMode::Idle,
                );
                return;
            }
            Err(ExclusiveError::WorkActive) => {
                if update_work_wait_expired(work_wait_started, Instant::now()) {
                    warn!("Update wait exceeded ten minutes; current work remains untouched");
                    set_update_menu(
                        &app,
                        "Update wait timed out",
                        "Try Again…",
                        true,
                        UpdateMenuMode::Idle,
                    );
                    return;
                }
                set_update_menu(
                    &app,
                    "Waiting for current dictation or transcription to finish…",
                    "Waiting for current work…",
                    false,
                    UpdateMenuMode::Installing,
                );
            }
            Err(error) => {
                warn!(?error, "Could not safely apply update");
                set_update_menu(
                    &app,
                    "Update preparation failed",
                    "Try Again…",
                    true,
                    UpdateMenuMode::Idle,
                );
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[cfg(any(target_os = "macos", test))]
fn update_work_wait_expired(start: Instant, now: Instant) -> bool {
    now.saturating_duration_since(start) >= Duration::from_secs(600)
}

#[cfg(test)]
#[path = "signed_tests.rs"]
mod tests;
