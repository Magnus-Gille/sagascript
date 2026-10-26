//! One private, read-only preflight report for the current CLI process.
use crate::exit::CliError;
use crate::output::{outln, Out};
use clap::{Args, Subcommand};
use sagascript_core::acceptance::{
    self, files, BuildObservation, CheckStatus, Observation, ObservationStatus, PreflightReport,
    Source,
};
use sagascript_core::error::DictationError;
use std::path::PathBuf;

#[derive(Args)]
pub struct AcceptanceArgs {
    #[command(subcommand)]
    pub action: AcceptanceAction,
}
#[derive(Subcommand)]
pub enum AcceptanceAction {
    /// Inspect this process without recording, prompts, engines or implicit settings access.
    #[command(
        long_about = "Inspect this process only, not another running GUI. Reports build identity, permission observations and remaining manual checks. Unsupported or unknown required checks block acceptance (exit 1). Does not start capture, request permissions, load models or read ordinary settings."
    )]
    Preflight {
        /// Exact 40-character source revision of the expected clean candidate.
        #[arg(long, value_parser = parse_revision)]
        expect_revision: String,
        /// Emit one machine-readable JSON report.
        #[arg(long)]
        json: bool,
        /// Explicit test fixture; only has_completed_onboarding is reported.
        #[arg(long)]
        settings_file: Option<PathBuf>,
        /// Publish a new private receipt. Existing files are never overwritten.
        #[arg(long)]
        receipt: Option<PathBuf>,
    },
}
fn parse_revision(value: &str) -> Result<String, String> {
    if acceptance::valid_revision(value) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err("expected a full 40-character hexadecimal source revision".into())
    }
}

pub fn run(args: AcceptanceArgs, out: &Out, build: BuildObservation) -> Result<(), CliError> {
    let AcceptanceAction::Preflight {
        expect_revision,
        json,
        settings_file,
        receipt,
    } = args.action;
    let onboarding = match settings_file {
        None => Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved),
        Some(path) => match files::read_onboarding_fixture(&path) {
            Ok(done) => Observation::observed(Source::PersistedFixture, done),
            Err(files::FixtureError::Invalid | files::FixtureError::TooLarge) => {
                Observation::unavailable(Source::PersistedFixture, ObservationStatus::Invalid)
            }
            Err(files::FixtureError::Unavailable) => {
                Observation::unavailable(Source::PersistedFixture, ObservationStatus::Error)
            }
        },
    };
    let report = acceptance::assemble(
        &expect_revision,
        build,
        acceptance::observe_process(),
        onboarding,
        Source::CurrentProcess,
    );
    emit(report, json, receipt.as_deref(), out)
}

fn failure(message: &str) -> CliError {
    CliError::Failed(DictationError::ApplicationLaunchError(message.into()))
}

fn emit(
    mut report: PreflightReport,
    json: bool,
    receipt: Option<&std::path::Path>,
    out: &Out,
) -> Result<(), CliError> {
    if let Some(path) = receipt {
        let bytes = serde_json::to_vec_pretty(&report)
            .map_err(|_| failure("Could not serialize acceptance report"))?;
        if files::write_receipt_new(path, &bytes).is_err() {
            report.summary.status = CheckStatus::Blocked;
            report.summary.checks.push(acceptance::Check {
                name: "receipt",
                status: CheckStatus::Blocked,
            });
            report
                .summary
                .remaining_manual_steps
                .push("receipt_write_failed");
        }
    }
    let blocked = report.summary.status == CheckStatus::Blocked;
    // Preserve a failed check even if stdout is closed; no second JSON document.
    let write_result = if json {
        out.json(&report).map_err(CliError::from)
    } else {
        outln!(
            out,
            "Acceptance preflight: {:?} (current process only)",
            report.summary.status
        )
        .and_then(|_| {
            outln!(
                out,
                "Remaining: {}",
                report.summary.remaining_manual_steps.join(", ")
            )
        })
        .map_err(CliError::from)
    };
    if blocked {
        return Err(failure("Acceptance preflight blocked; inspect the report"));
    }
    write_result.map_err(|_| failure("Could not write acceptance report to stdout"))?;
    out.flush()
        .map_err(|_| failure("Could not flush acceptance report to stdout"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn malformed_revision_is_usage_error() {
        for rev in ["abc", "", "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"] {
            let error = crate::Cli::try_parse_from([
                "sagascript",
                "acceptance",
                "preflight",
                "--expect-revision",
                rev,
            ])
            .err()
            .expect("must reject bad revision");
            assert_eq!(error.exit_code(), 2);
        }
    }
    #[test]
    fn embedded_preflight_uses_executable_identity_instead_of_cached_library_identity() {
        let revision = "1234567890abcdef1234567890abcdef12345678";
        for dirty in ["false", "true", "unknown"] {
            let cli = crate::Cli::try_parse_from([
                "sagascript",
                "acceptance",
                "preflight",
                "--expect-revision",
                revision,
                "--json",
            ])
            .unwrap();
            let (out, captured) = Out::capture();
            let exit = crate::run_with_build(
                cli,
                &out,
                BuildObservation::new("executable-version", revision, dirty),
            );
            let report: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
            assert_eq!(report["build"]["version"], "executable-version");
            assert_eq!(report["build"]["revision"]["value"], revision);
            let clean = report["summary"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|check| check["name"] == "clean_source")
                .unwrap();
            assert_eq!(
                clean["status"],
                if dirty == "false" {
                    "passed"
                } else {
                    "blocked"
                }
            );
            if dirty != "false" {
                assert_eq!(exit.code(), 1);
            }
        }
    }

    #[test]
    fn report_is_one_json_document_and_never_reads_implicit_settings() {
        let cli = crate::Cli::try_parse_from([
            "sagascript",
            "acceptance",
            "preflight",
            "--expect-revision",
            "0000000000000000000000000000000000000000",
            "--json",
        ])
        .unwrap();
        let (out, captured) = Out::capture();
        let exit = crate::run_with(cli, &out);
        assert_eq!(exit.code(), 1);
        let value: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
        assert_eq!(value["onboarding"]["status"], "not_observed");
        assert_eq!(value["scope"], "current_process");
        assert_eq!(value["summary"]["status"], "blocked");
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;
    use sagascript_core::acceptance::{AccessibilityStatus, ProcessObservations};
    use sagascript_core::audio::permission::MicrophonePermissionStatus;
    use sagascript_testsupport::TempDir;
    fn passed() -> PreflightReport {
        let sha = "a".repeat(40);
        acceptance::assemble(
            &sha,
            BuildObservation::new("test", &sha, "false"),
            ProcessObservations {
                microphone: Observation::observed(
                    Source::CurrentProcess,
                    MicrophonePermissionStatus::Authorized,
                ),
                accessibility: Observation::observed(
                    Source::CurrentProcess,
                    AccessibilityStatus::Trusted,
                ),
                in_app_bundle: Observation::observed(Source::CurrentProcess, true),
                binary_sha256: Observation::observed(Source::CurrentProcess, "b".repeat(64)),
            },
            Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved),
            Source::CurrentProcess,
        )
    }
    #[test]
    fn failed_receipt_cannot_publish_a_pass_on_stdout_or_exit_zero() {
        let dir = TempDir::new();
        let path = dir.join("receipt.json");
        std::fs::write(&path, b"existing private data").unwrap();
        let (out, captured) = Out::capture();
        let result = emit(passed(), true, Some(&path), &out);
        let exit = crate::exit::conclude(result, Some(crate::exit::JsonOutput::Pretty), &out, None);
        assert_eq!(exit.code(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), b"existing private data");
        let json: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
        assert_eq!(json["summary"]["status"], "blocked");
        assert!(!captured.stdout().contains("existing private data"));
        assert!(!captured.stdout().contains(&path.display().to_string()));
        assert!(!captured.stderr().contains(&path.display().to_string()));
    }
    #[test]
    fn stdout_and_new_receipt_have_identical_reports() {
        let dir = TempDir::new();
        let canonical = std::fs::canonicalize(dir.path()).unwrap();
        let path = canonical.join("receipt.json");
        let (out, captured) = Out::capture();
        emit(passed(), true, Some(&path), &out).unwrap();
        let stdout: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
        let file: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(stdout, file);
        assert_eq!(stdout["summary"]["status"], "passed");
        assert!(stdout.get("release_ready").is_none());
    }
    #[test]
    fn published_observations_do_not_hide_later_stdout_or_flush_failure() {
        struct FailOutput(bool);
        impl std::io::Write for FailOutput {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0 {
                    Ok(bytes.len())
                } else {
                    Err(std::io::ErrorKind::BrokenPipe.into())
                }
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        for fail_on_flush in [false, true] {
            let dir = TempDir::new();
            let path = std::fs::canonicalize(dir.path())
                .unwrap()
                .join("observations.json");
            let (out, _) = Out::capture_stderr(FailOutput(fail_on_flush));
            let result = emit(passed(), true, Some(&path), &out);
            assert_eq!(crate::exit::conclude(result, None, &out, None).code(), 1);
            // The exclusive receipt describes observed checks, not successful
            // delivery of stdout or the final process exit. Consumers need both.
            let bytes = std::fs::read(&path).unwrap();
            let report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(report["summary"]["status"], "passed");
            let (out, _) = Out::capture();
            assert!(emit(passed(), true, Some(&path), &out).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn closed_stdout_is_failure_even_when_checks_pass() {
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (out, _) = Out::capture_stderr(Broken);
        let result = emit(passed(), true, None, &out);
        assert_eq!(crate::exit::conclude(result, None, &out, None).code(), 1);
    }
    #[test]
    fn failed_final_flush_is_failure_even_when_writes_succeed() {
        struct FlushBroken;
        impl std::io::Write for FlushBroken {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let (out, _) = Out::capture_stderr(FlushBroken);
        let result = emit(passed(), true, None, &out);
        assert_eq!(crate::exit::conclude(result, None, &out, None).code(), 1);
    }
}
