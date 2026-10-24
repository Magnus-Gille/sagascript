//! Read-only microphone permission reporting for the CLI.

use clap::{Args, Subcommand};
use sagascript_core::audio::permission::{
    microphone_permission, MicrophonePermissionReport, MicrophonePermissionStatus,
};
use sagascript_core::error::DictationError;
use serde::Serialize;

use crate::output::{outln, Out};

#[derive(Args)]
pub struct PermissionsArgs {
    #[command(subcommand)]
    pub action: PermissionsAction,
}

#[derive(Subcommand)]
pub enum PermissionsAction {
    /// Report microphone permission for this CLI process.
    #[command(
        long_about = "Report microphone permission for this CLI process. This command is read-only: it does not access saved settings, record audio, or request a permission prompt. The report describes this CLI process and does not inspect the running Sagascript GUI app.",
        after_long_help = "EXAMPLES:\n  sagascript permissions status\n  sagascript permissions status --json"
    )]
    Status {
        /// Emit the report as one machine-readable JSON document.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Serialize)]
struct StatusOutput<'a> {
    schema_version: u8,
    scope: &'static str,
    microphone: &'a MicrophonePermissionReport,
}

pub fn run(args: PermissionsArgs, out: &Out) -> Result<(), DictationError> {
    match args.action {
        PermissionsAction::Status { json } => {
            let report = microphone_permission();
            if json {
                out.json(&StatusOutput {
                    schema_version: 1,
                    scope: "current_process",
                    microphone: &report,
                })?;
            } else {
                outln!(out, "{}", format_text(&report))?;
            }
        }
    }
    Ok(())
}

fn status_name(status: &MicrophonePermissionStatus) -> &'static str {
    match status {
        MicrophonePermissionStatus::Authorized => "authorized",
        MicrophonePermissionStatus::Denied => "denied",
        MicrophonePermissionStatus::Restricted => "restricted",
        MicrophonePermissionStatus::NotDetermined => "not_determined",
        MicrophonePermissionStatus::Unsupported => "unsupported",
        MicrophonePermissionStatus::Unknown => "unknown",
    }
}

fn format_text(report: &MicrophonePermissionReport) -> String {
    format!(
        "Scope: current CLI process\nPlatform: {}\nMicrophone permission: {}\nIn app bundle: {}\n\nThis report describes this CLI process only. It does not inspect the running Sagascript GUI app, request a permission prompt, or record audio.",
        report.platform,
        status_name(&report.status),
        if report.in_app_bundle { "yes" } else { "no" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cli;
    use clap::Parser;

    fn synthetic_report(status: MicrophonePermissionStatus) -> MicrophonePermissionReport {
        MicrophonePermissionReport {
            platform: "test-platform".to_owned(),
            status,
            in_app_bundle: false,
        }
    }

    #[test]
    fn parses_status_json() {
        let cli = Cli::try_parse_from(["sagascript", "permissions", "status", "--json"])
            .expect("permissions status --json should parse");
        assert!(matches!(
            cli.command,
            Some(crate::Command::Permissions(PermissionsArgs {
                action: PermissionsAction::Status { json: true }
            }))
        ));
    }

    #[test]
    fn formats_synthetic_report_without_os_access() {
        let report = synthetic_report(MicrophonePermissionStatus::NotDetermined);
        assert_eq!(
            format_text(&report),
            "Scope: current CLI process\nPlatform: test-platform\nMicrophone permission: not_determined\nIn app bundle: no\n\nThis report describes this CLI process only. It does not inspect the running Sagascript GUI app, request a permission prompt, or record audio."
        );
    }

    #[test]
    fn serializes_the_stable_json_envelope() {
        let report = synthetic_report(MicrophonePermissionStatus::Authorized);
        let value = serde_json::to_value(StatusOutput {
            schema_version: 1,
            scope: "current_process",
            microphone: &report,
        })
        .expect("synthetic report should serialize");

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["scope"], "current_process");
        assert_eq!(value["microphone"]["platform"], "test-platform");
        assert_eq!(value["microphone"]["status"], "authorized");
        assert_eq!(value["microphone"]["in_app_bundle"], false);
    }
}
