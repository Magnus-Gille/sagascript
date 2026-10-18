use tauri::Manager;
#[cfg(target_os = "macos")]
use tracing::error;
use tracing::{info, warn};

/// Treat macOS TCC approval as runtime authorization, never as a preference
/// that can be inherited from another bundle identity or forced by the CLI.
pub(crate) fn load_settings_with_permission_gate() -> sagascript_core::settings::Settings {
    let settings = sagascript_core::settings::store::load();

    #[cfg(target_os = "macos")]
    {
        let mut settings = settings;
        if !auto_paste_permitted(
            settings.auto_paste,
            crate::platform::macos::is_accessibility_trusted(),
        ) {
            warn!("Auto-paste was enabled without Accessibility permission; disabling it");
            match sagascript_core::settings::store::update(|latest| latest.auto_paste = false) {
                Ok(persisted) => settings = persisted,
                Err(error) => {
                    error!("Failed to persist permission-gated auto-paste setting: {error}");
                    settings.auto_paste = false;
                }
            }
        }
        settings
    }

    #[cfg(not(target_os = "macos"))]
    settings
}

#[cfg(any(target_os = "macos", test))]
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
