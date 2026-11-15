//! Signed download and install of an update (macOS), behind the orchestrator's
//! `CheckSource` / `UpdateProvider` / `Recovery` seams, plus the pure recovery
//! decision the unit tests cover on every platform.

#[cfg(target_os = "macos")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use tauri::Manager;
#[cfg(target_os = "macos")]
use tracing::warn;

#[cfg(target_os = "macos")]
use super::activity::ExclusiveLease;
#[cfg(target_os = "macos")]
use super::orchestrator::{
    ActivityAttempt, BoxFuture, CheckSource, CheckedRelease, InstallOutcome, MenuSink,
    Orchestrator, ProgressSink, Recovery, UpdateProvider, PROGRESS_UNKNOWN,
};
#[cfg(target_os = "macos")]
use super::recovery as update_recovery;
#[cfg(target_os = "macos")]
use crate::commands::{lock_controller, SharedController};
#[cfg(target_os = "macos")]
use crate::dispatch_to_main;
#[cfg(target_os = "macos")]
use crate::events::{self, EmitEvent};

#[cfg(target_os = "macos")]
use sagascript_core::sync::lock_or_recover;

// The stashed `Update` is recovered as-is after a panic (ADR 0005): it is a
// single `Option` swap, and the next check overwrites or clears it.
#[cfg(target_os = "macos")]
const SIGNED_CHECK_LOCK: &str = "update.signed_check";

/// Check seam for the signed updater manifest: one successful check stashes
/// the plugin's `Update` for a later install offer.
#[cfg(target_os = "macos")]
pub(super) struct SignedCheck {
    app: tauri::AppHandle,
    update: Mutex<Option<tauri_plugin_updater::Update>>,
}

#[cfg(target_os = "macos")]
impl SignedCheck {
    pub(super) fn new(app: tauri::AppHandle) -> Self {
        Self {
            app,
            update: Mutex::new(None),
        }
    }

    pub(super) fn take_update(&self) -> Option<tauri_plugin_updater::Update> {
        lock_or_recover(&self.update, SIGNED_CHECK_LOCK).take()
    }
}

#[cfg(target_os = "macos")]
impl CheckSource for SignedCheck {
    fn check(&self) -> BoxFuture<'_, Result<Option<CheckedRelease>, String>> {
        Box::pin(async {
            match super::updates::check_signed_update(&self.app).await? {
                None => Ok(None),
                Some(update) => {
                    let version = semver::Version::parse(update.version.trim_start_matches('v'))
                        .map_err(|error| format!("invalid updater version: {error}"))?;
                    *lock_or_recover(&self.update, SIGNED_CHECK_LOCK) = Some(update);
                    Ok(Some(CheckedRelease {
                        version: version.to_string(),
                        install_offered: true,
                    }))
                }
            }
        })
    }
}

#[cfg(target_os = "macos")]
pub(super) struct SignedUpdate {
    app: tauri::AppHandle,
    update: tauri_plugin_updater::Update,
}

#[cfg(target_os = "macos")]
impl SignedUpdate {
    fn new(app: tauri::AppHandle, update: tauri_plugin_updater::Update) -> Self {
        Self { app, update }
    }
}

#[cfg(target_os = "macos")]
impl UpdateProvider for SignedUpdate {
    fn download(&self, on_progress: ProgressSink) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async {
            let mut downloaded = 0_u64;
            let last_percent = Arc::new(std::sync::atomic::AtomicU8::new(PROGRESS_UNKNOWN));
            self.update
                .download(
                    move |chunk, total| {
                        downloaded = downloaded.saturating_add(chunk as u64);
                        let percent = total
                            .filter(|total| *total > 0)
                            .map(|total| (downloaded.saturating_mul(100) / total).min(100) as u8);
                        let byte = percent.unwrap_or(PROGRESS_UNKNOWN);
                        if last_percent.swap(byte, std::sync::atomic::Ordering::Relaxed) != byte {
                            on_progress((byte != PROGRESS_UNKNOWN).then_some(byte));
                        }
                    },
                    || {},
                )
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), String> {
        self.update.install(bytes).map_err(|error| {
            let _ = self.app.emit_event_to(
                "settings",
                events::event::UPDATE_ABORTED,
                &error.to_string(),
            );
            error.to_string()
        })
    }
}

#[cfg(target_os = "macos")]
pub(super) struct DraftRecovery {
    app: tauri::AppHandle,
}

#[cfg(target_os = "macos")]
impl DraftRecovery {
    fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

#[cfg(target_os = "macos")]
impl Recovery for DraftRecovery {
    type Permit = super::activity::ExclusiveLease;

    fn try_begin(&self) -> ActivityAttempt<ExclusiveLease> {
        use super::activity::{ExclusiveError, UpdateActivity};

        let activity: Arc<UpdateActivity> = self.app.state::<Arc<UpdateActivity>>().inner().clone();
        match activity.try_exclusive(false) {
            Ok(permit) => ActivityAttempt::Ready(permit),
            Err(ExclusiveError::WorkActive) => ActivityAttempt::WorkActive,
            Err(ExclusiveError::ResultsPending) => ActivityAttempt::Blocked,
            Err(ExclusiveError::AlreadyExclusive) => {
                ActivityAttempt::Failed("an update is already being applied".into())
            }
        }
    }

    fn prepare<'a>(&'a self, _permit: &ExclusiveLease) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async {
            prepare_update_recovery(&self.app)
                .await
                .inspect_err(|error| {
                    let _ =
                        self.app
                            .emit_event_to("settings", events::event::UPDATE_ABORTED, error);
                })
        })
    }
}

/// Drives the signed install through the orchestrator, then restarts with the
/// permit held so no new work starts before the process is replaced.
#[cfg(target_os = "macos")]
pub(super) async fn install_signed_update<S: MenuSink>(
    orchestrator: &Orchestrator<S>,
    app: tauri::AppHandle,
    update: tauri_plugin_updater::Update,
    version: &str,
) {
    let provider = SignedUpdate::new(app.clone(), update);
    let recovery = DraftRecovery::new(app.clone());
    match orchestrator.install(provider, recovery, version).await {
        InstallOutcome::RestartReady { permit } => {
            dispatch_to_main(&app, move |app| {
                let _permit = permit;
                app.restart();
            });
        }
        InstallOutcome::Stopped { failure } => {
            warn!("Update installation stopped: {failure}");
        }
    }
}

#[cfg(target_os = "macos")]
async fn prepare_update_recovery(app: &tauri::AppHandle) -> Result<(), String> {
    use std::time::Duration;

    use super::activity::UpdatePreparation;

    if app.get_webview_window("settings").is_some() {
        let preparation: tauri::State<'_, UpdatePreparation> = app.state();
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

#[cfg(test)]
#[path = "signed_tests.rs"]
mod tests;
