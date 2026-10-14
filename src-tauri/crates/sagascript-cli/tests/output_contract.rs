//! The CLI's output and exit contract, end to end (#364).
//!
//! A reader that stops early (`sagascript … | head -1`) must not crash the
//! CLI, and a `--json`/`--jsonl` run that fails before writing a result
//! reports one structured error object on stdout instead of an empty stream.
//! Settings and every user directory point into a temp dir, so the spawned CLI
//! never sees the real settings or models (ADR 0004).

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use sagascript_testsupport::{isolated_cli_env, TempDir};
use serde_json::Value;

struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new();
        std::fs::create_dir_all(root.join("home")).expect("create isolated home");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
        isolated_cli_env(&mut command, &self.root.join("settings.json"));
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, self.home());
        }
        command.args(args).stdin(Stdio::null());
        command
    }
}

fn run(mut command: Command) -> Output {
    command.output().expect("run sagascript")
}

/// Runs `command` with stdout connected to a pipe whose reader is already
/// closed, so the first write fails with a broken pipe: what `… | head -1`
/// does to every write after `head` has exited.
fn run_into_closed_pipe(mut command: Command) -> Output {
    let (reader, writer) = std::io::pipe().expect("create pipe");
    drop(reader);
    command
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("run sagascript")
}

fn assert_quiet_exit_after_broken_pipe(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{what}: a closed stdout must end the run quietly with exit 0; status={} stderr={stderr}",
        output.status
    );
    assert!(!stderr.contains("panicked"), "{what} panicked: {stderr}");
    assert!(
        !stderr.contains("Error:"),
        "{what} reported the closed pipe as an error: {stderr}"
    );
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("CLI stdout is UTF-8")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("CLI stderr is UTF-8")
}

fn assert_error_object(value: &Value, context: &str) {
    assert_eq!(value["status"], "error", "{context}: {value}");
    assert!(
        value["code"].as_str().is_some_and(|code| !code.is_empty()),
        "{context}: error object needs a machine-readable code: {value}"
    );
    assert!(
        value["error"].as_str().is_some_and(|text| !text.is_empty()),
        "{context}: error object needs the message: {value}"
    );
}

#[test]
fn config_list_into_a_closed_pipe_exits_zero_without_a_panic() {
    let sandbox = Sandbox::new();
    let output = run_into_closed_pipe(sandbox.command(&["config", "list"]));
    assert_quiet_exit_after_broken_pipe(&output, "config list");
}

#[test]
fn completions_into_a_closed_pipe_exit_zero_without_a_panic() {
    let sandbox = Sandbox::new();
    let output = run_into_closed_pipe(sandbox.command(&["completions", "bash"]));
    assert_quiet_exit_after_broken_pipe(&output, "completions bash");
}

#[test]
fn formats_into_a_closed_pipe_exits_zero_without_a_panic() {
    let sandbox = Sandbox::new();
    let output = run_into_closed_pipe(sandbox.command(&["formats"]));
    assert_quiet_exit_after_broken_pipe(&output, "formats");
}

#[test]
fn transcribe_json_for_a_missing_file_reports_an_error_object_on_stdout() {
    let sandbox = Sandbox::new();
    let missing = sandbox.root.join("missing.wav");
    let output = run(sandbox.command(&[
        "transcribe",
        "--json",
        "--language",
        "en",
        "--model",
        "tiny.en",
        missing.to_str().expect("UTF-8 path"),
    ]));
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);

    assert_eq!(output.status.code(), Some(1), "stderr={stderr}");
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout is not one JSON document ({error}): {stdout:?}"));
    assert!(
        value.is_object(),
        "single-file --json must be an object: {value}"
    );
    assert_error_object(&value, "transcribe --json missing.wav");
    assert!(
        stderr.contains("Error: "),
        "stderr keeps the human error: {stderr}"
    );
}

#[test]
fn transcribe_jsonl_preflight_failure_reports_one_error_line() {
    let sandbox = Sandbox::new();
    let missing = sandbox.root.join("missing.wav");
    let output = run(sandbox.command(&[
        "transcribe",
        "--jsonl",
        "--profile",
        "no-such-profile",
        missing.to_str().expect("UTF-8 path"),
    ]));
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);

    assert_eq!(output.status.code(), Some(1), "stderr={stderr}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "one compact error line: {stdout:?}");
    assert!(stdout.ends_with('\n'), "JSONL lines end with a newline");
    let value: Value = serde_json::from_str(lines[0]).expect("error line is JSON");
    assert_error_object(&value, "transcribe --jsonl --profile no-such-profile");
    assert_eq!(value["code"], "settings_error");
    assert_eq!(
        value["error"],
        "Settings error: Unknown dictation profile 'no-such-profile'"
    );
    assert_eq!(
        stderr,
        "Error: Settings error: Unknown dictation profile 'no-such-profile'\n"
    );
}

#[test]
fn glossary_suggest_json_reports_an_error_object_when_input_is_missing() {
    let sandbox = Sandbox::new();
    let heard = sandbox.root.join("heard.txt");
    let corrected = sandbox.root.join("missing-corrected.txt");
    std::fs::write(&heard, "heard text\n").expect("write heard transcript");
    let output = run(sandbox.command(&[
        "glossary",
        "suggest",
        heard.to_str().expect("UTF-8 path"),
        "--corrected",
        corrected.to_str().expect("UTF-8 path"),
        "--json",
    ]));
    let stdout = stdout_text(&output);

    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={}",
        stderr_text(&output)
    );
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout is not one JSON document ({error}): {stdout:?}"));
    assert_error_object(&value, "glossary suggest --json");
    assert_eq!(value["code"], "settings_error");
}

#[test]
fn human_output_failures_leave_stdout_empty() {
    let sandbox = Sandbox::new();
    let missing = sandbox.root.join("missing.wav");
    let output = run(sandbox.command(&[
        "transcribe",
        "--profile",
        "no-such-profile",
        missing.to_str().expect("UTF-8 path"),
    ]));

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "{:?}", stdout_text(&output));
    assert_eq!(
        stderr_text(&output),
        "Error: Settings error: Unknown dictation profile 'no-such-profile'\n"
    );
}

/// Batch failures need a "downloaded" model: the CLI checks a model file's
/// size before it starts the batch and only loads it after decoding, so a
/// sparse file of the right size lets undecodable inputs reach item-level
/// errors without any inference. The model directory follows `dirs` on the
/// platforms where HOME/XDG_DATA_HOME relocate it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod with_a_placeholder_model {
    use super::*;
    use sagascript_core::settings::WhisperModel;

    fn install_placeholder_model(sandbox: &Sandbox) {
        #[cfg(target_os = "macos")]
        let data_dir = sandbox.home().join("Library").join("Application Support");
        #[cfg(target_os = "linux")]
        let data_dir = sandbox.home();
        let models = data_dir.join("Sagascript").join("Models");
        std::fs::create_dir_all(&models).expect("create isolated model dir");
        let model = WhisperModel::TinyEn;
        let file = std::fs::File::create(models.join(model.ggml_filename()))
            .expect("create placeholder model");
        file.set_len(model.download_integrity().size)
            .expect("size placeholder model");
    }

    /// Three undecodable inputs in one directory, sorted by name.
    fn undecodable_directory(sandbox: &Sandbox) -> PathBuf {
        let dir = sandbox.root.join("inputs");
        std::fs::create_dir_all(&dir).expect("create input dir");
        for name in ["a.wav", "b.wav", "c.wav"] {
            std::fs::write(dir.join(name), b"not audio").expect("write input");
        }
        dir
    }

    fn transcribe(sandbox: &Sandbox, flags: &[&str], inputs: &[&str]) -> Command {
        let mut args = vec!["transcribe", "--language", "en", "--model", "tiny.en"];
        args.extend_from_slice(flags);
        args.extend_from_slice(inputs);
        sandbox.command(&args)
    }

    #[test]
    fn jsonl_reports_each_failed_item_on_its_own_line() {
        let sandbox = Sandbox::new();
        install_placeholder_model(&sandbox);
        let dir = undecodable_directory(&sandbox);
        let output = run(transcribe(
            &sandbox,
            &["--jsonl"],
            &[dir.to_str().expect("UTF-8 path")],
        ));
        let stdout = stdout_text(&output);
        let stderr = stderr_text(&output);

        assert_eq!(output.status.code(), Some(1), "stderr={stderr}");
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 3, "{stdout}");
        for (line, name) in lines.iter().zip(["a.wav", "b.wav", "c.wav"]) {
            let source = dir.join(name).display().to_string();
            assert!(
                line.starts_with(&format!(
                    "{{\"status\":\"error\",\"source\":{},\"error\":",
                    serde_json::to_string(&source).expect("encode source")
                )),
                "item line shape changed: {line}"
            );
            let value: Value = serde_json::from_str(line).expect("item line is JSON");
            assert_eq!(value.as_object().map(|object| object.len()), Some(3));
            assert!(stderr.contains(&format!("Error transcribing {source}: ")));
        }
        assert!(
            stderr.ends_with("Error: Transcription failed: 3 of 3 batch item(s) failed\n"),
            "{stderr}"
        );
    }

    #[test]
    fn jsonl_directory_into_a_closed_pipe_exits_zero_without_a_panic() {
        let sandbox = Sandbox::new();
        install_placeholder_model(&sandbox);
        let dir = undecodable_directory(&sandbox);
        let output = run_into_closed_pipe(transcribe(
            &sandbox,
            &["--jsonl"],
            &[dir.to_str().expect("UTF-8 path")],
        ));
        assert_quiet_exit_after_broken_pipe(&output, "transcribe --jsonl dir/");
        let stderr = stderr_text(&output);
        assert!(
            !stderr.contains("[2/3]"),
            "the batch must stop once nobody reads its output: {stderr}"
        );
    }

    #[test]
    fn json_for_one_failed_file_is_a_single_error_object() {
        let sandbox = Sandbox::new();
        install_placeholder_model(&sandbox);
        let dir = undecodable_directory(&sandbox);
        let input = dir.join("a.wav");
        let input = input.to_str().expect("UTF-8 path");
        let output = run(transcribe(&sandbox, &["--json"], &[input]));
        let stdout = stdout_text(&output);

        assert_eq!(output.status.code(), Some(1), "{}", stderr_text(&output));
        let value: Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
        assert!(
            value.is_object(),
            "single-file --json must stay an object: {value}"
        );
        assert_error_object(&value, "transcribe --json undecodable.wav");
        assert_eq!(value["source"], input);
        assert_eq!(value["code"], "file_decode_error");
    }

    #[test]
    fn json_for_several_files_stays_an_array_of_items() {
        let sandbox = Sandbox::new();
        install_placeholder_model(&sandbox);
        let dir = undecodable_directory(&sandbox);
        let output = run(transcribe(
            &sandbox,
            &["--json"],
            &[dir.to_str().expect("UTF-8 path")],
        ));
        let stdout = stdout_text(&output);

        assert_eq!(output.status.code(), Some(1), "{}", stderr_text(&output));
        let value: Value = serde_json::from_str(&stdout).expect("stdout is one JSON document");
        let items = value.as_array().expect("batch --json is an array");
        assert_eq!(items.len(), 3);
        for item in items {
            assert_eq!(item["status"], "error");
            assert_eq!(item.as_object().map(|object| object.len()), Some(3));
        }
    }
}
