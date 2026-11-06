//! Route macOS Cmd+Q through Tauri's vetoable exit request.
//!
//! The native predefined Quit item invokes AppKit's `terminate:` directly,
//! bypassing `ExitRequested` and the retained-live-session guard.
use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};

const GUARDED_QUIT_ID: &str = "sagascript-guarded-quit";

pub(crate) fn build_guarded_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    // Preserve the framework's About metadata, Services and editing/window
    // menus. Compare with the framework's own localized Quit label because
    // its public menu API does not expose the predefined item's action type.
    let quit_label = PredefinedMenuItem::quit(app, None)?.text()?;
    let mut candidates = Vec::new();
    for item in menu.items()? {
        if let MenuItemKind::Submenu(submenu) = item {
            for (index, item) in submenu.items()?.into_iter().enumerate() {
                if let MenuItemKind::Predefined(native) = item {
                    if native.text()? == quit_label {
                        candidates.push((submenu.clone(), index, native));
                    }
                }
            }
        }
    }
    if candidates.len() != 1 {
        return Err(tauri::Error::Io(std::io::Error::other(
            "Expected exactly one native macOS Quit item",
        )));
    }
    for (submenu, index, native) in candidates {
        let quit = MenuItem::with_id(app, GUARDED_QUIT_ID, &quit_label, true, Some("CmdOrCtrl+Q"))?;
        submenu.remove(&native)?;
        submenu.insert(&quit, index)?;
    }
    Ok(menu)
}

pub(crate) fn handle_guarded_menu(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    if event.id().as_ref() == GUARDED_QUIT_ID {
        app.exit(0);
    }
}
