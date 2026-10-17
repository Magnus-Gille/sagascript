//! The menu-bar / system-tray icon and its menu: the profile entries, the
//! status line, the update entries and the build identity.
//!
//! Tray, menu and window calls belong on the main thread (ADR 0003). The
//! functions here are called from there; code that may run elsewhere goes
//! through `dispatch_to_main`.

mod menu;
#[cfg(target_os = "macos")]
mod position;
mod status;

use std::sync::Mutex;

use sagascript_core::sync::lock_or_recover;
use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    Manager,
};
use tracing::info;

use crate::commands::{lock_controller, SharedController};
use crate::open_settings_window;
use crate::updater::{
    handle_check_for_updates_menu_action, register_update_menu_items, UPDATE_CHECK_ACTION,
};
use menu::create_profile_menu_items;
#[cfg(target_os = "macos")]
use position::{configure_macos_tray_identity, seed_macos_tray_preferred_position};

pub(crate) use menu::{select_profile_menu, update_profiles_menu};
pub(crate) use status::{update_tray_last_result, update_tray_status};

const BUILD_IDENTITY: &str = concat!(
    "Version ",
    env!("CARGO_PKG_VERSION"),
    " · Build ",
    env!("GIT_HASH"),
    " · ",
    env!("BUILD_DATE"),
);

/// Shared tray status menu item for updating from anywhere
type SharedStatusItem = Mutex<Option<MenuItem<tauri::Wry>>>;

// Tray menu bookkeeping is recovered as-is after a panic (ADR 0005): it only
// holds menu handles and flags that the next menu update overwrites.
const STATUS_ITEM_LOCK: &str = "tray.status_item";

const PROFILE_MENU_LOCK: &str = "tray.profile_menu";

struct ProfileMenuState {
    submenu: Submenu<tauri::Wry>,
    items: Vec<MenuItem<tauri::Wry>>,
    selected_profile_id: Option<String>,
}

type SharedProfileMenuState = Mutex<Option<ProfileMenuState>>;

pub(crate) fn manage_tray_state(app: &tauri::App) {
    let status_item: SharedStatusItem = Mutex::new(None);
    let profile_menu: SharedProfileMenuState = Mutex::new(None);
    app.manage(status_item);
    app.manage(profile_menu);
}

/// Build the tray icon and its menu, store the menu handles for later updates
/// and render the initial state.
pub(crate) fn create_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // Build tray menu
    let quit = MenuItem::with_id(app, "quit", "Quit Sagascript", true, None::<&str>)?;
    let settings_item =
        MenuItem::with_id(app, "settings", "Open Sagascript...", true, None::<&str>)?;
    let transcribe_file_item = MenuItem::with_id(
        app,
        "transcribe_file",
        "Transcribe File...",
        true,
        None::<&str>,
    )?;
    let status = MenuItem::with_id(app, "status", "Sagascript - Idle", false, None::<&str>)?;
    let profiles = {
        let ctrl: tauri::State<'_, SharedController> = app.state();
        let profiles = lock_controller(&ctrl).settings().resolved_hotkey_profiles();
        profiles
    };
    let selected_profile_id = profiles.first().map(|profile| profile.id.clone());
    let profile_items = create_profile_menu_items(app, &profiles, selected_profile_id.as_deref())?;
    let profiles_menu = Submenu::new(app, "Profiles", true)?;
    for item in &profile_items {
        profiles_menu.append(item)?;
    }
    let update_status = MenuItem::with_id(
        app,
        "update_status",
        "Updates: not checked",
        false,
        None::<&str>,
    )?;
    let check_for_updates_item = MenuItem::with_id(
        app,
        "check_for_updates",
        UPDATE_CHECK_ACTION,
        true,
        None::<&str>,
    )?;
    let build_info_item =
        MenuItem::with_id(app, "build_info", BUILD_IDENTITY, false, None::<&str>)?;

    // Store status item so we can update it after transcription
    {
        let status_state: tauri::State<'_, SharedStatusItem> = app.state();
        *lock_or_recover(&status_state, STATUS_ITEM_LOCK) = Some(status.clone());
    }
    {
        let profile_state: tauri::State<'_, SharedProfileMenuState> = app.state();
        *lock_or_recover(&profile_state, PROFILE_MENU_LOCK) = Some(ProfileMenuState {
            submenu: profiles_menu.clone(),
            items: profile_items,
            selected_profile_id,
        });
    }
    register_update_menu_items(app, &update_status, &check_for_updates_item);
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &profiles_menu,
            &update_status,
            &check_for_updates_item,
            &build_info_item,
            &settings_item,
            &transcribe_file_item,
            &quit,
        ],
    )?;

    #[cfg(target_os = "macos")]
    seed_macos_tray_preferred_position();

    let tray_builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Sagascript")
        // macOS 26 can register an image-backed status item but paint
        // it blank. Compact native text stays visible: S while idle,
        // then a state marker while recording or transcribing.
        .title("S");
    #[cfg(target_os = "windows")]
    let tray_builder = match app.default_window_icon() {
        Some(icon) => tray_builder.icon(icon.clone()),
        None => return Err("Windows tray icon is missing from the app bundle".into()),
    };
    let tray = tray_builder
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "quit" => {
                info!("Quit requested");
                app.exit(0);
            }
            "settings" => {
                open_settings_window(app, None, "tray_menu_settings");
            }
            "transcribe_file" => {
                open_settings_window(app, Some("transcribe"), "tray_menu_transcribe_file");
            }
            "manage_profiles" => {
                open_settings_window(app, Some("dictate"), "tray_menu_manage_profiles");
            }
            "check_for_updates" => {
                handle_check_for_updates_menu_action(app);
            }
            _ => {}
        })
        .build(app)?;

    #[cfg(target_os = "macos")]
    configure_macos_tray_identity(&tray)?;
    tray.set_visible(true)?;

    info!("Tray icon created");

    // Render the initial tray state through the same path as later
    // updates — the hotkey-health flag was recorded above, before the
    // tray existed, and no state transition may ever come to refresh
    // a static "Idle" label if the hotkey is dead.
    update_tray_status(app.handle(), "idle");
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
