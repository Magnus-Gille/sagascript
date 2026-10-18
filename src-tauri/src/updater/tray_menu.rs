//! Tray presentation and the Tauri-side update seams.
//!
//! This module owns every user-facing update string in the app. The
//! orchestration in [`crate::updater`] only ever speaks in [`UpdatePhase`]s;
//! [`TrayUpdateMenu`] turns them into tray texts, so the wording can change
//! without touching the install sequence, and the sequence can be tested
//! without a tray.

#[cfg(target_os = "macos")]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use sagascript_core::sync::lock_or_recover;
#[cfg(target_os = "macos")]
use tauri::Manager;

use super::{MenuSink, UpdateFailure, UpdatePhase};
#[cfg(target_os = "macos")]
use crate::events::EmitEvent;

/// One tray rendering of a phase.
pub(crate) struct TrayTexts {
    pub(crate) status: String,
    pub(crate) action: String,
    pub(crate) enabled: bool,
    pub(crate) mode: crate::UpdateMenuMode,
}

/// Thin handle over the tray's update items.
pub struct TrayUpdateMenu {
    app: tauri::AppHandle,
}

impl TrayUpdateMenu {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }

    /// Pure phase → text mapping, asserted directly by the tests.
    pub(crate) fn texts_for(phase: &UpdatePhase) -> TrayTexts {
        let retry = |status: &str| TrayTexts {
            status: status.to_owned(),
            action: "Try Again…".to_owned(),
            enabled: true,
            mode: crate::UpdateMenuMode::Idle,
        };
        match phase {
            UpdatePhase::Checking => TrayTexts {
                status: "Checking for updates…".to_owned(),
                action: crate::UPDATE_CHECK_ACTION.to_owned(),
                enabled: false,
                mode: crate::UpdateMenuMode::Checking,
            },
            UpdatePhase::UpToDate => TrayTexts {
                status: "Sagascript is up to date".to_owned(),
                action: "Check Again…".to_owned(),
                enabled: true,
                mode: crate::UpdateMenuMode::Idle,
            },
            UpdatePhase::Available {
                version,
                install_offered,
            } => TrayTexts {
                status: format!("Update available — v{version}"),
                action: if *install_offered {
                    format!("Install and Restart v{version}…")
                } else {
                    format!("Open Release v{version}…")
                },
                enabled: true,
                mode: crate::UpdateMenuMode::Available(version.clone()),
            },
            UpdatePhase::Downloading { version, percent } => TrayTexts {
                status: format!("Downloading update v{version}…"),
                action: percent.map_or_else(
                    || "Downloading update…".to_owned(),
                    |percent| format!("Downloading update… {percent}%"),
                ),
                enabled: false,
                mode: crate::UpdateMenuMode::Installing,
            },
            UpdatePhase::WaitingForWork { .. } => TrayTexts {
                status: "Waiting for current dictation or transcription to finish…".to_owned(),
                action: "Waiting for current work…".to_owned(),
                enabled: false,
                mode: crate::UpdateMenuMode::Installing,
            },
            UpdatePhase::PreparingDrafts { .. } => TrayTexts {
                status: "Saving recoverable drafts…".to_owned(),
                action: "Preparing update…".to_owned(),
                enabled: false,
                mode: crate::UpdateMenuMode::Installing,
            },
            UpdatePhase::Installing { version } => TrayTexts {
                status: format!("Installing update v{version}…"),
                action: "Installing update…".to_owned(),
                enabled: false,
                mode: crate::UpdateMenuMode::Installing,
            },
            UpdatePhase::Installed { version } => TrayTexts {
                status: format!("Update v{version} installed; restarting Sagascript…"),
                action: "Restarting…".to_owned(),
                enabled: false,
                mode: crate::UpdateMenuMode::Installing,
            },
            UpdatePhase::Failed { failure } => match failure {
                UpdateFailure::Check => retry("Couldn't check for updates"),
                UpdateFailure::Download => retry("Update download failed"),
                UpdateFailure::WaitTimedOut => retry("Update wait timed out"),
                UpdateFailure::PreparationBlocked => retry("Update preparation blocked"),
                UpdateFailure::PreparationFailed => retry("Update preparation failed"),
                UpdateFailure::RecoveryConflict => retry("Could not save drafts for update"),
                UpdateFailure::Install => retry("Update installation failed"),
            },
        }
    }
}

impl MenuSink for TrayUpdateMenu {
    fn show(&self, phase: &UpdatePhase) {
        let texts = Self::texts_for(phase);
        crate::set_update_menu(
            &self.app,
            texts.status,
            texts.action,
            texts.enabled,
            texts.mode,
        );
    }
}

/// Download/verify/install against the signed updater plugin.
#[cfg(target_os = "macos")]
pub struct SignedUpdate {
    app: tauri::AppHandle,
    update: tauri_plugin_updater::Update,
}

#[cfg(target_os = "macos")]
impl SignedUpdate {
    pub fn new(app: tauri::AppHandle, update: tauri_plugin_updater::Update) -> Self {
        Self { app, update }
    }
}

#[cfg(target_os = "macos")]
impl super::UpdateProvider for SignedUpdate {
    fn download(
        &self,
        on_progress: super::ProgressSink,
    ) -> super::BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            let downloaded = Arc::new(std::sync::Mutex::new(0_u64));
            let last_percent = Arc::new(std::sync::Mutex::new(None));
            let on_chunk = {
                let downloaded = Arc::clone(&downloaded);
                let last_percent = Arc::clone(&last_percent);
                let on_progress = Arc::clone(&on_progress);
                move |chunk: usize, total: Option<u64>| {
                    let mut downloaded = lock_or_recover(&downloaded, super::PROGRESS_LOCK);
                    *downloaded = downloaded.saturating_add(chunk as u64);
                    let percent = total
                        .filter(|total| *total > 0)
                        .map(|total| (downloaded.saturating_mul(100) / total).min(100) as u8);
                    let mut last = lock_or_recover(&last_percent, super::PROGRESS_LOCK);
                    if percent != *last {
                        *last = percent;
                        on_progress(percent);
                    }
                }
            };
            self.update
                .download(on_chunk, || {})
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), String> {
        self.update.install(bytes).map_err(|error| {
            let error = error.to_string();
            let _ =
                self.app
                    .emit_event_to("settings", crate::events::event::UPDATE_ABORTED, &error);
            error
        })
    }
}

/// Draft preparation under the exclusive activity permit.
#[cfg(target_os = "macos")]
pub struct DraftRecovery {
    app: tauri::AppHandle,
}

#[cfg(target_os = "macos")]
impl DraftRecovery {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

#[cfg(target_os = "macos")]
impl super::Recovery for DraftRecovery {
    type Permit = crate::update_activity::ExclusiveLease;

    fn try_begin(&self) -> super::ActivityAttempt<Self::Permit> {
        let activity: Arc<crate::update_activity::UpdateActivity> = self
            .app
            .state::<Arc<crate::update_activity::UpdateActivity>>()
            .inner()
            .clone();
        use crate::update_activity::ExclusiveError;
        match activity.try_exclusive(false) {
            Ok(lease) => super::ActivityAttempt::Ready(lease),
            Err(ExclusiveError::WorkActive) => super::ActivityAttempt::WorkActive,
            Err(ExclusiveError::ResultsPending) => super::ActivityAttempt::Blocked,
            Err(ExclusiveError::AlreadyExclusive) => {
                super::ActivityAttempt::Failed("an update is already being applied".to_owned())
            }
        }
    }

    fn prepare<'a>(&'a self, _permit: &Self::Permit) -> super::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            crate::prepare_update_recovery(&self.app)
                .await
                .inspect_err(|error| {
                    let _ = self.app.emit_event_to(
                        "settings",
                        crate::events::event::UPDATE_ABORTED,
                        error,
                    );
                })
        })
    }
}

#[cfg(test)]
#[path = "tray_menu_tests.rs"]
mod tests;
