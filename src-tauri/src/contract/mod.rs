//! Rust half of the Rust–TypeScript transport contract (#347, contracts/README.md).
//!
//! The shared fixtures in contracts/transport/ are written from real Rust
//! payloads by the tests that own each payload (`events_tests.rs`,
//! `commands/settings_tests.rs`, `meeting_jobs_tests.rs`) and parsed by the frontend's
//! Vitest suite against its TypeScript types (src/test/transport.test.ts).
//! `command_map` generates the frontend's `CommandMap` from the registered
//! command signatures. After an intended change, regenerate both with
//!
//! ```text
//! SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract
//! ```
//!
//! in src-tauri/ and review the diff: a changed fixture is a changed wire
//! format the frontend must accept.

mod command_map;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Set to regenerate the fixtures and the generated TypeScript instead of
/// comparing against them.
pub const BLESS_ENV: &str = "SAGASCRIPT_BLESS_CONTRACTS";

/// Every fixture in contracts/transport/, each written by one test.
pub const FIXTURES: &[&str] = &[
    "command_errors.json",
    "events.json",
    "get_meeting_job.json",
    "get_settings.json",
];

/// The commands that write settings (they reach `persist_and_sync` or a sibling),
/// and the functions that call the settings store's writers themselves.
pub(crate) use command_map::SettingsWriters;

/// See [`SettingsWriters`].
pub(crate) fn settings_writers() -> SettingsWriters {
    command_map::settings_writers()
}

/// The names of every command registered in `tauri::generate_handler!`.
pub fn registered_command_names() -> Vec<String> {
    command_map::registered_names()
}

pub fn blessing() -> bool {
    std::env::var_os(BLESS_ENV).is_some()
}

/// The repository root (src-tauri/ is the app crate's manifest directory).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent directory")
        .to_path_buf()
}

pub fn transport_dir() -> PathBuf {
    repo_root().join("contracts").join("transport")
}

/// Reads one shared fixture.
pub fn read_fixture(name: &str) -> Value {
    let path = transport_dir().join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
}

/// Compares `actual` with the shared fixture `name` as JSON values (so line
/// endings and formatting do not matter), or writes it when blessing.
pub fn assert_fixture(name: &str, actual: &Value) {
    assert!(
        FIXTURES.contains(&name),
        "{name} is not listed in contract::FIXTURES"
    );
    let path = transport_dir().join(name);
    if blessing() {
        let mut text = serde_json::to_string_pretty(actual).expect("JSON values serialize");
        text.push('\n');
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        return;
    }
    let expected = read_fixture(name);
    if &expected != actual {
        panic!(
            "contracts/transport/{name} no longer matches what Rust sends:\n{}\n\
             If the change is intended, regenerate with `{BLESS_ENV}=1 cargo test -p sagascript \
             contract` in src-tauri/, review the diff and update the TypeScript side \
             (src/test/transport.test.ts tells you what).",
            first_difference(&expected, actual, "$")
                .unwrap_or_else(|| "(no difference found)".into())
        );
    }
}

/// Serializes a payload the way Tauri does on the wire (serde_json).
pub fn to_value(payload: impl serde::Serialize) -> Value {
    serde_json::to_value(payload).expect("transport payloads serialize to JSON")
}

/// The first path where two JSON values differ, for a readable failure.
pub fn first_difference(expected: &Value, actual: &Value, path: &str) -> Option<String> {
    match (expected, actual) {
        (Value::Object(want), Value::Object(got)) => {
            for key in want.keys().chain(got.keys()) {
                let here = format!("{path}.{key}");
                match (want.get(key), got.get(key)) {
                    (Some(_), None) => return Some(format!("{here}: missing (fixture has it)")),
                    (None, Some(value)) => {
                        return Some(format!("{here}: unexpected, Rust sends {value}"))
                    }
                    (Some(want), Some(got)) => {
                        if let Some(difference) = first_difference(want, got, &here) {
                            return Some(difference);
                        }
                    }
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(want), Value::Array(got)) => {
            for (index, (want, got)) in want.iter().zip(got).enumerate() {
                if let Some(difference) = first_difference(want, got, &format!("{path}[{index}]")) {
                    return Some(difference);
                }
            }
            (want.len() != got.len()).then(|| {
                format!(
                    "{path}: fixture has {} items, Rust sends {}",
                    want.len(),
                    got.len()
                )
            })
        }
        _ => (expected != actual)
            .then(|| format!("{path}: fixture has {expected}, Rust sends {actual}")),
    }
}

#[test]
fn every_transport_fixture_is_listed() {
    let mut on_disk: Vec<String> = fs::read_dir(transport_dir())
        .expect("contracts/transport exists")
        .map(|entry| {
            entry
                .expect("readable directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    on_disk.sort();
    assert_eq!(
        on_disk, FIXTURES,
        "contracts/transport/ and contract::FIXTURES differ: list a new fixture (and \
         write it from a test), or delete a stale one"
    );
}

#[test]
fn first_difference_names_the_path() {
    let want = serde_json::json!({ "a": { "b": [1, 2] }, "c": "x" });
    assert_eq!(first_difference(&want, &want, "$"), None);
    let got = serde_json::json!({ "a": { "b": [1, 3] }, "c": "x" });
    assert_eq!(
        first_difference(&want, &got, "$").as_deref(),
        Some("$.a.b[1]: fixture has 2, Rust sends 3")
    );
    let renamed = serde_json::json!({ "a": { "b": [1, 2] }, "d": "x" });
    assert_eq!(
        first_difference(&want, &renamed, "$").as_deref(),
        Some("$.c: missing (fixture has it)")
    );
}
