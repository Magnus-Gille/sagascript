//! `sagascript config` for the recorder settings (SAGA-564), through the real
//! binary against an isolated settings file. No audio device is opened.

use std::process::{Command, Output};

use sagascript_testsupport::{fake_settings, isolated_cli_env, FakeSettings};

fn cli(settings: &FakeSettings, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    isolated_cli_env(&mut command, settings.path());
    command.args(args).output().expect("run sagascript CLI")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn recorder_keys_get_set_reset_and_persist() {
    let settings = fake_settings();
    assert_eq!(
        stdout(&cli(&settings, &["config", "get", "recording_quality"])),
        "lossless"
    );
    assert_eq!(
        stdout(&cli(
            &settings,
            &["config", "get", "recording_channel_pair"]
        )),
        "1-2"
    );

    for (key, value) in [
        ("recording_quality", "compact"),
        ("recording_channel_pair", "3-3"),
        ("recording_directory", "/Volumes/Audio/Takes"),
    ] {
        let set = cli(&settings, &["config", "set", key, value]);
        assert!(
            set.status.success(),
            "{key}: {}",
            String::from_utf8_lossy(&set.stderr)
        );
        assert_eq!(stdout(&cli(&settings, &["config", "get", key])), value);
    }
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings.path()).unwrap()).unwrap();
    assert_eq!(raw["recording_quality"], "compact");
    assert_eq!(raw["recording_channel_pair"], serde_json::json!([3, 3]));
    assert_eq!(raw["recording_directory"], "/Volumes/Audio/Takes");

    let list = stdout(&cli(&settings, &["config", "list"]));
    for line in [
        "recording_quality",
        "recording_directory",
        "recording_channel_pair",
    ] {
        assert!(
            list.lines().any(|l| l.starts_with(line)),
            "list lacks {line}:\n{list}"
        );
    }
    assert!(!list.contains("recording_folder_confirmed"));
    assert!(!list.contains("recording_consent_acknowledged"));

    for key in [
        "recording_quality",
        "recording_channel_pair",
        "recording_directory",
    ] {
        assert!(cli(&settings, &["config", "reset", key]).status.success());
    }
    assert_eq!(
        stdout(&cli(&settings, &["config", "get", "recording_quality"])),
        "lossless"
    );
    assert_eq!(
        stdout(&cli(
            &settings,
            &["config", "get", "recording_channel_pair"]
        )),
        "1-2"
    );
    assert_eq!(
        stdout(&cli(&settings, &["config", "get", "recording_directory"])),
        ""
    );
}

#[test]
fn recorder_keys_reject_bad_values_with_a_message_and_change_nothing() {
    let settings = fake_settings();
    let file = settings.dir().join("a-file");
    std::fs::write(&file, b"x").unwrap();
    for (key, value) in [
        ("recording_channel_pair", "0-2"),
        ("recording_channel_pair", "two"),
        ("recording_quality", "studio"),
        ("recording_directory", file.to_str().unwrap()),
    ] {
        let output = cli(&settings, &["config", "set", key, value]);
        assert!(!output.status.success(), "{key}={value} was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(key)
                || String::from_utf8_lossy(&output.stderr).contains("file"),
            "{key}={value}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        stdout(&cli(
            &settings,
            &["config", "get", "recording_channel_pair"]
        )),
        "1-2"
    );
    assert_eq!(
        stdout(&cli(&settings, &["config", "get", "recording_directory"])),
        ""
    );
}
