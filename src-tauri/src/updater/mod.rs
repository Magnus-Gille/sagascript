//! The in-app update flow: the update entries of the tray menu, the check
//! against the release manifest and, on macOS, the signed download and install
//! (`signed.rs`).
//!
//! The update task runs on the async runtime; every menu change goes through
//! `dispatch_to_main` because tray calls belong on the main thread (ADR 0003).

pub(crate) mod activity;
pub(crate) mod recovery;
#[cfg(any(target_os = "macos", test))]
mod signed;
pub(crate) mod updates;

use std::sync::Mutex;

use sagascript_core::sync::lock_or_recover;
use tauri::{menu::MenuItem, Manager};
use tracing::{error, warn};

use crate::{dispatch_to_main, focus_diag};
#[cfg(target_os = "macos")]
use signed::{finish_update_check_error, install_signed_update};

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
    available_version: Option<semver::Version>,
}

type SharedUpdateMenuState = Mutex<UpdateMenuState>;

pub(crate) const UPDATE_CHECK_ACTION: &str = "Check for Updates…";

#[derive(Clone)]
enum UpdateMenuMode {
    Available(semver::Version),
    #[cfg(target_os = "macos")]
    Installing,
    Idle,
}

fn update_status_text(result: &updates::UpdateCheck) -> String {
    match result {
        updates::UpdateCheck::Available { version } => {
            format!("Update available — v{version}")
        }
        updates::UpdateCheck::UpToDate => "Sagascript is up to date".to_string(),
    }
}

fn update_action_text(result: &updates::UpdateCheck) -> String {
    match result {
        updates::UpdateCheck::Available { version } => {
            format!("Open Release v{version}…")
        }
        updates::UpdateCheck::UpToDate => "Check Again…".to_string(),
    }
}

fn stable_release_url(version: &semver::Version) -> String {
    format!("https://github.com/Magnus-Gille/sagascript/releases/tag/v{version}")
}

fn open_update_release(version: &semver::Version) -> Result<(), updates::UpdateError> {
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
                UpdateMenuMode::Available(version) => {
                    state.checking = false;
                    state.installing = false;
                    state.available_version = Some(version);
                }
                #[cfg(target_os = "macos")]
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
        #[cfg(target_os = "macos")]
        if updates::updater_public_key().is_some() {
            match updates::check_signed_update(&app).await {
                Ok(Some(update)) => {
                    let version =
                        match semver::Version::parse(update.version.trim_start_matches('v')) {
                            Ok(version) => version,
                            Err(error) => {
                                finish_update_check_error(
                                    &app,
                                    format!("invalid updater version: {error}"),
                                );
                                return;
                            }
                        };
                    if install_when_available {
                        install_signed_update(app, update, version).await;
                    } else {
                        set_update_menu(
                            &app,
                            format!("Update available — v{version}"),
                            format!("Install and Restart v{version}…"),
                            true,
                            UpdateMenuMode::Available(version),
                        );
                    }
                    return;
                }
                Ok(None) => {
                    set_update_menu(
                        &app,
                        "Sagascript is up to date",
                        "Check Again…",
                        true,
                        UpdateMenuMode::Idle,
                    );
                    return;
                }
                Err(error) => {
                    finish_update_check_error(&app, error);
                    return;
                }
            }
        }

        let result = updates::check_for_update(env!("CARGO_PKG_VERSION")).await;
        let (status_text, action_text, available_version, check_error) = match result {
            Ok(result) => {
                let available_version = match &result {
                    updates::UpdateCheck::Available { version } => Some(version.clone()),
                    updates::UpdateCheck::UpToDate => None,
                };
                (
                    update_status_text(&result),
                    update_action_text(&result),
                    available_version,
                    None,
                )
            }
            Err(error) => (
                "Couldn't check for updates".to_string(),
                "Try Again…".to_string(),
                None,
                Some(error),
            ),
        };
        if let Some(error) = check_error {
            warn!("Update check failed: {error}");
        }
        let mode = match available_version {
            Some(version) => UpdateMenuMode::Available(version),
            None => UpdateMenuMode::Idle,
        };
        set_update_menu(&app, status_text, action_text, true, mode);
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
