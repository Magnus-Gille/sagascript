//! Owner regression for #340 and #365: a settings file with one wrong-typed
//! field must be preserved byte-for-byte before `config set` replaces it, and
//! the replacement keeps every other value. The CLI runs against an isolated
//! settings path in a temp dir.

use std::process::{Command, Output};

use sagascript_testsupport::{fake_settings, isolated_cli_env, FakeSettings};

const ORIGINAL: &[u8] = br#"{
  "show_overlay": false,
  "language": "sv",
  "beam_size": 3,
  "future_key": {"keep": true},
  "auto_paste": "invalid-bool"
}
"#;

fn run_cli(settings: &FakeSettings, args: &[&str]) -> Output {
    let home = settings.dir().join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    isolated_cli_env(&mut command, settings.path());
    // Every user directory points into the temp dir as well, so nothing
    // outside it can be read or written.
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
    command.args(args).output().expect("run sagascript CLI")
}

#[test]
fn config_set_preserves_a_schema_invalid_settings_file_before_replacing_it() {
    let settings = fake_settings();
    std::fs::write(settings.path(), ORIGINAL).expect("write invalid settings");

    let output = run_cli(&settings, &["config", "set", "beam_size", "9"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "config set failed:\n{stderr}");

    let recovery = settings.path().with_extension("json.bak");
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
        serde_json::from_slice(&std::fs::read(settings.path()).expect("read settings"))
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
    let output = run_cli(&settings, &["config", "set", "beam_size", "4"]);
    assert!(output.status.success());
    assert!(!settings.path().with_extension("json.bak.1").exists());
    assert_eq!(std::fs::read(&recovery).unwrap(), ORIGINAL);
}

/// The primary profile carries a language a newer release added. `hotkey`
/// and `language` mirror the primary profile, so dropping it would leave
/// `hotkey` naming a shortcut nothing registers, and `config reset hotkey`
/// would rewrite another profile's shortcut.
const UNREADABLE_PRIMARY: &[u8] = br#"{
  "language": "tlh",
  "hotkey": "Super+S",
  "hotkey_profiles": [
    {"id": "default", "name": "Klingon", "shortcut": "Super+S", "language": "tlh"},
    {"id": "english", "name": "English", "shortcut": "Super+E", "language": "en"}
  ],
  "profile_models": {"default": "auto", "english": "auto"},
  "profile_glossary_migrated": true,
  "model_lineup_version": 1,
  "engine_defaults_version": 1,
  "schema_version": 6
}
"#;

#[test]
fn an_unreadable_primary_profile_keeps_its_shortcut_and_hotkey_reset_targets_it() {
    let settings = fake_settings();
    std::fs::write(settings.path(), UNREADABLE_PRIMARY).expect("write settings");

    let output = run_cli(&settings, &["config", "get", "hotkey"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "Super+S");

    let output = run_cli(&settings, &["config", "profiles", "list"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let ids_and_shortcuts: Vec<(String, String)> = stdout
        .lines()
        .skip(1)
        .map(|line| {
            let mut columns = line.split_whitespace();
            let id = columns.next().unwrap_or_default().to_string();
            // NAME is a single word in this file.
            let shortcut = columns.nth(1).unwrap_or_default().to_string();
            (id, shortcut)
        })
        .collect();
    assert_eq!(
        ids_and_shortcuts,
        [
            ("default".to_string(), "Super+S".to_string()),
            ("english".to_string(), "Super+E".to_string())
        ],
        "{stdout}"
    );
    // Reading changes nothing on disk.
    assert_eq!(std::fs::read(settings.path()).unwrap(), UNREADABLE_PRIMARY);

    let output = run_cli(&settings, &["config", "reset", "hotkey"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "config reset hotkey failed:\n{stderr}"
    );
    assert_eq!(
        std::fs::read(settings.path().with_extension("json.bak")).expect("recovery copy exists"),
        UNREADABLE_PRIMARY
    );
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(settings.path()).expect("read settings"))
            .expect("settings are valid JSON");
    let profiles = written["hotkey_profiles"].as_array().expect("profiles");
    assert_eq!(profiles.len(), 2, "{written}");
    assert_eq!(profiles[0]["id"], "default");
    assert_eq!(profiles[0]["name"], "Klingon");
    assert_eq!(profiles[0]["shortcut"], "Control+Shift+Space");
    assert_eq!(profiles[1]["id"], "english");
    assert_eq!(profiles[1]["shortcut"], "Super+E");
    assert_eq!(written["hotkey"], "Control+Shift+Space");
}
