use tauri::Manager;
use tracing::{info, warn};

/// Loads the settings for app startup and for the settings watcher.
///
/// macOS Accessibility (TCC) approval is runtime authorization, never folded into
/// the stored preference: the paste service checks it before every paste and
/// leaves the text on the clipboard when it is missing, and Settings shows that
/// auto-paste waits for approval. An earlier version wrote `auto_paste = false`
/// here whenever `AXIsProcessTrusted()` answered false, which turned the user's
/// preference off for good after an update replaced the bundle and the first
/// answer came back false (SAGA-518). A preference inherited from another bundle
/// identity is still reset by the settings migration, and a value the CLI set is
/// still never pasted without approval.
pub(crate) fn load_app_settings() -> sagascript_core::settings::Settings {
    #[cfg(target_os = "macos")]
    let accessibility_trusted = crate::platform::macos::is_accessibility_trusted();
    #[cfg(not(target_os = "macos"))]
    let accessibility_trusted = true;
    load_app_settings_with(sagascript_core::machine::load, accessibility_trusted)
}

fn load_app_settings_with(
    load: impl FnOnce() -> sagascript_core::settings::Settings,
    accessibility_trusted: bool,
) -> sagascript_core::settings::Settings {
    let settings = load();
    warn_if_auto_paste_waits_for_permission(&settings, accessibility_trusted);
    settings
}

/// Only reports. It takes the settings by reference so that it cannot change them.
fn warn_if_auto_paste_waits_for_permission(
    settings: &sagascript_core::settings::Settings,
    accessibility_trusted: bool,
) -> bool {
    let waits = !auto_paste_permitted(settings.auto_paste, accessibility_trusted);
    if waits {
        warn!("Auto-paste is on but Accessibility is not approved yet; pasting waits for approval");
    }
    waits
}

fn auto_paste_permitted(requested: bool, accessibility_trusted: bool) -> bool {
    !requested || accessibility_trusted
}

/// Migrate the legacy FlowDictate settings file to the new Sagascript path, if
/// present and no Sagascript settings file already exists. Rename failure used
/// to be silently swallowed (`let _ = std::fs::rename(...)`), which would
/// silently reset an upgrading user to defaults with no diagnostic trail.
fn migrate_legacy_settings(legacy: &std::path::Path, new_path: &std::path::Path) {
    if !legacy.exists() || new_path.exists() {
        return;
    }

    info!("Migrating settings store from FlowDictate");
    match std::fs::rename(legacy, new_path) {
        Ok(()) => info!(
            "Migrated settings store from FlowDictate ({}) to Sagascript ({})",
            legacy.display(),
            new_path.display()
        ),
        Err(e) => warn!(
            "Failed to migrate FlowDictate settings from {} to {}: {e} — \
             the upgrading user will fall back to default settings",
            legacy.display(),
            new_path.display()
        ),
    }
}

fn migrate_legacy_settings_unless_overridden(
    legacy: &std::path::Path,
    new_path: &std::path::Path,
    settings_path_is_overridden: bool,
) {
    if settings_path_is_overridden {
        return;
    }
    migrate_legacy_settings(legacy, new_path);
}

pub(super) fn migrate_legacy_settings_at_startup(app: &tauri::App) {
    let app_dir = app.path().app_data_dir().ok();
    if let Some(dir) = app_dir {
        let legacy = dir.join("flowdictate-settings.json");
        let new_path = sagascript_core::settings::store::settings_path();
        migrate_legacy_settings_unless_overridden(
            &legacy,
            &new_path,
            sagascript_core::settings::store::settings_path_is_overridden(),
        );
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
