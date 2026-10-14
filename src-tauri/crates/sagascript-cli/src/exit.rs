//! How a CLI run ends (#364): the exit status, the `Error: …` line on stderr
//! and, for a failed `--json`/`--jsonl` run, the structured error object on
//! stdout.
//!
//! Commands return `Result<(), CliError>` (or `DictationError`, which converts
//! to [`CliError::Failed`]); [`conclude`] turns the result plus the run's
//! state (cancellation, a closed or failing stdout) into an [`Exit`]. Nothing
//! here calls `std::process::exit`; the binaries return the status from `main`.

use std::fmt;
use std::process::ExitCode;

use sagascript_core::error::DictationError;
use serde::Serialize;

use crate::output::{errln, Out, WriteError};

/// Every way a run ends, with its documented status
/// (docs/recipes/add-cli-subcommand.md, "Exit codes").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// 0: the command did its job.
    Success,
    /// 0: stdout's reader went away (`… | head -1`); the run stopped quietly.
    StdoutClosed,
    /// 1: the command failed.
    Failure,
    /// 2: the invocation is invalid. clap's own usage errors exit 2 before a
    /// command runs.
    Usage,
    /// 3: a check ran to completion, wrote its report, and did not pass
    /// (`latency-report` budget, `engine doctor`, `benchmark-dictation`'s
    /// limits and text checks, `diarization reference-qualify`).
    CheckFailed,
    /// 130 for SIGINT (and Ctrl-C on Windows), 143 for SIGTERM.
    Cancelled(u8),
}

impl Exit {
    /// The process exit status.
    pub fn code(self) -> u8 {
        match self {
            Self::Success | Self::StdoutClosed => 0,
            Self::Failure => 1,
            Self::Usage => 2,
            Self::CheckFailed => 3,
            Self::Cancelled(code) => code,
        }
    }
}

impl From<Exit> for ExitCode {
    fn from(exit: Exit) -> Self {
        ExitCode::from(exit.code())
    }
}

impl std::process::Termination for Exit {
    fn report(self) -> ExitCode {
        self.into()
    }
}

/// Why a command failed.
#[derive(Debug)]
pub enum CliError {
    /// The command failed (exit 1).
    Failed(DictationError),
    /// The invocation is invalid in a way clap cannot express (exit 2).
    Usage(String),
    /// A check completed and did not pass (exit 3). Its report is already on
    /// stdout; the error is the `Error: …` line for stderr.
    CheckFailed(DictationError),
}

impl CliError {
    pub fn exit(&self) -> Exit {
        match self {
            Self::Failed(_) => Exit::Failure,
            Self::Usage(_) => Exit::Usage,
            Self::CheckFailed(_) => Exit::CheckFailed,
        }
    }

    /// The machine-readable `code` of the JSON error object.
    pub fn code(&self) -> String {
        match self {
            Self::Failed(error) => dictation_error_code(error),
            Self::Usage(_) => "usage".to_string(),
            Self::CheckFailed(_) => "check_failed".to_string(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) | Self::CheckFailed(error) => error.fmt(f),
            Self::Usage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CliError {}

impl From<DictationError> for CliError {
    fn from(error: DictationError) -> Self {
        Self::Failed(error)
    }
}

impl From<WriteError> for CliError {
    fn from(error: WriteError) -> Self {
        Self::Failed(error.into())
    }
}

/// The error's variant in snake_case (`FileDecodeError` → `file_decode_error`),
/// taken from the `kind` tag `DictationError` already serializes, so every
/// variant has a code without a second list to keep in sync.
pub fn dictation_error_code(error: &DictationError) -> String {
    let kind = serde_json::to_value(error)
        .ok()
        .and_then(|value| value.get("kind")?.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut code = String::with_capacity(kind.len() + 4);
    for (index, character) in kind.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                code.push('_');
            }
            code.push(character.to_ascii_lowercase());
        } else {
            code.push(character);
        }
    }
    if code.is_empty() {
        code.push_str("error");
    }
    code
}

/// How a command's machine-readable stdout is encoded, so an error object
/// matches the command's own output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonOutput {
    /// One pretty-printed document (`transcribe --json`, `engine … --json`,
    /// `record --json`).
    Pretty,
    /// Compact one-line objects (`transcribe --jsonl`, `glossary suggest --json`).
    Compact,
}

/// The error object on stdout: `{"status":"error","code":…,"error":…}`, with
/// `source` when one input failed. Its `status`/`source`/`error` keys match
/// the batch items of `transcribe --json`/`--jsonl`.
#[derive(Debug, Serialize)]
pub struct ErrorObject<'a> {
    status: &'static str,
    code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    error: &'a str,
}

impl<'a> ErrorObject<'a> {
    pub fn new(code: &'a str, source: Option<&'a str>, error: &'a str) -> Self {
        Self {
            status: "error",
            code,
            source,
            error,
        }
    }

    pub fn write(&self, out: &Out, framing: JsonOutput) -> Result<(), WriteError> {
        match framing {
            JsonOutput::Pretty => out.json(self),
            JsonOutput::Compact => out.jsonl(self),
        }
    }
}

/// Ends a run: decides the exit status and writes the final diagnostics.
///
/// In order: a cancellation (`Cancelled`, 130/143) wins, because a Ctrl-C
/// also ends the reader of a pipeline; then a closed stdout ends the run
/// quietly with 0; then a stdout that could not be written fails it. A
/// command error is reported as `Error: …` on stderr. With `--json`/`--jsonl`
/// (`json`), a run that fails before writing anything to stdout also writes
/// one [`ErrorObject`], so a consumer never has to parse an empty stream; a
/// run that already wrote its report (a failed `engine doctor --json`) or
/// some JSONL items keeps stdout as it is.
pub fn conclude(
    result: Result<(), CliError>,
    json: Option<JsonOutput>,
    out: &Out,
    cancelled: Option<i32>,
) -> Exit {
    // A buffered tail can still meet a closed reader.
    let _ = out.flush();
    if let Some(signal_code) = cancelled {
        report_on_stdout(out, json, "cancelled", "Cancelled");
        errln!(out, "Cancelled");
        return Exit::Cancelled(u8::try_from(signal_code).unwrap_or(130));
    }
    if out.stdout_closed() {
        return Exit::StdoutClosed;
    }
    if let Some(error) = out.stdout_error() {
        errln!(out, "Error: could not write to stdout: {error}");
        return Exit::Failure;
    }
    match result {
        Ok(()) => Exit::Success,
        Err(error) => {
            report_on_stdout(out, json, &error.code(), &error.to_string());
            errln!(out, "Error: {error}");
            error.exit()
        }
    }
}

fn report_on_stdout(out: &Out, json: Option<JsonOutput>, code: &str, message: &str) {
    if let Some(framing) = json {
        if !out.stdout_used() {
            // Best effort: the stderr line below still reports the failure.
            let _ = ErrorObject::new(code, None, message).write(out, framing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::outln;
    use crate::output::test_writers::{ClosedPipe, FullDisk};

    #[test]
    fn codes_follow_the_documented_table() {
        assert_eq!(Exit::Success.code(), 0);
        assert_eq!(Exit::StdoutClosed.code(), 0);
        assert_eq!(Exit::Failure.code(), 1);
        assert_eq!(Exit::Usage.code(), 2);
        assert_eq!(Exit::CheckFailed.code(), 3);
        assert_eq!(Exit::Cancelled(130).code(), 130);
        assert_eq!(Exit::Cancelled(143).code(), 143);
    }

    #[test]
    fn error_codes_are_the_snake_case_variant() {
        let code = |error| dictation_error_code(&error);
        assert_eq!(
            code(DictationError::FileDecodeError("x".into())),
            "file_decode_error"
        );
        assert_eq!(
            code(DictationError::SettingsError("x".into())),
            "settings_error"
        );
        assert_eq!(
            code(DictationError::MicrophonePermissionDenied),
            "microphone_permission_denied"
        );
        assert_eq!(code(DictationError::ModelBusy), "model_busy");
        assert_eq!(
            CliError::CheckFailed(DictationError::SettingsError("x".into())).code(),
            "check_failed"
        );
        assert_eq!(CliError::Usage("x".into()).code(), "usage");
    }

    #[test]
    fn a_failure_reports_on_stderr_and_exits_one() {
        let (out, captured) = Out::capture();
        let error = DictationError::SettingsError("Unknown key".into());
        assert_eq!(conclude(Err(error.into()), None, &out, None), Exit::Failure);
        assert_eq!(captured.stdout(), "");
        assert_eq!(captured.stderr(), "Error: Settings error: Unknown key\n");
    }

    #[test]
    fn a_failed_json_run_writes_one_error_document() {
        let (out, captured) = Out::capture();
        let error = DictationError::FileDecodeError("bad".into());
        let exit = conclude(Err(error.into()), Some(JsonOutput::Pretty), &out, None);
        assert_eq!(exit, Exit::Failure);
        assert_eq!(
            captured.stdout(),
            "{\n  \"status\": \"error\",\n  \"code\": \"file_decode_error\",\n  \"error\": \"File decode error: bad\"\n}\n"
        );
        assert_eq!(captured.stderr(), "Error: File decode error: bad\n");
    }

    #[test]
    fn a_failed_jsonl_run_writes_one_error_line() {
        let (out, captured) = Out::capture();
        let error = DictationError::TranscriptionFailed("no model".into());
        conclude(Err(error.into()), Some(JsonOutput::Compact), &out, None);
        assert_eq!(
            captured.stdout(),
            "{\"status\":\"error\",\"code\":\"transcription_failed\",\"error\":\"Transcription failed: no model\"}\n"
        );
    }

    #[test]
    fn a_report_already_on_stdout_is_not_followed_by_an_error_object() {
        let (out, captured) = Out::capture();
        out.json(&serde_json::json!({"ok": false})).unwrap();
        let error = CliError::CheckFailed(DictationError::TranscriptionFailed("doctor".into()));
        let exit = conclude(Err(error), Some(JsonOutput::Pretty), &out, None);
        assert_eq!(exit, Exit::CheckFailed);
        assert_eq!(captured.stdout(), "{\n  \"ok\": false\n}\n");
        assert_eq!(captured.stderr(), "Error: Transcription failed: doctor\n");
    }

    #[test]
    fn cancellation_wins_and_keeps_its_signal_status() {
        for signal_code in [130, 143] {
            let (out, captured) = Out::capture();
            let error = DictationError::TranscriptionFailed("Cancelled".into());
            let exit = conclude(Err(error.into()), None, &out, Some(signal_code));
            assert_eq!(exit.code(), u8::try_from(signal_code).unwrap());
            assert_eq!(captured.stderr(), "Cancelled\n");
            assert_eq!(captured.stdout(), "");
        }
    }

    #[test]
    fn a_cancelled_json_run_reports_the_cancellation_on_stdout() {
        let (out, captured) = Out::capture();
        let exit = conclude(Ok(()), Some(JsonOutput::Compact), &out, Some(130));
        assert_eq!(exit, Exit::Cancelled(130));
        assert_eq!(
            captured.stdout(),
            "{\"status\":\"error\",\"code\":\"cancelled\",\"error\":\"Cancelled\"}\n"
        );
    }

    #[test]
    fn a_closed_stdout_ends_quietly_with_zero_even_after_an_error() {
        let (out, captured) = Out::capture_stderr(ClosedPipe);
        let error = outln!(out, "result").unwrap_err();
        let exit = conclude(Err(error.into()), Some(JsonOutput::Compact), &out, None);
        assert_eq!(exit, Exit::StdoutClosed);
        assert_eq!(exit.code(), 0);
        assert_eq!(captured.stderr(), "");
    }

    #[test]
    fn a_failing_stdout_is_reported_once_with_its_cause() {
        let (out, captured) = Out::capture_stderr(FullDisk);
        let error = outln!(out, "result").unwrap_err();
        assert_eq!(conclude(Err(error.into()), None, &out, None), Exit::Failure);
        assert_eq!(
            captured.stderr(),
            "Error: could not write to stdout: No space left on device\n"
        );
    }
}
