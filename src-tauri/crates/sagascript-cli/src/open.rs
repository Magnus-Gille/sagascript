use sagascript_core::error::DictationError;

#[cfg(target_os = "macos")]
const APP_BUNDLE_PATH: &str = "/Applications/Sagascript.app";

/// Private marker consumed by the desktop binary before clap parsing. It lets
/// `sagascript open` distinguish an explicit reveal request from a normal
/// background/login launch.
pub const GUI_OPEN_ARG: &str = "--show-settings-window";
/// Private marker registered with the login item so a passive startup can be
/// distinguished from a deliberate Finder, Spotlight, or CLI launch.
pub const GUI_BACKGROUND_ARG: &str = "--background";

/// Ask the operating system to launch or reactivate the installed desktop app.
/// On macOS, Launch Services routes this to the existing signed bundle when it
/// is already running, which in turn triggers the single-instance/reopen path.
pub fn run() -> Result<(), DictationError> {
    #[cfg(target_os = "macos")]
    {
        let status = launcher_command().status().map_err(|error| {
            DictationError::ApplicationLaunchError(format!(
                "failed to invoke macOS Launch Services: {error}"
            ))
        })?;

        if status.success() {
            eprintln!("Opening Sagascript...");
            Ok(())
        } else {
            Err(DictationError::ApplicationLaunchError(format!(
                "macOS Launch Services exited with {status}; is Sagascript.app installed?"
            )))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(DictationError::ApplicationLaunchError(
            "the desktop recovery command is currently available only on macOS".to_string(),
        ))
    }
}

#[cfg(target_os = "macos")]
fn launcher_command() -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/open");
    command.args([APP_BUNDLE_PATH, "--args", GUI_OPEN_ARG]);
    command
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launcher_targets_the_signed_bundle_with_an_explicit_open_marker() {
        use std::ffi::OsStr;

        let command = super::launcher_command();
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
}
