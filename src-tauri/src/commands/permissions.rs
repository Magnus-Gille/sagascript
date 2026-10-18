//! Permission queries for onboarding (Accessibility, microphone). The platform
//! calls live in `platform/`; these commands only name them.

use crate::command_error::CommandError;

#[tauri::command]
pub async fn check_accessibility_permission() -> Result<bool, CommandError> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::is_accessibility_trusted())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn request_accessibility_permission() -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation(
            "command_request_accessibility_permission",
            "opens_system_settings",
        );
        crate::platform::macos::request_accessibility_permission()
            .map_err(CommandError::LaunchFailed)?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_accessibility_settings() -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation(
            "command_open_accessibility_settings",
            "opens_system_settings",
        );
        crate::platform::macos::open_accessibility_settings()
            .map_err(CommandError::LaunchFailed)?;
    }
    Ok(())
}

/// Returns the microphone authorization status as a string.
/// Possible values: "authorized", "not_determined", "denied", "restricted", "unsupported".
/// "unsupported" is returned when the binary is not running from a proper .app bundle
/// (e.g. during `cargo run` or `cargo tauri dev` without a bundle).
#[tauri::command]
pub async fn microphone_status() -> Result<String, CommandError> {
    Ok(crate::platform::mic_permission().status())
}

/// Triggers AVCaptureDevice.requestAccessForMediaType:completionHandler: and returns
/// the new status string after the user responds (or immediately if already determined).
/// Possible return values: "authorized", "not_determined", "denied", "restricted", "unsupported".
#[tauri::command]
pub async fn request_microphone_access() -> Result<String, CommandError> {
    crate::platform::mic_permission().request().await
}

#[tauri::command]
pub async fn open_microphone_settings() -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    {
        crate::focus_diag::activation("command_open_microphone_settings", "opens_system_settings");
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn()
            .map_err(|e| {
                CommandError::LaunchFailed(format!("Failed to open System Settings: {}", e))
            })?;
    }
    Ok(())
}
