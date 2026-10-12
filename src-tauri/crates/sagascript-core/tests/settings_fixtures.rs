//! Golden corpus of historical settings files (see fixtures/settings/README.md).
//!
//! Every fixture goes through the startup load with fixed machine state. The
//! result must deserialize strictly, keep the user's values, hold no legacy
//! keys, be idempotent and match the fixture's `expected/` snapshot.
//!
//! After an intended change, regenerate the snapshots with
//! `SAGASCRIPT_BLESS_SETTINGS=1 cargo test -p sagascript-core --test settings_fixtures`
//! and review the diff.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sagascript_core::settings::{store, LineupEnv, Settings, WhisperModel};
use serde_json::Value;

const SETTINGS_FILE: &str = "sagascript-settings.json";
const BLESS_ENV: &str = "SAGASCRIPT_BLESS_SETTINGS";
/// Keys that only releases before the current shape wrote.
const LEGACY_KEYS: &[&str] = &[
    "hasCompletedOnboarding",
    "initial_prompt",
    "profile_glossaries",
];

/// One fixture and the top-level keys whose value a migration is meant to
/// change. Every other value in the input must survive.
struct Fixture {
    name: &'static str,
    migrated_keys: &'static [&'static str],
}

const FIXTURES: &[Fixture] = &[
    // A new bundle identity needs Accessibility approval again.
    Fixture {
        name: "pre-1.0",
        migrated_keys: &["auto_paste"],
    },
    Fixture {
        name: "v1.0.0",
        migrated_keys: &[],
    },
    Fixture {
        name: "v1.0.6",
        migrated_keys: &[],
    },
    Fixture {
        name: "v1.1.0",
        migrated_keys: &[],
    },
    Fixture {
        name: "v1.2.0",
        migrated_keys: &[],
    },
    Fixture {
        name: "v1.3.0",
        migrated_keys: &[],
    },
    // Users still on the old warm-up defaults move to the new ones.
    Fixture {
        name: "v1.4.0",
        migrated_keys: &["engine_prewarm", "engine_idle_unload_minutes"],
    },
    Fixture {
        name: "v1.4.2",
        migrated_keys: &[],
    },
];

fn downloaded(model: WhisperModel) -> bool {
    matches!(
        model,
        WhisperModel::Base
            | WhisperModel::BaseEn
            | WhisperModel::KbWhisperBase
            | WhisperModel::KbWhisperSmall
    )
}

fn machine() -> LineupEnv<'static> {
    LineupEnv {
        pianissimo_supported: true,
        pianissimo_downloaded: true,
        whisper_downloaded: &downloaded,
    }
}

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/settings")
        .join(name)
}

/// The settings file and legacy locations under a copied `input/` tree, in
/// the order the store searches them.
struct Layout {
    settings: PathBuf,
    legacy: Vec<PathBuf>,
}

fn layout(root: &Path) -> Layout {
    let data = root.join("data");
    Layout {
        settings: root.join("config").join("sagascript").join(SETTINGS_FILE),
        legacy: vec![
            data.join("ai.gille.sagascript").join(SETTINGS_FILE),
            data.join("com.sagascript.app").join(SETTINGS_FILE),
            data.join("ai.gille.sagascript")
                .join("flowdictate-settings.json"),
        ],
    }
}

/// Every file under `dir`, by path relative to it. Lock files are left out:
/// they only exist to serialize writers.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if path.extension().is_none_or(|extension| extension != "lock") {
                let relative = path.strip_prefix(base).unwrap();
                let key = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(key, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

fn copy_tree(from: &Path, to: &Path) {
    for (relative, contents) in files(from) {
        let target = to.join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, contents).unwrap();
    }
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// `value` with every object's keys in sorted order, for stable snapshots
/// whether or not serde_json preserves insertion order.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let entries: BTreeMap<String, Value> =
                map.into_iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn pretty(value: Value) -> Vec<u8> {
    let mut text = serde_json::to_string_pretty(&sorted(value)).unwrap();
    text.push('\n');
    text.into_bytes()
}

fn settings_json(settings: &Settings) -> Value {
    serde_json::to_value(settings).unwrap()
}

/// The fixture's input settings file and where it lies under `input/`.
fn input_settings(name: &str) -> (String, Value) {
    let (relative, contents) = files(&fixture_dir(name).join("input"))
        .into_iter()
        .find(|(relative, _)| relative.ends_with(".json"))
        .unwrap_or_else(|| panic!("{name}: no input settings file"));
    (relative, json(&contents))
}

/// What a fixture produces.
struct Outcome {
    config: BTreeMap<String, Vec<u8>>,
    loaded: Value,
    load_from: Value,
}

fn run(name: &str) -> Outcome {
    let input = fixture_dir(name).join("input");

    // Startup load: legacy locations are imported, then migrated.
    let root = tempfile::tempdir().unwrap();
    copy_tree(&input, root.path());
    let paths = layout(root.path());
    let loaded = store::load_at(&paths.settings, &paths.legacy, &machine());
    let config = files(&root.path().join("config"));

    // Rollback sources stay as they were.
    assert_eq!(
        files(&root.path().join("data")),
        files(&input.join("data")),
        "{name}: a legacy settings file was modified"
    );

    // Idempotent: a second startup changes nothing.
    let again = store::load_at(&paths.settings, &paths.legacy, &machine());
    assert_eq!(
        files(&root.path().join("config")),
        config,
        "{name}: a second startup load rewrote the configuration"
    );
    assert_eq!(settings_json(&again), settings_json(&loaded), "{name}");

    // The migrated file is complete: a plain load returns the same settings.
    assert_eq!(
        settings_json(&store::load_from(&paths.settings)),
        settings_json(&loaded),
        "{name}: reloading the migrated file gives different settings"
    );

    // Plain load (no machine state) of the input at the current location.
    let plain = tempfile::tempdir().unwrap();
    copy_tree(&input, plain.path());
    let plain_paths = layout(plain.path());
    let (relative, _) = input_settings(name);
    if relative.starts_with("data/") {
        fs::create_dir_all(plain_paths.settings.parent().unwrap()).unwrap();
        fs::copy(plain.path().join(&relative), &plain_paths.settings).unwrap();
    }
    let load_from = settings_json(&store::load_from(&plain_paths.settings));

    Outcome {
        config,
        loaded: settings_json(&loaded),
        load_from,
    }
}

fn check(name: &str) {
    let fixture = FIXTURES
        .iter()
        .find(|fixture| fixture.name == name)
        .unwrap_or_else(|| panic!("{name} is not in FIXTURES"));
    let outcome = run(name);
    let migrated_bytes = outcome
        .config
        .get(&format!("sagascript/{SETTINGS_FILE}"))
        .unwrap_or_else(|| panic!("{name}: no settings file after startup"));
    let migrated = json(migrated_bytes);

    // Strict: the current struct reads it without any migration.
    serde_json::from_slice::<Settings>(migrated_bytes)
        .unwrap_or_else(|error| panic!("{name}: migrated file is not strictly valid: {error}"));
    for key in LEGACY_KEYS {
        assert!(
            migrated.get(*key).is_none(),
            "{name}: legacy key {key} remains"
        );
    }

    // User values survive.
    let (_, input) = input_settings(name);
    for (key, value) in input.as_object().unwrap() {
        match key.as_str() {
            "hasCompletedOnboarding" => {
                assert_eq!(&migrated["has_completed_onboarding"], value, "{name}")
            }
            "initial_prompt" => {
                assert_eq!(&outcome.loaded["initial_prompt"], value, "{name}");
                assert_eq!(
                    outcome.config["sagascript/glossary.txt"],
                    format!("{}\n", value.as_str().unwrap()).into_bytes(),
                    "{name}: global dictionary"
                );
            }
            key if fixture.migrated_keys.contains(&key) => assert_ne!(
                &migrated[key], value,
                "{name}: {key} was expected to migrate"
            ),
            _ => assert_eq!(&migrated[key], value, "{name}: {key} changed"),
        }
    }

    compare_or_bless(name, &outcome);
}

fn compare_or_bless(name: &str, outcome: &Outcome) {
    let expected = fixture_dir(name).join("expected");
    let mut actual: BTreeMap<String, Vec<u8>> = outcome
        .config
        .iter()
        .map(|(relative, contents)| {
            let contents = if relative.ends_with(".json") {
                pretty(json(contents))
            } else {
                contents.clone()
            };
            (format!("config/{relative}"), contents)
        })
        .collect();
    actual.insert("loaded.json".into(), pretty(outcome.loaded.clone()));
    actual.insert("load_from.json".into(), pretty(outcome.load_from.clone()));

    if std::env::var_os(BLESS_ENV).is_some() {
        let _ = fs::remove_dir_all(&expected);
        for (relative, contents) in &actual {
            let path = expected.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        return;
    }

    let wanted = files(&expected);
    assert_eq!(
        wanted.keys().collect::<Vec<_>>(),
        actual.keys().collect::<Vec<_>>(),
        "{name}: different files than expected/ (set {BLESS_ENV}=1 to regenerate)"
    );
    for (relative, contents) in &actual {
        let want = &wanted[relative];
        if relative.ends_with(".json") {
            assert_eq!(
                json(contents),
                json(want),
                "{name}: {relative} differs from expected/ (set {BLESS_ENV}=1 to regenerate)"
            );
        } else {
            assert_eq!(
                String::from_utf8_lossy(contents),
                String::from_utf8_lossy(want),
                "{name}: {relative} differs from expected/"
            );
        }
    }
}

#[test]
fn every_fixture_directory_is_listed() {
    let mut on_disk: Vec<String> = fs::read_dir(fixture_dir(""))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = FIXTURES.iter().map(|f| f.name.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

#[test]
fn pre_1_0() {
    check("pre-1.0");
}

#[test]
fn v1_0_0() {
    check("v1.0.0");
}

#[test]
fn v1_0_6() {
    check("v1.0.6");
}

#[test]
fn v1_1_0() {
    check("v1.1.0");
}

#[test]
fn v1_2_0() {
    check("v1.2.0");
}

#[test]
fn v1_3_0() {
    check("v1.3.0");
}

#[test]
fn v1_4_0() {
    check("v1.4.0");
}

#[test]
fn v1_4_2() {
    check("v1.4.2");
}
