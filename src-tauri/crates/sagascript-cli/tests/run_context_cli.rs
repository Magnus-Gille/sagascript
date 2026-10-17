//! How `transcribe`, `record` and `meeting reprocess` resolve profile,
//! language, model and one-run prompt, end to end (#346).
//!
//! The three commands share one resolver for profile, language and
//! dictionary, but each keeps its own model rules and so its own order of
//! errors when two things are wrong at once. These tests run the real binary
//! and pin what a user sees, so that sharing the resolver cannot change a
//! message, an error `code` or an exit code. Settings, `HOME` and every user
//! directory are a temp dir's (ADR 0004); no model is ever downloaded.

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
        std::fs::write(
            root.join("settings.json"),
            r#"{"language":"en","hotkey":"Super+D","profile_glossary_migrated":true,
                "hotkey_profiles":[
                  {"id":"default","name":"English","shortcut":"Super+D","language":"en"},
                  {"id":"swedish","name":"Swedish","shortcut":"Super+S","language":"sv"}],
                "profile_glossaries":{"default":"Sagascript","swedish":"Grimnir = grimmer"}}"#,
        )
        .expect("write settings");
        std::fs::write(root.join("a.wav"), b"RIFF").expect("write input");
        std::fs::write(root.join("review.json"), "{}").expect("write review");
        std::fs::write(root.join("hint.txt"), "Notre Dame\n").expect("write hint");
        Self { root }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
        isolated_cli_env(&mut command, &self.root.join("settings.json"));
        let home: PathBuf = self.root.join("home");
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, &home);
        }
        command
            .current_dir(self.root.path())
            .args(args)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run sagascript")
    }
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("CLI stderr is UTF-8")
}

/// The one line the CLI prints for a failure, without its `Error: ` prefix.
fn failure(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "a failed run exits 1; stderr={}",
        stderr_text(output)
    );
    let stderr = stderr_text(output);
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .unwrap_or_else(|| panic!("no `Error:` line in {stderr}"));
    line.to_string()
}

const MISSING_HINT: &str = "no-such-folder/hint.txt";

fn assert_prompt_file_failure(message: &str) {
    assert!(
        message.starts_with(
            "File decode error: Failed to read prompt file 'no-such-folder/hint.txt': "
        ),
        "{message}"
    );
}

#[test]
fn transcribe_reports_resolution_failures_with_their_codes() {
    let sandbox = Sandbox::new();

    let unknown_profile = sandbox.run(&["transcribe", "a.wav", "--profile", "nope"]);
    assert_eq!(
        failure(&unknown_profile),
        "Settings error: Unknown dictation profile 'nope'"
    );
    assert!(unknown_profile.stdout.is_empty());

    let bad_language = sandbox.run(&["transcribe", "a.wav", "--language", "xx"]);
    assert_eq!(
        failure(&bad_language),
        "Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"
    );

    for (flags, code) in [
        (&["--profile", "nope"][..], "settings_error"),
        (&["--language", "xx"][..], "settings_error"),
        (&["--prompt-file", MISSING_HINT][..], "file_decode_error"),
    ] {
        let mut args = vec!["transcribe", "a.wav", "--json"];
        args.extend_from_slice(flags);
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(1), "{flags:?}");
        let object: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("{flags:?}: stdout is not JSON ({error})"));
        assert_eq!(object["status"], "error", "{flags:?}: {object}");
        assert_eq!(object["code"], code, "{flags:?}: {object}");
    }

    let prompt_file = sandbox.run(&["transcribe", "a.wav", "--prompt-file", MISSING_HINT]);
    assert_prompt_file_failure(&failure(&prompt_file));
}

#[test]
fn transcribe_checks_the_prompt_file_before_the_model() {
    // `transcribe` resolves profile, language and prompt together and only then
    // the model; `record` and `meeting reprocess` do it the other way round.
    let sandbox = Sandbox::new();
    let both_wrong = sandbox.run(&[
        "transcribe",
        "a.wav",
        "--model",
        "bogus",
        "--prompt-file",
        MISSING_HINT,
    ]);
    assert_prompt_file_failure(&failure(&both_wrong));

    let profile_first = sandbox.run(&[
        "transcribe",
        "a.wav",
        "--profile",
        "nope",
        "--prompt-file",
        MISSING_HINT,
    ]);
    assert_eq!(
        failure(&profile_first),
        "Settings error: Unknown dictation profile 'nope'"
    );

    let language_first = sandbox.run(&[
        "transcribe",
        "a.wav",
        "--language",
        "xx",
        "--prompt-file",
        MISSING_HINT,
    ]);
    assert_eq!(
        failure(&language_first),
        "Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"
    );
}

#[test]
fn the_resolved_language_reaches_the_model_choice() {
    let sandbox = Sandbox::new();
    // An English-only model against a Swedish run: the language came from the
    // profile in one case and from the flag in the other.
    for flags in [&["--profile", "swedish"][..], &["--language", "sv"][..]] {
        let mut args = vec!["transcribe", "a.wav", "--model", "base.en"];
        args.extend_from_slice(flags);
        assert_eq!(
            failure(&sandbox.run(&args)),
            "Settings error: Whisper model 'Whisper Base (EN)' is incompatible with Swedish",
            "{flags:?}"
        );
    }
    // The same model for the default (English) profile gets past resolution
    // and stops at the missing download.
    assert_eq!(
        failure(&sandbox.run(&["transcribe", "a.wav", "--model", "base.en"])),
        "Transcription failed: Model 'Whisper Base (EN)' is not downloaded. Run: sagascript download-model base.en"
    );
}

#[test]
fn profile_and_language_cannot_be_combined_in_any_command() {
    let sandbox = Sandbox::new();
    // Without `record` and `diarization` (the lean build) nothing is added.
    #[cfg_attr(
        not(any(feature = "record", feature = "diarization")),
        allow(unused_mut)
    )]
    let mut commands: Vec<Vec<&str>> = vec![vec!["transcribe", "a.wav"]];
    #[cfg(feature = "record")]
    commands.push(vec!["record"]);
    #[cfg(feature = "diarization")]
    commands.push(vec![
        "meeting",
        "reprocess",
        "plan",
        "a.wav",
        "--previous-review",
        "review.json",
        "--output",
        "plan.json",
    ]);
    for mut args in commands {
        let label = args.join(" ");
        args.extend(["--profile", "swedish", "--language", "sv"]);
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(2), "{label}");
        assert!(
            stderr_text(&output).contains("cannot be used with"),
            "{label}: {}",
            stderr_text(&output)
        );
    }
}

/// The one `error` object a `--json` run prints on stdout.
fn json_failure(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr={}",
        stderr_text(output)
    );
    let object: Value = serde_json::from_slice(&output.stdout).expect("stdout is one JSON object");
    assert_eq!(object["status"], "error", "{object}");
    object
}

#[test]
fn a_language_override_decides_whether_the_profile_dictionary_is_in_force() {
    // `--correct-hints` needs a dictionary to correct with. The default
    // profile has one ("Sagascript"); it applies while the language is the
    // profile's own and is left behind when `--language` changes it.
    let sandbox = Sandbox::new();
    let run = |flags: &[&str]| {
        let mut args = vec!["transcribe", "a.wav", "--json", "--correct-hints"];
        args.extend_from_slice(flags);
        json_failure(&sandbox.run(&args))
    };

    let same_language = run(&["--model", "base.en", "--language", "en"]);
    assert_eq!(
        same_language["code"], "transcription_failed",
        "{same_language}"
    );
    assert!(
        same_language["error"]
            .as_str()
            .is_some_and(|text| text.contains("is not downloaded")),
        "{same_language}"
    );

    let changed_language = run(&["--model", "kb-whisper-base", "--language", "sv"]);
    assert_eq!(
        changed_language["code"], "settings_error",
        "{changed_language}"
    );
    assert!(
        changed_language["error"].as_str().is_some_and(|text| text
            .starts_with("Settings error: --correct-hints requires a non-empty --hint/--prompt, ")),
        "{changed_language}"
    );

    let changed_with_prompt = run(&[
        "--model",
        "kb-whisper-base",
        "--language",
        "sv",
        "--prompt",
        "Acme",
    ]);
    assert_eq!(
        changed_with_prompt["code"], "transcription_failed",
        "{changed_with_prompt}"
    );
    assert!(
        changed_with_prompt["error"]
            .as_str()
            .is_some_and(|text| text.contains("is not downloaded")),
        "{changed_with_prompt}"
    );
}

#[cfg(feature = "diarization")]
mod reprocess {
    use super::*;

    fn plan(sandbox: &Sandbox, flags: &[&str]) -> Output {
        let mut args = vec![
            "meeting",
            "reprocess",
            "plan",
            "a.wav",
            "--previous-review",
            "review.json",
            "--output",
            "plan.json",
        ];
        args.extend_from_slice(flags);
        sandbox.run(&args)
    }

    #[test]
    fn the_model_is_checked_before_the_prompt_file() {
        let sandbox = Sandbox::new();
        assert_eq!(
            failure(&plan(
                &sandbox,
                &["--model", "bogus", "--prompt-file", MISSING_HINT]
            )),
            "Settings error: Unknown file model preference 'bogus'"
        );
        assert_eq!(
            failure(&plan(
                &sandbox,
                &[
                    "--model",
                    "pianissimo-sv",
                    "--language",
                    "sv",
                    "--prompt-file",
                    MISSING_HINT
                ]
            )),
            "Settings error: Meeting reprocessing requires a Whisper model; select one with --model"
        );
        assert_prompt_file_failure(&failure(&plan(
            &sandbox,
            &["--model", "base.en", "--prompt-file", MISSING_HINT],
        )));
    }

    #[test]
    fn profile_and_language_errors_come_first() {
        let sandbox = Sandbox::new();
        assert_eq!(
            failure(&plan(
                &sandbox,
                &[
                    "--profile",
                    "nope",
                    "--model",
                    "bogus",
                    "--prompt-file",
                    MISSING_HINT
                ]
            )),
            "Settings error: Unknown dictation profile 'nope'"
        );
        assert_eq!(
            failure(&plan(
                &sandbox,
                &["--language", "xx", "--prompt-file", MISSING_HINT]
            )),
            "Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"
        );
    }

    #[test]
    fn a_resolved_runtime_moves_on_to_reading_the_review() {
        // Resolution succeeds for every flag set below (no model is needed to
        // resolve it); the first thing to fail is the placeholder review.
        let sandbox = Sandbox::new();
        for flags in [
            &["--model", "base.en"][..],
            // No --model: an automatic choice that is never Pianissimo (this
            // is the real binary in a sandbox, so what is downloaded on the
            // machine running the test does not matter).
            &["--language", "sv"][..],
            &[
                "--profile",
                "swedish",
                "--model",
                "kb-whisper-base",
                "--prompt-file",
                "hint.txt",
            ][..],
            &[
                "--language",
                "sv",
                "--model",
                "kb-whisper-base",
                "--prompt",
                "Grimnir",
            ][..],
        ] {
            assert_eq!(
                failure(&plan(&sandbox, flags)),
                "File decode error: meeting previous review input is invalid JSON or fails its schema validation",
                "{flags:?}"
            );
        }
    }
}

#[cfg(feature = "record")]
mod record {
    use super::*;

    #[test]
    fn resolution_failures_come_before_any_capture() {
        let sandbox = Sandbox::new();
        assert_eq!(
            failure(&sandbox.run(&["record", "--profile", "nope"])),
            "Settings error: Unknown dictation profile 'nope'"
        );
        assert_eq!(
            failure(&sandbox.run(&["record", "--language", "xx"])),
            "Settings error: Unknown language 'xx'. Valid: en, sv, no, fi, auto"
        );
    }

    #[test]
    fn the_model_is_checked_before_the_prompt_file() {
        let sandbox = Sandbox::new();
        assert_eq!(
            failure(&sandbox.run(&["record", "--model", "bogus", "--prompt-file", MISSING_HINT])),
            "Settings error: Unknown model 'bogus'. Run 'sagascript list-models' to see available models."
        );
        assert_prompt_file_failure(&failure(&sandbox.run(&[
            "record",
            "--model",
            "base.en",
            "--prompt-file",
            MISSING_HINT,
        ])));
    }
}
