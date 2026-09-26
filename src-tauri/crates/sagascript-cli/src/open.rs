use sagascript_core::error::DictationError;
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
const APP_BUNDLE_PATH: &str = "/Applications/Sagascript.app";

/// Private marker consumed by the desktop binary before clap parsing. It lets
/// `sagascript open` distinguish an explicit reveal request from a normal
/// background/login launch.
pub const GUI_OPEN_ARG: &str = "--show-settings-window";
/// Explicit CLI request handled inside the signed desktop app. The app checks,
/// verifies, installs, and restarts through its configured Tauri updater.
pub const GUI_UPDATE_ARG: &str = "--install-update";
/// Private marker registered with the login item so a passive startup can be
/// distinguished from a deliberate Finder, Spotlight, or CLI launch.
pub const GUI_BACKGROUND_ARG: &str = "--background";

/// Ask the operating system to launch or reactivate the installed desktop app.
/// On macOS, Launch Services routes this to the existing signed bundle when it
/// is already running, which in turn triggers the single-instance/reopen path.
pub fn run() -> Result<(), DictationError> {
    run_with_marker(GUI_OPEN_ARG, "Opening Sagascript...")
}

/// Request an in-app update through the installed desktop app. The updater
/// never downloads or installs from this CLI process.
pub fn run_update() -> Result<(), DictationError> {
    #[cfg(target_os = "macos")]
    {
        run_with_marker(GUI_UPDATE_ARG, "Checking for a signed Sagascript update...")
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(DictationError::ApplicationLaunchError(
            "in-app updates are currently available on macOS only".to_string(),
        ))
    }
}

fn run_with_marker(marker: &str, message: &str) -> Result<(), DictationError> {
    #[cfg(target_os = "macos")]
    {
        let status = launcher_command(marker).status().map_err(|error| {
            DictationError::ApplicationLaunchError(format!(
                "failed to invoke macOS Launch Services: {error}"
            ))
        })?;

        if status.success() {
            eprintln!("{message}");
            Ok(())
        } else {
            Err(DictationError::ApplicationLaunchError(format!(
                "macOS Launch Services exited with {status}; is Sagascript.app installed?"
            )))
        }
    }

    #[cfg(target_os = "windows")]
    {
        let executable = installed_windows_app_path().ok_or_else(|| {
            DictationError::ApplicationLaunchError(
                "LOCALAPPDATA is unavailable; cannot locate the installed Sagascript app"
                    .to_string(),
            )
        })?;
        launch_windows_app(&executable, marker)?;
        eprintln!("{message}");
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (marker, message);
        Err(DictationError::ApplicationLaunchError(
            "the desktop recovery command is currently available only on macOS and Windows"
                .to_string(),
        ))
    }
}

#[cfg(target_os = "windows")]
fn installed_windows_app_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| windows_app_path(Path::new(&base)))
}

#[cfg(target_os = "windows")]
fn windows_app_path(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Sagascript").join("sagascript.exe")
}

#[cfg(target_os = "windows")]
fn launch_windows_app(executable: &Path, marker: &str) -> Result<(), DictationError> {
    if !executable.is_file() {
        return Err(DictationError::ApplicationLaunchError(format!(
            "Sagascript desktop app was not found at {}; install the desktop app first",
            executable.display()
        )));
    }

    std::process::Command::new(executable)
        .arg(marker)
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            DictationError::ApplicationLaunchError(format!(
                "failed to launch {}: {error}",
                executable.display()
            ))
        })
}

#[cfg(target_os = "macos")]
fn launcher_command(marker: &str) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/open");
    command.args([APP_BUNDLE_PATH, "--args", marker]);
    command
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launcher_targets_the_signed_bundle_with_an_explicit_open_marker() {
        use std::ffi::OsStr;

        let command = super::launcher_command(super::GUI_OPEN_ARG);
        assert_eq!(command.get_program(), OsStr::new("/usr/bin/open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new(super::APP_BUNDLE_PATH),
                OsStr::new("--args"),
                OsStr::new(super::GUI_OPEN_ARG),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn update_launcher_uses_the_private_install_marker() {
        use std::ffi::OsStr;

        let command = super::launcher_command(super::GUI_UPDATE_ARG);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new(super::APP_BUNDLE_PATH),
                OsStr::new("--args"),
                OsStr::new(super::GUI_UPDATE_ARG),
            ]
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_launcher_targets_the_per_user_install() {
        use std::path::Path;

        assert_eq!(
            super::windows_app_path(Path::new(r"C:\Users\test\AppData\Local")),
            Path::new(r"C:\Users\test\AppData\Local\Sagascript\sagascript.exe")
        );
    }
}
