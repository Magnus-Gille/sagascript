#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;
#[cfg(any(target_os = "macos", all(test, unix)))]
use std::process::ExitStatus;

use clap::{Args, Subcommand};
use sagascript_core::error::DictationError;

#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
const APP_BUNDLE_PATH: &str = "/Applications/Sagascript.app";

pub const SUCCESS_MESSAGE: &str = "Presenter request sent; check Sagascript status for completion";

/// A private request understood by the desktop binary's startup/single-instance
/// callback. It intentionally carries no recording data, paths, or text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenterRequest {
    Start { profile_id: Option<String> },
    Finish,
    Cancel,
}

/// Build the one private argv marker accepted by the desktop presenter bridge.
pub fn build_marker(request: &PresenterRequest) -> Result<String, String> {
    match request {
        PresenterRequest::Start { profile_id: None } => Ok("--presenter-start".to_string()),
        PresenterRequest::Start {
            profile_id: Some(profile_id),
        } => {
            if !valid_profile_id(profile_id) {
                return Err("invalid presenter profile id".to_string());
            }
            Ok(format!("--presenter-start={profile_id}"))
        }
        PresenterRequest::Finish => Ok("--presenter-finish".to_string()),
        PresenterRequest::Cancel => Ok("--presenter-cancel".to_string()),
    }
}

/// Parse exactly one private argv marker. The caller must pass argv after the
/// executable name; all ordinary arguments and mixed/unknown markers fail.
pub fn parse_marker(args: &[String]) -> Result<PresenterRequest, String> {
    if args.len() != 1 {
        return Err("presenter request requires exactly one private marker".to_string());
    }

    match args[0].as_str() {
        "--presenter-start" => Ok(PresenterRequest::Start { profile_id: None }),
        "--presenter-finish" => Ok(PresenterRequest::Finish),
        "--presenter-cancel" => Ok(PresenterRequest::Cancel),
        marker => match marker.strip_prefix("--presenter-start=") {
            Some(profile_id) if valid_profile_id(profile_id) => Ok(PresenterRequest::Start {
                profile_id: Some(profile_id.to_string()),
            }),
            Some(_) => Err("invalid presenter profile id".to_string()),
            None => Err("unknown presenter marker".to_string()),
        },
    }
}

fn valid_profile_id(profile_id: &str) -> bool {
    let bytes = profile_id.as_bytes();
    if bytes.is_empty() || bytes.len() > 32 {
        return false;
    }
    (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'_'
        })
}

#[derive(Args)]
pub struct PresenterArgs {
    #[command(subcommand)]
    pub action: PresenterAction,
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum PresenterAction {
    /// Start presenter dictation with the named profile, or the default/first profile when omitted
    Start {
        /// Optional explicit profile id (lowercase ASCII letters, digits, '-' or '_')
        #[arg(value_name = "PROFILE-ID")]
        profile_id: Option<String>,
    },
    /// Finish the active presenter dictation
    Finish,
    /// Cancel the active presenter dictation
    Cancel,
}

impl PresenterAction {
    fn into_request(self) -> PresenterRequest {
        match self {
            Self::Start { profile_id } => PresenterRequest::Start { profile_id },
            Self::Finish => PresenterRequest::Finish,
            Self::Cancel => PresenterRequest::Cancel,
        }
    }
}

pub fn run(args: PresenterArgs) -> Result<(), DictationError> {
    send_request(args.action.into_request())?;
    println!("{SUCCESS_MESSAGE}");
    Ok(())
}

fn send_request(request: PresenterRequest) -> Result<(), DictationError> {
    let command = command_for(&request)?;

    #[cfg(target_os = "macos")]
    {
        let mut process = Command::new(&command.program);
        process.args(&command.args);
        let status = process.status().map_err(spawn_failure)?;
        if status.success() {
            return Ok(());
        }
        Err(status_failure(status))
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let mut process = Command::new(&command.program);
        process.args(&command.args);
        if command.create_no_window {
            process.creation_flags(0x0800_0000);
        }
        process.spawn().map(|_| ()).map_err(spawn_failure)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = command;
        Err(DictationError::ApplicationLaunchError(
            "presenter requests are currently supported only on macOS and Windows".to_string(),
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LaunchCommand {
    program: String,
    args: Vec<String>,
    create_no_window: bool,
}

fn command_for(request: &PresenterRequest) -> Result<LaunchCommand, DictationError> {
    let marker = build_marker(request).map_err(DictationError::SettingsError)?;

    #[cfg(target_os = "macos")]
    {
        Ok(LaunchCommand {
            program: "/usr/bin/open".to_string(),
            args: vec![
                "-g".to_string(),
                "-n".to_string(),
                APP_BUNDLE_PATH.to_string(),
                "--args".to_string(),
                marker,
            ],
            create_no_window: false,
        })
    }

    #[cfg(target_os = "windows")]
    {
        let executable = installed_windows_app_path().ok_or_else(|| {
            DictationError::ApplicationLaunchError(
                "LOCALAPPDATA is unavailable; cannot locate the installed Sagascript app"
                    .to_string(),
            )
        })?;
        if !executable.is_file() {
            return Err(DictationError::ApplicationLaunchError(
                "Sagascript desktop app is not installed".to_string(),
            ));
        }
        Ok(windows_command_for_path(&executable, marker))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = marker;
        Err(DictationError::ApplicationLaunchError(
            "presenter requests are currently supported only on macOS and Windows".to_string(),
        ))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn spawn_failure(error: std::io::Error) -> DictationError {
    DictationError::ApplicationLaunchError(format!("failed to launch presenter request: {error}"))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn status_failure(status: ExitStatus) -> DictationError {
    DictationError::ApplicationLaunchError(format!(
        "presenter request launcher exited unsuccessfully: {status}"
    ))
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
fn windows_command_for_path(path: &Path, marker: String) -> LaunchCommand {
    LaunchCommand {
        program: path.to_string_lossy().into_owned(),
        args: vec![marker],
        create_no_window: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{build_marker, parse_marker, PresenterRequest};

    #[test]
    fn marker_builder_and_parser_round_trip_all_requests() {
        for (request, marker) in [
            (
                PresenterRequest::Start { profile_id: None },
                "--presenter-start",
            ),
            (
                PresenterRequest::Start {
                    profile_id: Some("swedish_1".to_string()),
                },
                "--presenter-start=swedish_1",
            ),
            (PresenterRequest::Finish, "--presenter-finish"),
            (PresenterRequest::Cancel, "--presenter-cancel"),
        ] {
            assert_eq!(build_marker(&request).unwrap(), marker);
            assert_eq!(parse_marker(&[marker.to_string()]).unwrap(), request);
        }
    }

    #[test]
    fn parser_rejects_extra_mixed_unknown_and_malformed_arguments() {
        for args in [
            vec![],
            vec!["--presenter-start".to_string(), "extra".to_string()],
            vec![
                "--presenter-start".to_string(),
                "--presenter-cancel".to_string(),
            ],
            vec!["--presenter-start=Bad_ID".to_string()],
            vec!["--presenter-start=".to_string()],
            vec!["--presenter-start=abc/../x".to_string()],
            vec!["--presenter-start=abc\u{0000}x".to_string()],
            vec!["--unknown".to_string()],
            vec!["/tmp/recording.wav".to_string()],
        ] {
            assert!(parse_marker(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn builder_rejects_invalid_profile_ids() {
        for profile_id in ["", "Bad", "-bad", "bad.id", "bad/id", "bad profile"] {
            assert!(build_marker(&PresenterRequest::Start {
                profile_id: Some(profile_id.to_string()),
            })
            .is_err());
        }
        assert!(build_marker(&PresenterRequest::Start {
            profile_id: Some("a".repeat(33)),
        })
        .is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_command_is_background_new_app_with_only_marker_args() {
        let command = super::command_for(&PresenterRequest::Finish).unwrap();
        assert_eq!(command.program, "/usr/bin/open");
        assert_eq!(
            command.args,
            [
                "-g",
                "-n",
                "/Applications/Sagascript.app",
                "--args",
                "--presenter-finish",
            ]
        );
    }

    #[test]
    fn launch_failures_are_returned_without_claiming_success() {
        let error = super::spawn_failure(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "blocked",
        ));
        assert!(error.to_string().contains("failed to launch"));
        assert!(!error.to_string().contains("request sent"));
    }

    #[cfg(unix)]
    #[test]
    fn nonzero_launcher_status_is_returned_as_failure() {
        let status = std::process::Command::new("/usr/bin/false")
            .status()
            .expect("false should be available on the test host");
        assert!(!status.success());
        assert!(super::status_failure(status)
            .to_string()
            .contains("unsuccessfully"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_command_uses_the_per_user_install_and_hidden_console_flag() {
        let path = std::path::Path::new(r"C:\Users\test\AppData\Local\Sagascript\sagascript.exe");
        let command = super::windows_command_for_path(path, "--presenter-cancel".to_string());
        assert_eq!(command.program, path.to_string_lossy());
        assert_eq!(command.args, ["--presenter-cancel"]);
        assert!(command.create_no_window);
    }
}
