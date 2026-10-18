use tracing::info;

#[cfg(target_os = "macos")]
const TRAY_AUTOSAVE_NAME: &str = "ai.gille.sagascript.main";

#[cfg(target_os = "macos")]
const DEFAULT_TRAY_PREFERRED_POSITION: f64 = 340.0;

/// Match the key AppKit uses to persist a named status item's menu-bar slot.
#[cfg(target_os = "macos")]
fn tray_preferred_position_key(autosave_name: &str) -> String {
    format!("NSStatusItem Preferred Position {autosave_name}")
}

#[cfg(target_os = "macos")]
fn initial_tray_preferred_position(has_saved_position: bool) -> Option<f64> {
    (!has_saved_position).then_some(DEFAULT_TRAY_PREFERRED_POSITION)
}

/// Seed a usable first position on crowded, notched menu bars. AppKit normally
/// puts a brand-new status item at the far-left edge of the status area, where
/// it may report `isVisible = true` while sitting behind the camera cutout.
/// Once the user moves the item, AppKit owns this value and we never overwrite
/// it.
#[cfg(target_os = "macos")]
pub(super) fn seed_macos_tray_preferred_position() {
    use objc2_foundation::{NSString, NSUserDefaults};

    let defaults = NSUserDefaults::standardUserDefaults();
    let key_string = tray_preferred_position_key(TRAY_AUTOSAVE_NAME);
    let key = NSString::from_str(&key_string);
    let Some(position) = initial_tray_preferred_position(defaults.objectForKey(&key).is_some())
    else {
        return;
    };

    defaults.setDouble_forKey(position, &key);
    info!(position, "Seeded initial macOS tray position");
}

/// Give AppKit a stable identity for the status item before making it visible.
/// Without an autosave name macOS 26 places every fresh item in the default
/// leftmost slot, which can sit behind a MacBook camera cutout.
#[cfg(target_os = "macos")]
pub(super) fn configure_macos_tray_identity(tray: &tauri::tray::TrayIcon) -> tauri::Result<()> {
    let configured = tray.with_inner_tray_icon(|inner| {
        let Some(status_item) = inner.ns_status_item() else {
            return false;
        };

        let autosave_name = objc2_foundation::NSString::from_str(TRAY_AUTOSAVE_NAME);
        // Tauri constructs the native item visible. Toggle it inside the same
        // main-thread callback so the stable name is in place before AppKit
        // performs the visible placement pass.
        status_item.setVisible(false);
        status_item.setAutosaveName(Some(&autosave_name));
        status_item.setVisible(true);

        status_item.autosaveName().to_string() == TRAY_AUTOSAVE_NAME
    })?;

    if configured {
        Ok(())
    } else {
        Err(std::io::Error::other("failed to assign the macOS tray autosave name").into())
    }
}

#[cfg(test)]
#[path = "position_tests.rs"]
mod tests;
