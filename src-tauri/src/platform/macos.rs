use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use std::io;
use std::process::{Command, ExitStatus};

const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility";

extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: core_foundation::base::CFTypeRef) -> bool;
}

/// Check if the process has accessibility (AX) permissions
pub fn is_accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

fn accessibility_settings_command() -> Command {
    let mut command = Command::new("open");
    command.arg(ACCESSIBILITY_SETTINGS_URL);
    command
}

fn interpret_open_result(result: io::Result<ExitStatus>) -> Result<(), String> {
    let status =
        result.map_err(|error| format!("Failed to open Accessibility settings: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to open Accessibility settings: open exited with {status}"
        ))
    }
}

/// Request Accessibility permission and bring the relevant System Settings
/// pane forward. AXTrustedCheckOptionPrompt alone does not reliably navigate
/// an already-running System Settings instance away from its current pane.
pub fn request_accessibility_permission() -> Result<(), String> {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);

    unsafe {
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef());
    }

    interpret_open_result(accessibility_settings_command().status())
}

/// Set the app as an accessory (no dock icon)
#[allow(deprecated)]
pub fn set_activation_policy_accessory() {
    use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
    unsafe {
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accessibility_settings_command, interpret_open_result, ACCESSIBILITY_SETTINGS_URL,
    };
    use std::ffi::OsStr;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn accessibility_request_opens_the_privacy_pane() {
        let command = accessibility_settings_command();

        assert_eq!(command.get_program(), OsStr::new("open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(ACCESSIBILITY_SETTINGS_URL)]
        );
    }

    #[test]
    fn accessibility_settings_launch_failure_is_reported() {
        let error = interpret_open_result(Err(io::Error::new(
            io::ErrorKind::NotFound,
            "open is unavailable",
        )))
        .unwrap_err();

        assert!(error.contains("open is unavailable"));
    }

    #[test]
    fn accessibility_settings_nonzero_exit_is_reported() {
        let error = interpret_open_result(Ok(ExitStatus::from_raw(1 << 8))).unwrap_err();

        assert!(error.contains("open exited with"));
    }

    #[test]
    fn accessibility_settings_success_is_accepted() {
        assert!(interpret_open_result(Ok(ExitStatus::from_raw(0))).is_ok());
    }
}
