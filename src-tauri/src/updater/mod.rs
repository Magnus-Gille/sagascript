//! The in-app update flow: the update entries of the tray menu, the check
//! against the release manifest and, on macOS, the signed download and install
//! (`signed.rs`).
//!
//! The update task runs on the async runtime; every menu change goes through
//! `dispatch_to_main` because tray calls belong on the main thread (ADR 0003).
//! The sequence itself is driven by the orchestrator (`orchestrator.rs`),
//! which sees only the `MenuSink` / `CheckSource` / `UpdateProvider` /
//! `Recovery` seams implemented here and in `signed.rs` (issue #376).

pub(crate) mod activity;
mod orchestrator;
pub(crate) mod recovery;
#[cfg(any(target_os = "macos", test))]
mod signed;
pub(crate) mod updates;

use std::sync::{Arc, Mutex};

use sagascript_core::sync::lock_or_recover;
use tauri::{menu::MenuItem, Manager};
use tracing::error;

use crate::{dispatch_to_main, focus_diag};
use orchestrator::{
    BoxFuture, CheckSource, CheckedRelease, CheckingGuard, MenuSink, Orchestrator, UpdateFailure,
    UpdatePhase,
};
#[cfg(target_os = "macos")]
use signed::{install_signed_update, SignedCheck};

// Update menu bookkeeping is recovered as-is after a panic (ADR 0005), like the
// tray's: it only holds menu handles and flags that the next menu update
// overwrites.
const UPDATE_MENU_LOCK: &str = "tray.update_menu";

#[derive(Clone)]
struct UpdateMenuItems {
    status: MenuItem<tauri::Wry>,
    check: MenuItem<tauri::Wry>,
}

struct UpdateMenuState {
    items: Option<UpdateMenuItems>,
    checking: bool,
    installing: bool,
    available_version: Option<String>,
}

type SharedUpdateMenuState = Mutex<UpdateMenuState>;

pub(crate) const UPDATE_CHECK_ACTION: &str = "Check for Updates…";

#[derive(Clone, Debug, PartialEq, Eq)]
enum UpdateMenuMode {
    Checking,
    Available(String),
    #[cfg(any(target_os = "macos", test))]
    Installing,
    Idle,
}

struct TrayTexts {
    status: String,
    action: String,
    enabled: bool,
    mode: UpdateMenuMode,
}

/// Maps orchestrator phases to the tray's texts and menu mode. This is the
/// only place the user-visible update strings live.
fn texts_for(phase: &UpdatePhase) -> TrayTexts {
    let retry = |status: String| TrayTexts {
        status,
        action: "Try Again…".to_string(),
        enabled: true,
        mode: UpdateMenuMode::Idle,
    };
    match phase {
        UpdatePhase::Checking => TrayTexts {
            status: "Checking for updates…".to_string(),
            action: UPDATE_CHECK_ACTION.to_string(),
            enabled: false,
            mode: UpdateMenuMode::Checking,
        },
        UpdatePhase::UpToDate => TrayTexts {
            status: "Sagascript is up to date".to_string(),
            action: "Check Again…".to_string(),
            enabled: true,
            mode: UpdateMenuMode::Idle,
        },
        UpdatePhase::Available {
            version,
            install_offered,
        } => {
            let status = format!("Update available — v{version}");
            let action = if *install_offered {
                format!("Install and Restart v{version}…")
            } else {
                format!("Open Release v{version}…")
            };
            TrayTexts {
                status,
                action,
                enabled: true,
                mode: UpdateMenuMode::Available(version.clone()),
            }
        }
        UpdatePhase::Failed { failure } => {
            let status = match failure {
                UpdateFailure::Check => "Couldn't check for updates".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::Download => "Update download failed".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::WaitTimedOut => "Update wait timed out".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::PreparationBlocked => "Update preparation blocked".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::PreparationFailed => "Update preparation failed".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::RecoveryConflict => "Could not save drafts for update".to_string(),
                #[cfg(any(target_os = "macos", test))]
                UpdateFailure::Install => "Update installation failed".to_string(),
            };
            retry(status)
        }
        #[cfg(any(target_os = "macos", test))]
        UpdatePhase::Downloading { version, percent } => TrayTexts {
            status: format!("Downloading update v{version}…"),
            action: percent.map_or_else(
                || "Downloading update…".to_string(),
                |percent| format!("Downloading update… {percent}%"),
            ),
            enabled: false,
            mode: UpdateMenuMode::Installing,
        },
        #[cfg(any(target_os = "macos", test))]
        UpdatePhase::WaitingForWork => TrayTexts {
            status: "Waiting for current dictation or transcription to finish…".to_string(),
            action: "Waiting for current work…".to_string(),
            enabled: false,
            mode: UpdateMenuMode::Installing,
        },
        #[cfg(any(target_os = "macos", test))]
        UpdatePhase::PreparingDrafts => TrayTexts {
            status: "Saving recoverable drafts…".to_string(),
            action: "Preparing update…".to_string(),
            enabled: false,
            mode: UpdateMenuMode::Installing,
        },
        #[cfg(any(target_os = "macos", test))]
        UpdatePhase::Installing { version } => TrayTexts {
            status: format!("Installing update v{version}…"),
            action: "Installing update…".to_string(),
            enabled: false,
            mode: UpdateMenuMode::Installing,
        },
        #[cfg(any(target_os = "macos", test))]
        UpdatePhase::Installed { version } => TrayTexts {
            status: format!("Update v{version} installed; restarting Sagascript…"),
            action: "Restarting…".to_string(),
            enabled: false,
            mode: UpdateMenuMode::Installing,
        },
    }
}

struct TrayUpdateMenu {
    app: tauri::AppHandle,
}

impl TrayUpdateMenu {
    fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}

impl MenuSink for TrayUpdateMenu {
    fn show(&self, phase: &UpdatePhase) {
        let texts = texts_for(phase);
        set_update_menu(
            &self.app,
            texts.status,
            texts.action,
            texts.enabled,
            texts.mode,
        );
    }
}

/// The GitHub release manifest check: announces a release without installing.
struct ManifestCheck;

impl CheckSource for ManifestCheck {
    fn check(&self) -> BoxFuture<'_, Result<Option<CheckedRelease>, String>> {
        Box::pin(async {
            match updates::check_for_update(env!("CARGO_PKG_VERSION")).await {
                Ok(updates::UpdateCheck::Available { version }) => Ok(Some(CheckedRelease {
                    version: version.to_string(),
                    install_offered: false,
                })),
                Ok(updates::UpdateCheck::UpToDate) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        })
    }
}

fn stable_release_url(version: &str) -> String {
    format!("https://github.com/Magnus-Gille/sagascript/releases/tag/v{version}")
}

fn open_update_release(version: &str) -> Result<(), updates::UpdateError> {
    let url = stable_release_url(version);

    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };

    command
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|source| updates::UpdateError::Open { url, source })
}

fn set_update_menu(
    app: &tauri::AppHandle,
    status_text: impl Into<String>,
    action_text: impl Into<String>,
    enabled: bool,
    mode: UpdateMenuMode,
) {
    let status_text = status_text.into();
    let action_text = action_text.into();
    dispatch_to_main(app, move |app| {
        let items = {
            let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
            let mut state = lock_or_recover(&state, UPDATE_MENU_LOCK);
            match mode {
                UpdateMenuMode::Checking => {
                    state.checking = true;
                    state.installing = false;
                    state.available_version = None;
                }
                UpdateMenuMode::Available(version) => {
                    state.checking = false;
                    state.installing = false;
                    state.available_version = Some(version);
                }
                #[cfg(any(target_os = "macos", test))]
                UpdateMenuMode::Installing => {
                    state.checking = false;
                    state.installing = true;
                    state.available_version = None;
                }
                UpdateMenuMode::Idle => {
                    state.checking = false;
                    state.installing = false;
                    state.available_version = None;
                }
            }
            state.items.clone()
        };
        let Some(items) = items else {
            return;
        };
        if let Err(error) = items.status.set_text(status_text) {
            error!("Failed to update tray update status: {error}");
        }
        if let Err(error) = items.check.set_text(action_text) {
            error!("Failed to update tray update action: {error}");
        }
        if let Err(error) = items.check.set_enabled(enabled) {
            error!("Failed to update tray update action availability: {error}");
        }
    });
}

fn check_for_updates(app: tauri::AppHandle) {
    check_for_updates_with_intent(app, false);
}

pub(crate) fn check_for_updates_and_install(app: tauri::AppHandle) {
    check_for_updates_with_intent(app, true);
}

fn check_for_updates_with_intent(app: tauri::AppHandle, install_when_available: bool) {
    focus_diag::log(
        "update_check_requested",
        serde_json::json!({ "installWhenAvailable": install_when_available }),
    );
    #[cfg(not(target_os = "macos"))]
    let _ = install_when_available;

    let items = {
        let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
        let mut state = lock_or_recover(&state, UPDATE_MENU_LOCK);
        if state.checking || state.installing {
            return;
        }
        let Some(items) = state.items.clone() else {
            return;
        };
        state.checking = true;
        items
    };

    if let Err(error) = items.status.set_text("Checking for updates…") {
        error!("Failed to update tray update status: {error}");
    }
    if let Err(error) = items.check.set_enabled(false) {
        error!("Failed to disable update action: {error}");
    }

    tauri::async_runtime::spawn(async move {
        // Clears the checking flag on every exit path, including a panic in
        // the update task, so a crashed check cannot freeze the menu (#376).
        let guard_app = app.clone();
        let _checking_guard = CheckingGuard::new(move || {
            let state: tauri::State<'_, SharedUpdateMenuState> = guard_app.state();
            let mut state = lock_or_recover(&state, UPDATE_MENU_LOCK);
            state.checking = false;
        });
        let orchestrator = Orchestrator::new(Arc::new(TrayUpdateMenu::new(app.clone())));

        #[cfg(target_os = "macos")]
        if updates::updater_public_key().is_some() {
            let source = SignedCheck::new(app.clone());
            if let Ok(Some(checked)) = orchestrator.check(&source).await {
                if install_when_available {
                    if let Some(update) = source.take_update() {
                        install_signed_update(&orchestrator, app.clone(), update, &checked.version)
                            .await;
                    }
                }
            }
            return;
        }

        let _ = orchestrator.check(&ManifestCheck).await;
    });
}

pub(crate) fn manage_update_menu_state(app: &tauri::App) {
    let update_menu: SharedUpdateMenuState = Mutex::new(UpdateMenuState {
        items: None,
        checking: false,
        installing: false,
        available_version: None,
    });
    app.manage(update_menu);
}

pub(crate) fn register_update_menu_items(
    app: &tauri::App,
    update_status: &MenuItem<tauri::Wry>,
    check_for_updates_item: &MenuItem<tauri::Wry>,
) {
    let update_state: tauri::State<'_, SharedUpdateMenuState> = app.state();
    lock_or_recover(&update_state, UPDATE_MENU_LOCK).items = Some(UpdateMenuItems {
        status: update_status.clone(),
        check: check_for_updates_item.clone(),
    });
}

/// The tray's "Check for Updates" entry: check, or install an announced update.
pub(crate) fn handle_check_for_updates_menu_action(app: &tauri::AppHandle) {
    let (available_version, installing) = {
        let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
        let state = lock_or_recover(&state, UPDATE_MENU_LOCK);
        (state.available_version.clone(), state.installing)
    };
    if installing {
        // The updater owns the menu action while it is
        // downloading, waiting for work, or restarting.
    } else if let Some(version) = available_version {
        if updates::updater_public_key().is_some() {
            check_for_updates_and_install(app.clone());
        } else if let Err(error) = open_update_release(&version) {
            error!("Failed to open update release: {error}");
        }
    } else {
        check_for_updates(app.clone());
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
