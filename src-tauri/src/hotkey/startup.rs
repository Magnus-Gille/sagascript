//! Startup shortcut selection and registration, without Tauri state or locks.
//!
//! Preparing separately lets the caller log invalid settings before the first
//! OS operation. The caller retains health recording, event emission and logging.

use sagascript_core::settings::Settings;

use crate::hotkey::transition::Registrar;
use crate::{hotkey, SAFE_FALLBACK_HOTKEY};

pub(crate) struct StartupPlan {
    pub(crate) shortcuts: Vec<String>,
    pub(crate) validation_error: Option<String>,
}

pub(crate) struct StartupRegistration {
    pub(crate) registration_error: Option<String>,
    pub(crate) health_error: Option<String>,
    pub(crate) operational_hotkey: hotkey::OperationalHotkey,
}

/// Validate every profile before choosing the requested shortcuts or the safe
/// fallback. Never modify the saved settings when falling back.
pub(crate) fn prepare(settings: &Settings) -> StartupPlan {
    let validation_error = settings
        .validate_shortcut_configuration()
        .err()
        .map(|error| error.to_string());
    let shortcuts = if validation_error.is_some() {
        vec![SAFE_FALLBACK_HOTKEY.to_string()]
    } else {
        settings.resolved_shortcuts()
    };
    StartupPlan {
        shortcuts,
        validation_error,
    }
}

/// Startup has no previous registered set to restore. If registration partially
/// fails, clean up the entire attempted set exactly once before reporting health.
pub(crate) fn apply(plan: StartupPlan, registrar: &mut impl Registrar) -> StartupRegistration {
    let registration_error = registrar.register(&plan.shortcuts).err();
    let cleanup_error = registration_error
        .as_ref()
        .and_then(|_| registrar.unregister(&plan.shortcuts).err());
    let (health_error, operational_hotkey) = startup_hotkey_health(
        plan.validation_error,
        registration_error.clone(),
        cleanup_error,
        plan.shortcuts,
    );
    StartupRegistration {
        registration_error,
        health_error,
        operational_hotkey,
    }
}

/// What the hotkey health records after the startup registration: the error to
/// show, if any, and what the OS now holds. `validation_error` is the saved
/// profiles' validation failure (the safe fallback was tried instead),
/// `registration_error` the failed registration and `cleanup_error` the failed
/// cleanup of a partial registration.
fn startup_hotkey_health(
    validation_error: Option<String>,
    registration_error: Option<String>,
    cleanup_error: Option<String>,
    registration_shortcuts: Vec<String>,
) -> (Option<String>, hotkey::OperationalHotkey) {
    match (validation_error, registration_error, cleanup_error) {
        (validation, Some(registration), Some(cleanup)) => (
            Some(format!("{}registration failed: {registration}; partial-registration cleanup failed: {cleanup}", validation.map(|error| format!("{error}; ")).unwrap_or_default())),
            hotkey::OperationalHotkey::Unknown,
        ),
        (None, None, None) => (
            None,
            hotkey::OperationalHotkey::registered_many(&registration_shortcuts),
        ),
        (Some(validation), None, None) => (
            Some(format!(
                "{validation}; using safe fallback '{SAFE_FALLBACK_HOTKEY}'"
            )),
            hotkey::OperationalHotkey::registered_many(&registration_shortcuts),
        ),
        (None, Some(registration), None) => {
            (Some(registration), hotkey::OperationalHotkey::Inactive)
        }
        (Some(validation), Some(registration), None) => (
            Some(format!(
                "{validation}; fallback '{SAFE_FALLBACK_HOTKEY}' also failed: {registration}"
            )),
            hotkey::OperationalHotkey::Inactive,
        ),
        // Cleanup runs only after a registration failure, so no run gets here;
        // if one did, the OS state is unknown, not registered.
        (validation, None, Some(cleanup)) => (
            Some(format!("{}partial-registration cleanup failed without a registration error: {cleanup}", validation.map(|error| format!("{error}; ")).unwrap_or_default())),
            hotkey::OperationalHotkey::Unknown,
        ),
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "startup_health_tests.rs"]
mod health_tests;
