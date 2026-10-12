//! Owner regression for #340 and #365: a settings file with one wrong-typed
//! field must be preserved byte-for-byte before `config set` replaces it, and
//! the replacement keeps every other value. The CLI runs against an isolated
//! settings path in a temp dir.

use std::path::Path;
use std::process::{Command, Output};

const ORIGINAL: &[u8] = br#"{
  "show_overlay": false,
  "language": "sv",
  "beam_size": 3,
  "future_key": {"keep": true},
  "auto_paste": "invalid-bool"
}
"#;

fn run_cli(root: &Path, settings: &Path, args: &[&str]) -> Output {
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    command.env_clear();
    // Keep only what the platform loader needs; every user directory points
    // into the temp dir so nothing outside it can be read or written.
    for key in ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
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
        .env("SAGASCRIPT_SETTINGS_PATH", settings)
        .args(args)
        .output()
        .expect("run sagascript CLI")
}

#[test]
fn config_set_preserves_a_schema_invalid_settings_file_before_replacing_it() {
    let root = tempfile::tempdir().expect("create temp dir");
    let config = root.path().join("config");
    std::fs::create_dir_all(&config).expect("create config dir");
    let settings = config.join("settings.json");
    std::fs::write(&settings, ORIGINAL).expect("write invalid settings");

    let output = run_cli(root.path(), &settings, &["config", "set", "beam_size", "9"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "config set failed:\n{stderr}");

    let recovery = config.join("settings.json.bak");
    assert_eq!(
        std::fs::read(&recovery).expect("recovery copy exists"),
        ORIGINAL,
        "the recovery copy must hold the original bytes"
    );
    assert!(
        stderr.contains(&recovery.display().to_string()),
        "stderr must name the recovery copy {}:\n{stderr}",
        recovery.display()
    );

    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&settings).expect("read settings"))
            .expect("settings are valid JSON");
    assert_eq!(written["beam_size"], 9);
    assert_eq!(written["future_key"], serde_json::json!({"keep": true}));
    // Only the unreadable field took its default (#365).
    assert_eq!(written["show_overlay"], false);
    assert_eq!(written["language"], "sv");
    assert_eq!(written["auto_paste"], true);
    assert!(
        stderr.contains("auto_paste"),
        "stderr must name the field:\n{stderr}"
    );
    assert!(
        !stderr.contains("invalid-bool"),
        "stderr must not echo the value:\n{stderr}"
    );

    // The file is valid now, so a second change makes no further copy.
    let output = run_cli(root.path(), &settings, &["config", "set", "beam_size", "4"]);
    assert!(output.status.success());
    assert!(!config.join("settings.json.bak.1").exists());
    assert_eq!(std::fs::read(&recovery).unwrap(), ORIGINAL);
}
