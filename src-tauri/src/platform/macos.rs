use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use std::io;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicI32, Ordering};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility";
static DICTATION_TARGET_PID: AtomicI32 = AtomicI32::new(0);

fn own_pid() -> i32 {
    i32::try_from(std::process::id()).unwrap_or(-1)
}

pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map(|application| application.processIdentifier())
}

fn should_restore_dictation_target(target: i32, current: Option<i32>, own: i32) -> bool {
    target > 0 && target != own && current == Some(own)
}

fn can_paste_to_frontmost(target: i32, current: Option<i32>, own: i32) -> bool {
    match current {
        Some(pid) if pid == own => target == own,
        Some(_) => true,
        None => false,
    }
}

/// Preserve the editor seen at hotkey-down before recording setup or the first
/// overlay WebView can activate Sagascript.
pub fn remember_dictation_target(target: Option<i32>) {
    let target = target.unwrap_or(0);
    DICTATION_TARGET_PID.store(target, Ordering::Release);
}

/// Restore only when Sagascript itself took focus. A deliberate switch to a
/// different app during dictation remains the user's chosen paste destination.
pub fn restore_dictation_target_if_stolen() -> Result<bool, String> {
    let target = DICTATION_TARGET_PID.load(Ordering::Acquire);
    if !should_restore_dictation_target(target, frontmost_pid(), own_pid()) {
        return Ok(false);
    }
    let target_app = NSRunningApplication::runningApplicationWithProcessIdentifier(target)
        .ok_or_else(|| "The previous paste target has closed".to_string())?;
    if !target_app.activateWithOptions(NSApplicationActivationOptions::empty()) {
        return Err("Could not restore the previous paste target".to_string());
    }
    Ok(true)
}

pub fn dictation_paste_target_is_valid() -> bool {
    can_paste_to_frontmost(
        DICTATION_TARGET_PID.load(Ordering::Acquire),
        frontmost_pid(),
        own_pid(),
    )
}

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

fn accessibility_request_prompts_user() -> bool {
    false
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

/// Bring the Accessibility pane forward without asking macOS to show its
/// permission prompt again. This is used when onboarding is already polling
/// and the user only needs to return to the correct System Settings pane.
pub fn open_accessibility_settings() -> Result<(), String> {
    interpret_open_result(accessibility_settings_command().status())
}

/// Register the Accessibility check without showing macOS's redundant native
/// alert, then bring the relevant System Settings pane forward directly. The
/// onboarding UI already explains why access is needed and keeps an explicit
/// button available if the user navigates away from the pane.
pub fn request_accessibility_permission() -> Result<(), String> {
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let value = if accessibility_request_prompts_user() {
        CFBoolean::true_value()
    } else {
        CFBoolean::false_value()
    };
    let options = CFDictionary::from_CFType_pairs(&[(key, value)]);

    unsafe {
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef());
    }

    open_accessibility_settings()
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

/// Bring the accessory application to the foreground before presenting a
/// user-requested window. Accessory apps do not reliably activate when a
/// window merely calls `show`/`set_focus`.
#[allow(deprecated)]
pub fn activate_app() {
    use cocoa::appkit::{NSApp, NSApplication};
    use cocoa::base::YES;

    unsafe {
        NSApp().activateIgnoringOtherApps_(YES);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        accessibility_request_prompts_user, accessibility_settings_command, interpret_open_result,
        can_paste_to_frontmost, should_restore_dictation_target, ACCESSIBILITY_SETTINGS_URL,
    };
    use std::ffi::OsStr;
    use std::io;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    #[test]
    fn focus_repair_only_runs_when_sagascript_took_the_target_from_another_app() {
        assert!(should_restore_dictation_target(101, Some(202), 202));
        assert!(!should_restore_dictation_target(101, Some(303), 202));
        assert!(!should_restore_dictation_target(202, Some(202), 202));
        assert!(!should_restore_dictation_target(0, Some(202), 202));
    }

    #[test]
    fn paste_guard_rejects_own_stolen_focus_but_allows_an_original_own_target() {
        assert!(!can_paste_to_frontmost(101, Some(202), 202));
        assert!(can_paste_to_frontmost(202, Some(202), 202));
        assert!(can_paste_to_frontmost(101, Some(303), 202));
        assert!(!can_paste_to_frontmost(0, Some(202), 202));
        assert!(!can_paste_to_frontmost(101, None, 202));
    }

    #[test]
    fn initial_accessibility_request_does_not_show_a_redundant_native_prompt() {
        assert!(!accessibility_request_prompts_user());
    }

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
