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

    /// The stored settings of the Swedish profile hold a model that only serves
    /// English: what a run of `swedish` then refuses with.
    fn with_an_english_model_on_the_swedish_profile(self) -> Self {
        std::fs::write(
            self.root.join("settings.json"),
            r#"{"language":"en","hotkey":"Super+D","profile_glossary_migrated":true,
                "hotkey_profiles":[
                  {"id":"default","name":"English","shortcut":"Super+D","language":"en"},
                  {"id":"swedish","name":"Swedish","shortcut":"Super+S","language":"sv"}],
                "profile_models":{"swedish":"tiny.en"},
                "profile_glossaries":{"default":"Sagascript","swedish":"Grimnir = grimmer"}}"#,
        )
        .expect("write settings");
        self
    }
}

/// The `code` and `error` of the one JSON error object a failed `--json` or
/// `--jsonl` run writes on stdout (#364).
fn json_error(output: &Output) -> (String, String) {
    let stdout = String::from_utf8(output.stdout.clone()).expect("CLI stdout is UTF-8");
    let value: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("stdout is not one JSON document ({error}): {stdout:?}"));
    assert_eq!(value["status"], "error", "{value}");
    (
        value["code"].as_str().expect("a code").to_string(),
        value["error"].as_str().expect("an error text").to_string(),
    )
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
        (&["--profile", "nope"][..], "unknown_profile"),
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

/// `glossary suggest --json` reports a settings refusal with the code of its
/// `SettingsError`, as `transcribe --json` does (ADR 0006).
mod glossary_suggest {
    use super::*;

    fn suggest(sandbox: &Sandbox, input: &str, profile: &str) -> Output {
        std::fs::write(sandbox.root.join("corrected.txt"), "corrected\n").expect("write text");
        sandbox.run(&[
            "glossary",
            "suggest",
            input,
            "--corrected",
            "corrected.txt",
            "--profile",
            profile,
            "--json",
        ])
    }

    #[test]
    fn an_unknown_profile_is_unknown_profile() {
        let sandbox = Sandbox::new();
        std::fs::write(sandbox.root.join("heard.txt"), "heard\n").expect("write text");
        let output = suggest(&sandbox, "heard.txt", "nope");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            json_error(&output),
            (
                "unknown_profile".to_string(),
                "Settings error: Unknown dictation profile 'nope'".to_string()
            )
        );
    }

    #[test]
    fn a_stored_model_the_profile_language_cannot_use_is_model_incompatible() {
        let sandbox = Sandbox::new().with_an_english_model_on_the_swedish_profile();
        let output = suggest(&sandbox, "a.wav", "swedish");
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            json_error(&output),
            (
                "model_incompatible".to_string(),
                "Settings error: Model 'Whisper Tiny (EN)' is incompatible with profile 'swedish'"
                    .to_string()
            )
        );
    }
}

/// The dictionary editor (`glossary add|remove|clear`, `suggest --apply`)
/// reports a refusal of its own closure with the text the settings give it,
/// once: it used to flatten `Settings error: Unknown dictation profile 'x'` into
/// a string and wrap it again (#360 part 2b).
mod glossary_edit {
    use super::*;

    #[test]
    fn an_unknown_profile_is_reported_once_by_every_editing_command() {
        let sandbox = Sandbox::new();
        for args in [
            &["glossary", "add", "Term", "--profile", "nope"][..],
            &["glossary", "remove", "Term", "--profile", "nope"][..],
            &["glossary", "clear", "--yes", "--profile", "nope"][..],
            &["glossary", "list", "--profile", "nope"][..],
            &["glossary", "path", "--profile", "nope"][..],
        ] {
            assert_eq!(
                failure(&sandbox.run(args)),
                "Settings error: Unknown dictation profile 'nope'",
                "{args:?}"
            );
        }
    }

    #[test]
    fn the_other_refusals_and_the_success_lines_keep_their_text() {
        let sandbox = Sandbox::new();
        let removed = sandbox.run(&["glossary", "remove", "Sagascript"]);
        assert_eq!(removed.status.code(), Some(0));
        assert!(
            stderr_text(&removed).contains("Removed personal dictionary term: Sagascript\n"),
            "{}",
            stderr_text(&removed)
        );
        assert_eq!(
            failure(&sandbox.run(&["glossary", "remove", "Sagascript"])),
            "Settings error: Personal dictionary term 'Sagascript' was not found"
        );
        assert_eq!(
            failure(&sandbox.run(&["glossary", "clear"])),
            "Settings error: Refusing to clear the personal dictionary without --yes"
        );
        let cleared = sandbox.run(&["glossary", "clear", "--yes", "--profile", "swedish"]);
        assert_eq!(cleared.status.code(), Some(0));
        assert!(
            stderr_text(&cleared).contains("Cleared personal dictionary\n"),
            "{}",
            stderr_text(&cleared)
        );
        assert_eq!(
            failure(&sandbox.run(&["glossary", "add", "Term", "--alias", "a,b"])),
            "Settings error: Glossary alias cannot contain a newline, comma, '=' or '|'"
        );
    }

    /// A settings file that cannot be read or written is the settings'
    /// `settings_storage_failed`, with the text it always had, also where the
    /// editor's closure used to flatten it into a plain `settings_error`.
    #[test]
    fn a_settings_file_that_cannot_be_used_is_a_storage_failure() {
        let sandbox = Sandbox::new();
        std::fs::write(sandbox.root.join("heard.txt"), "heard\n").expect("write text");
        std::fs::write(sandbox.root.join("corrected.txt"), "corrected\n").expect("write text");
        let args = [
            "glossary",
            "suggest",
            "heard.txt",
            "--corrected",
            "corrected.txt",
            "--apply",
            "--json",
        ];
        // The dry run needs no write; it works and names the suggestion.
        assert_eq!(sandbox.run(&args[..args.len() - 2]).status.code(), Some(0));

        // A folder where the settings file belongs: it cannot be read or replaced.
        std::fs::remove_file(sandbox.root.join("settings.json")).expect("remove settings");
        std::fs::create_dir(sandbox.root.join("settings.json")).expect("a folder takes its place");
        let output = sandbox.run(&args);
        assert_eq!(output.status.code(), Some(1));
        let (code, text) = json_error(&output);
        assert_eq!(code, "settings_storage_failed");
        assert!(
            text.starts_with("Settings error: Settings file ")
                && text.contains("refusing to overwrite it"),
            "{text}"
        );
        for args in [
            &["glossary", "add", "Term"][..],
            &["glossary", "remove", "Term"][..],
            &["glossary", "clear", "--yes"][..],
        ] {
            let message = failure(&sandbox.run(args));
            assert!(
                message.starts_with("Settings error: Settings file ")
                    && !message.starts_with("Settings error: Settings error: "),
                "{args:?}: {message}"
            );
        }
    }
}

#[cfg(feature = "record")]
mod record {
    use super::*;

    /// `record --json` writes its settings refusals as error objects with the
    /// code of their `SettingsError`.
    #[test]
    fn a_json_run_reports_a_settings_refusal_with_its_own_code() {
        let sandbox = Sandbox::new();
        let unknown = sandbox.run(&["record", "--json", "--profile", "nope"]);
        assert_eq!(unknown.status.code(), Some(1));
        assert_eq!(
            json_error(&unknown),
            (
                "unknown_profile".to_string(),
                "Settings error: Unknown dictation profile 'nope'".to_string()
            )
        );

        let sandbox = Sandbox::new().with_an_english_model_on_the_swedish_profile();
        let incompatible = sandbox.run(&["record", "--json", "--profile", "swedish"]);
        assert_eq!(incompatible.status.code(), Some(1));
        assert_eq!(
            json_error(&incompatible),
            (
                "model_incompatible".to_string(),
                "Settings error: Model 'Whisper Tiny (EN)' is incompatible with profile 'swedish'"
                    .to_string()
            )
        );

        // A flag the CLI words itself stays `settings_error`.
        let language = sandbox.run(&["record", "--json", "--language", "xx"]);
        assert_eq!(json_error(&language).0, "settings_error");
    }

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
