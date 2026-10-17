use crate::app_controller::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InitialWindowRequest {
    Hidden,
    Settings,
    Onboarding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuiLaunchMode {
    Standard,
    ShowSettings,
    Background,
    InstallUpdate,
}

/// Ordinary foreground/background launches never check the network for
/// updates. Only the explicit private install marker may contact the manifest.
pub(super) fn should_install_update_at_launch(mode: GuiLaunchMode) -> bool {
    mode == GuiLaunchMode::InstallUpdate
}

/// True when argv asks clap for informational output (`--version`/`-V`,
/// `--help`/`-h`, or the `help` subcommand) that must reach a terminal.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn cli_args_request_console_output(args: &[std::ffi::OsString]) -> bool {
    args.iter().skip(1).any(|argument| {
        argument == std::ffi::OsStr::new("--version")
            || argument == std::ffi::OsStr::new("-V")
            || argument == std::ffi::OsStr::new("--help")
            || argument == std::ffi::OsStr::new("-h")
    }) || args
        .get(1)
        .is_some_and(|argument| argument == std::ffi::OsStr::new("help"))
}

/// The release GUI uses the Windows GUI subsystem, so it starts without a
/// console and its stdout/stderr handles are invalid. When launched from a
/// terminal, attach to the parent's console and point any invalid standard
/// handle at it. Redirected (valid) handles are left alone. Does nothing, and
/// opens no window, when there is no parent console (Start menu launch).
#[cfg(windows)]
pub(crate) fn attach_parent_console() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    // SAFETY: plain Win32 console calls; no pointers are passed except a
    // handle owned by a `File` that is intentionally leaked below.
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for std_handle in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let current = GetStdHandle(std_handle);
            if !current.is_null() && current != INVALID_HANDLE_VALUE {
                continue;
            }
            if let Ok(console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                SetStdHandle(std_handle, console.as_raw_handle() as _);
                std::mem::forget(console);
            }
        }
    }
}

pub(crate) fn gui_launch_mode(args: impl IntoIterator<Item = std::ffi::OsString>) -> GuiLaunchMode {
    let mut args = args.into_iter();
    let _program = args.next();
    let remaining: Vec<_> = args.collect();
    match (remaining.first().cloned(), remaining.get(1).cloned()) {
        (Some(argument), None)
            if argument == std::ffi::OsStr::new(sagascript_cli::open::GUI_OPEN_ARG) =>
        {
            GuiLaunchMode::ShowSettings
        }
        (Some(argument), None)
            if argument == std::ffi::OsStr::new(sagascript_cli::open::GUI_BACKGROUND_ARG) =>
        {
            GuiLaunchMode::Background
        }
        (Some(argument), None)
            if argument == std::ffi::OsStr::new(sagascript_cli::open::GUI_UPDATE_ARG) =>
        {
            GuiLaunchMode::InstallUpdate
        }
        _ => GuiLaunchMode::Standard,
    }
}

pub(super) fn second_instance_requests_settings(args: &[String]) -> bool {
    // Retired private markers must never trigger UI or recording actions.
    if args
        .iter()
        .any(|argument| argument.starts_with("--presenter-"))
    {
        return false;
    }
    // `tauri-plugin-single-instance` forwards the complete argv on Windows,
    // including argv[0], but transports differ across platforms and versions.
    // Treat the private background marker as a capability to stay hidden and
    // let every other relaunch (including a bare Start-menu launch) reveal
    // Settings. Looking for the marker anywhere is robust to either argv
    // shape and to a future plugin adding metadata arguments.
    !args.iter().any(|argument| {
        argument == sagascript_cli::open::GUI_BACKGROUND_ARG
            || argument == sagascript_cli::open::GUI_UPDATE_ARG
    })
}

pub(super) fn second_instance_requests_update(args: &[String]) -> bool {
    args.iter()
        .any(|argument| argument == sagascript_cli::open::GUI_UPDATE_ARG)
}

pub(super) fn initial_window_request(
    has_completed_onboarding: bool,
    launch_mode: GuiLaunchMode,
) -> InitialWindowRequest {
    if !has_completed_onboarding {
        InitialWindowRequest::Onboarding
    } else if matches!(
        launch_mode,
        GuiLaunchMode::Background | GuiLaunchMode::InstallUpdate
    ) {
        InitialWindowRequest::Hidden
    } else {
        InitialWindowRequest::Settings
    }
}

pub(super) fn should_reveal_for_reopen(state: AppState) -> bool {
    !state.is_busy()
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
