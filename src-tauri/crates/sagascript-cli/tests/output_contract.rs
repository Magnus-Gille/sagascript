//! The CLI's output and exit contract, end to end (#364).
//!
//! These tests pin what `transcribe` writes today for failed batch items and
//! preflight errors, before its output moves to an output layer. Settings and
//! every user directory point into a temp dir, so the spawned CLI never sees
//! the real settings or models (ADR 0004).

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use sagascript_testsupport::{isolated_cli_env, TempDir};

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

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("CLI stdout is UTF-8")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("CLI stderr is UTF-8")
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
    use serde_json::Value;

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
