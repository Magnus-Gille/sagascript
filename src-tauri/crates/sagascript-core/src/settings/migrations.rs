//! How the settings file evolves: its `schema_version` and the ordered steps
//! that bring an older file up to date (ADR 0007).
//!
//! | Version | Release | Step | Legacy record |
//! |---|---|---|---|
//! | 1 | 1.0.0 | `hasCompletedOnboarding` becomes `has_completed_onboarding` | key absent |
//! | 2 | 1.1.0 | personal dictionaries live in their own files | no embedded `initial_prompt` / `profile_glossaries` |
//! | 3 | 1.4.0 | every dictation profile owns its live model | `hotkey_profiles` and `profile_models` complete |
//! | 4 | 1.4.0 | global dictionary hints copied into every profile dictionary | `profile_glossary_migrated` |
//! | 5 | 1.4.0 | Swedish model lineup (needs machine state) | `model_lineup_version` |
//! | 6 | 1.4.2 | engine warm-up defaults | `engine_defaults_version` |
//!
//! Steps operate on the settings object: the JSON object of the settings
//! file with its personal dictionaries read in by the store. Each step is a
//! plain function, testable on its own, and idempotent.
//!
//! **Where a file starts.** [`starting_version`] reads `schema_version` and
//! lowers it to just below the first step the file does not record in its
//! legacy form. A file without `schema_version` (written before it existed)
//! therefore starts where its markers and shape put it. A file an older
//! release rewrote starts where that release may have left it: such a
//! release keeps `schema_version` as an unknown key but can, for example, add
//! a profile without a model or embed `initial_prompt` again.
//!
//! **Downgrade safety.** Releases up to 1.4.3 decide their migrations from
//! the legacy records alone. The steps keep writing them (`Settings` keeps
//! the marker fields), so an older release reading a file written here finds
//! nothing left to migrate. A step that changes a user's value must keep a
//! marker of its own for the same reason: re-running it after an older
//! release rewrote the file must change nothing.
//!
//! **Machine state.** The Swedish lineup step needs to know what is
//! downloaded and whether Pianissimo runs. The store injects it through
//! [`MachineProbe`] on the startup load only, and the probe runs only while
//! that step is pending. A load without machine state (field updates,
//! `store::load_from`) applies the steps before it, as before this module
//! existed; the later ones wait for the next startup load.

use serde_json::{Map, Value};

use super::{LineupEnv, Settings, ENGINE_DEFAULTS_VERSION, MODEL_LINEUP_VERSION};

/// The settings object the steps operate on.
pub(crate) type Object = Map<String, Value>;

/// The settings schema version this build reads and writes.
pub const SCHEMA_VERSION: u32 = 6;

/// Settings-file key that records the schema version.
pub(crate) const SCHEMA_VERSION_KEY: &str = "schema_version";

/// The newest version a file without `schema_version` can be at. Every step
/// up to this one has a legacy record that [`starting_version`] checks.
const UNVERSIONED_MAX: u32 = 6;
const _: () = assert!(UNVERSIONED_MAX <= SCHEMA_VERSION);

/// Pre-release name of the onboarding flag (Tauri plugin store).
const LEGACY_ONBOARDING_KEY: &str = "hasCompletedOnboarding";
const ONBOARDING_KEY: &str = "has_completed_onboarding";

/// A step could not migrate the settings object.
#[derive(Debug)]
pub(crate) struct MigrationError {
    step: &'static str,
    reason: String,
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} migration failed: {}", self.step, self.reason)
    }
}

/// Machine state a migration step may need. Probed lazily: only a pending
/// Swedish lineup step asks for [`MachineProbe::lineup_env`].
pub(crate) trait MachineProbe {
    /// `pianissimo_backend::runtime_supported_on_this_os()`.
    fn pianissimo_supported(&self) -> bool;
    /// What the Swedish model-lineup step needs to know about this machine.
    fn lineup_env(&self) -> LineupEnv<'_>;
}

/// Fixed machine state, for tests.
impl MachineProbe for LineupEnv<'_> {
    fn pianissimo_supported(&self) -> bool {
        self.pianissimo_supported
    }

    fn lineup_env(&self) -> LineupEnv<'_> {
        LineupEnv {
            pianissimo_supported: self.pianissimo_supported,
            pianissimo_downloaded: self.pianissimo_downloaded,
            whisper_downloaded: self.whisper_downloaded,
        }
    }
}

/// A step's error is its reason; [`migrate`] names the step.
type PureStep = fn(&mut Object) -> Result<(), String>;
type MachineStep = fn(&mut Object, &dyn MachineProbe) -> Result<(), String>;

enum Apply {
    Pure(PureStep),
    Machine(MachineStep),
}

struct Step {
    /// The version a file is at once this step has run.
    version: u32,
    name: &'static str,
    /// Whether a file records this step in its legacy form (see
    /// [`starting_version`]). Checked on the file's object as read.
    recorded: fn(&Object) -> bool,
    apply: Apply,
}

/// The steps, oldest first. Append new ones; never reorder or renumber.
const STEPS: &[Step] = &[
    Step {
        version: 1,
        name: "onboarding key",
        recorded: |object| !object.contains_key(LEGACY_ONBOARDING_KEY),
        apply: Apply::Pure(|object| {
            canonical_onboarding_key(object);
            Ok(())
        }),
    },
    Step {
        version: 2,
        name: "dictionary files",
        recorded: |object| {
            !object.contains_key("initial_prompt") && !object.contains_key("profile_glossaries")
        },
        apply: Apply::Pure(dictionary_files),
    },
    Step {
        version: 3,
        name: "profile models",
        recorded: profile_models_recorded,
        apply: Apply::Pure(profile_models),
    },
    Step {
        version: 4,
        name: "profile dictionaries",
        recorded: |object| object.get("profile_glossary_migrated") == Some(&Value::Bool(true)),
        apply: Apply::Pure(profile_dictionaries),
    },
    Step {
        version: 5,
        name: "Swedish model lineup",
        recorded: |object| marker(object, "model_lineup_version") >= MODEL_LINEUP_VERSION,
        apply: Apply::Machine(swedish_lineup),
    },
    Step {
        version: 6,
        name: "engine warm-up defaults",
        recorded: |object| marker(object, "engine_defaults_version") >= ENGINE_DEFAULTS_VERSION,
        apply: Apply::Pure(engine_defaults),
    },
];

/// The first version whose step needs machine state; without it, the
/// migration stops before this version.
fn first_machine_version() -> u32 {
    STEPS
        .iter()
        .find(|step| matches!(step.apply, Apply::Machine(_)))
        .map_or(u32::MAX, |step| step.version)
}

fn marker(object: &Object, key: &str) -> u32 {
    object
        .get(key)
        .and_then(Value::as_u64)
        .map_or(0, |value| u32::try_from(value).unwrap_or(u32::MAX))
}

/// The version a settings file is at, from its object as read: its
/// `schema_version` (or [`UNVERSIONED_MAX`] without one), lowered to just
/// below the first step whose legacy record it lacks.
pub(crate) fn starting_version(object: &Object) -> u32 {
    let recorded = object
        .get(SCHEMA_VERSION_KEY)
        .and_then(Value::as_u64)
        .map_or(UNVERSIONED_MAX, |version| {
            u32::try_from(version).unwrap_or(u32::MAX)
        });
    let shape = STEPS
        .iter()
        .find(|step| !(step.recorded)(object))
        .map_or(u32::MAX, |step| step.version - 1);
    recorded.min(shape)
}

/// What [`migrate`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Migration {
    pub from: u32,
    pub to: u32,
}

impl Migration {
    /// Whether any step changed the file's version, so the file needs writing.
    pub fn ran(&self) -> bool {
        self.to > self.from
    }
}

/// Run the steps after `from` (see [`starting_version`], which must be taken
/// from the file's object before the store reads the dictionaries in) and
/// record the version reached as `schema_version`.
///
/// Without machine state the steps from the first one that needs it are not
/// applied; they only count as reached while the object records them.
/// A file from a newer release is recorded as [`SCHEMA_VERSION`]: once this
/// build writes it, the newer release must check it again.
pub(crate) fn migrate(
    object: &mut Object,
    from: u32,
    machine: Option<&dyn MachineProbe>,
) -> Result<Migration, MigrationError> {
    object.remove(SCHEMA_VERSION_KEY);
    let machine_from = machine.map_or_else(first_machine_version, |_| u32::MAX);
    let mut reached = from;
    for step in STEPS.iter().filter(|step| step.version > from) {
        let result = match (&step.apply, machine) {
            _ if step.version >= machine_from => None,
            (Apply::Pure(apply), _) => Some(apply(object)),
            (Apply::Machine(apply), Some(machine)) => Some(apply(object, machine)),
            (Apply::Machine(_), None) => None,
        };
        let applied = match result {
            Some(Ok(())) => true,
            Some(Err(reason)) => {
                return Err(MigrationError {
                    step: step.name,
                    reason,
                })
            }
            None => false,
        };
        if !applied && !(step.recorded)(object) {
            break;
        }
        reached = step.version;
    }
    let to = reached.min(SCHEMA_VERSION);
    object.insert(SCHEMA_VERSION_KEY.to_string(), Value::from(to));
    Ok(Migration { from, to })
}

/// Step 1. Pre-release builds stored the onboarding flag under the Tauri
/// plugin store's camel-case key. The canonical key wins if both exist.
/// The store also applies this to the object it merges a save into, so a
/// written file never holds both.
pub(crate) fn canonical_onboarding_key(object: &mut Object) {
    if let Some(legacy) = object.remove(LEGACY_ONBOARDING_KEY) {
        object.entry(ONBOARDING_KEY.to_string()).or_insert(legacy);
    }
}

/// Step 2. Since 1.1.0 the personal dictionaries live in `glossary.txt` and
/// `glossaries/<profile>.txt`: the store reads them in, over any embedded
/// values, and every save writes them there and drops the embedded keys.
/// What remains here is an embedded profile dictionary whose id cannot name
/// a file: it is dropped.
fn dictionary_files(object: &mut Object) -> Result<(), String> {
    if let Some(Value::Object(dictionaries)) = object.get_mut("profile_glossaries") {
        dictionaries.retain(|profile_id, _| {
            if let Err(error) = super::store::validate_profile_file_id(profile_id) {
                tracing::warn!("Ignoring embedded {error}");
                false
            } else {
                true
            }
        });
    }
    Ok(())
}

/// Legacy record of step 3: every profile has a valid model entry. A
/// profile an older release added without one makes the step run again.
fn profile_models_recorded(object: &Object) -> bool {
    let Some(profiles) = object.get("hotkey_profiles").and_then(Value::as_array) else {
        return false;
    };
    let Some(models) = object.get("profile_models").and_then(Value::as_object) else {
        return false;
    };
    !profiles.is_empty()
        && profiles.iter().all(|profile| {
            profile.get("id").and_then(Value::as_str).is_none_or(|id| {
                models
                    .get(id)
                    .and_then(Value::as_str)
                    .is_some_and(|model| super::FileModelPreference::parse_id(model).is_ok())
            })
        })
}

/// Step 3. A file without explicit profiles gets its default profile, and
/// each profile without a model gets the one the old global selection gave
/// it (see [`Settings::materialize_profile_models`]).
fn profile_models(object: &mut Object) -> Result<(), String> {
    with_settings(object, Settings::materialize_profile_models)
}

/// Step 4. The global dictionary's entries become hints in every profile
/// dictionary (see [`Settings::migrate_global_glossary_to_profiles`]).
fn profile_dictionaries(object: &mut Object) -> Result<(), String> {
    with_settings(object, Settings::migrate_global_glossary_to_profiles)
}

/// Step 5. Swedish profiles move off the retired KB-Whisper models where
/// that keeps dictation working on this machine (see
/// [`Settings::migrate_swedish_lineup`]).
fn swedish_lineup(object: &mut Object, machine: &dyn MachineProbe) -> Result<(), String> {
    with_settings(object, |settings| {
        // Probe the machine only while the one-time migration is pending.
        if settings.model_lineup_version < MODEL_LINEUP_VERSION {
            settings.migrate_swedish_lineup(&machine.lineup_env());
        }
    })
}

/// Step 6. Users still on the old engine warm-up defaults move to the new
/// ones (see [`Settings::migrate_engine_defaults`]).
fn engine_defaults(object: &mut Object) -> Result<(), String> {
    with_settings(object, |settings| {
        settings.migrate_engine_defaults();
    })
}

/// Run typed migration logic on the settings `object` holds and write back
/// the top-level keys whose value it changed. Every other key, known or
/// not, stays exactly as it was. A field that does not decode is read as
/// its default (see [`super::lenient`]) and never written back: it stays for
/// the store's final decode to reset and report.
fn with_settings(object: &mut Object, migrate: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let decoded = super::lenient::decode(object)?;
    let before = decoded.settings;
    let mut after = before.clone();
    migrate(&mut after);
    let as_object = |settings: &Settings| match serde_json::to_value(settings) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err("settings did not serialize as an object".to_string()),
        Err(e) => Err(e.to_string()),
    };
    let before = as_object(&before)?;
    for (key, value) in as_object(&after)? {
        if before.get(&key) != Some(&value) && !decoded.reset.contains(&key) {
            object.insert(key, value);
        }
    }
    Ok(())
}

/// Settings found at a legacy location (an earlier bundle identifier or
/// the data directory used before 1.1.0), as they are copied to the current
/// location. A different bundle identity must be approved for Accessibility
/// again before it may paste, so `auto_paste` is turned off unless the file
/// belongs to the same bundle. The versioned steps run on the next load.
pub(crate) fn from_legacy_location(object: &mut Object, same_bundle: bool) {
    // As the copy has always done, so a copied file looks as it did.
    canonical_onboarding_key(object);
    if !same_bundle {
        object.insert("auto_paste".to_string(), Value::Bool(false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{EnginePrewarm, FileModelPreference, Language, WhisperModel};
    use serde_json::json;

    fn object(value: Value) -> Object {
        match value {
            Value::Object(object) => object,
            other => panic!("not an object: {other}"),
        }
    }

    fn ready(_: WhisperModel) -> bool {
        true
    }

    /// Pianissimo supported and every model downloaded.
    fn machine() -> LineupEnv<'static> {
        LineupEnv {
            pianissimo_supported: true,
            pianissimo_downloaded: true,
            whisper_downloaded: &ready,
        }
    }

    /// The object as the store writes it: without the personal
    /// dictionaries, which go to their own files.
    fn on_disk(object: &Object) -> Object {
        let mut written = object.clone();
        written.remove("initial_prompt");
        written.remove("profile_glossaries");
        written
    }

    /// Migrate a settings object the way the store does, starting where the
    /// file it came from (`disk`) is.
    fn migrate_from_disk(object: &mut Object, disk: &Object) -> Migration {
        let from = starting_version(disk);
        migrate(object, from, Some(&machine())).unwrap()
    }

    fn migrate_all(object: &mut Object) -> Migration {
        let disk = object.clone();
        migrate_from_disk(object, &disk)
    }

    fn fixture(name: &str) -> Object {
        fixture_inputs()
            .into_iter()
            .find(|(fixture, _)| *fixture == name)
            .map(|(_, object)| object)
            .unwrap()
    }

    /// The settings file every fixture's input holds (see
    /// tests/fixtures/settings/README.md), by fixture name.
    fn fixture_inputs() -> Vec<(&'static str, Object)> {
        macro_rules! fixture {
            ($name:literal, $path:literal) => {
                (
                    $name,
                    object(
                        serde_json::from_str(include_str!(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/tests/fixtures/settings/",
                            $name,
                            "/input/",
                            $path
                        )))
                        .unwrap(),
                    ),
                )
            };
        }
        vec![
            fixture!(
                "pre-1.0",
                "data/com.sagascript.app/sagascript-settings.json"
            ),
            fixture!(
                "v1.0.0",
                "data/ai.gille.sagascript/sagascript-settings.json"
            ),
            fixture!(
                "v1.0.6",
                "data/ai.gille.sagascript/sagascript-settings.json"
            ),
            fixture!("v1.1.0", "config/sagascript/sagascript-settings.json"),
            fixture!("v1.2.0", "config/sagascript/sagascript-settings.json"),
            fixture!("v1.3.0", "config/sagascript/sagascript-settings.json"),
            fixture!("v1.4.0", "config/sagascript/sagascript-settings.json"),
            fixture!("v1.4.2", "config/sagascript/sagascript-settings.json"),
        ]
    }

    #[test]
    fn steps_are_numbered_one_by_one_up_to_the_schema_version() {
        let versions: Vec<u32> = STEPS.iter().map(|step| step.version).collect();
        assert_eq!(versions, (1..=SCHEMA_VERSION).collect::<Vec<_>>());
        assert_eq!(first_machine_version(), 5);
    }

    #[test]
    fn fixtures_start_where_their_release_left_them() {
        let starts: Vec<(&str, u32)> = fixture_inputs()
            .iter()
            .map(|(name, object)| (*name, starting_version(object)))
            .collect();
        assert_eq!(
            starts,
            [
                ("pre-1.0", 0),
                ("v1.0.0", 1),
                ("v1.0.6", 1),
                ("v1.1.0", 2),
                ("v1.2.0", 2),
                ("v1.3.0", 2),
                ("v1.4.0", 5),
                ("v1.4.2", 6),
            ]
        );
    }

    #[test]
    fn schema_version_is_lowered_by_a_missing_legacy_record_but_never_raised() {
        // An older release kept schema_version but added a profile without a model.
        let rewritten = object(json!({
            "schema_version": 6,
            "profile_glossary_migrated": true,
            "model_lineup_version": 1,
            "engine_defaults_version": 1,
            "hotkey_profiles": [{"id": "default", "name": "D", "shortcut": "Option+Space", "language": "sv"},
                                {"id": "new", "name": "N", "shortcut": "Option+N", "language": "en"}],
            "profile_models": {"default": "auto"}
        }));
        assert_eq!(starting_version(&rewritten), 2);
        // A version below what the markers show is kept.
        let mut lower = rewritten.clone();
        lower.insert("schema_version".into(), json!(1));
        assert_eq!(starting_version(&lower), 1);
        // A newer release's version is kept as the start: nothing runs.
        let mut newer = object(json!({
            "schema_version": 9,
            "profile_glossary_migrated": true,
            "model_lineup_version": 1,
            "engine_defaults_version": 1,
            "hotkey_profiles": [{"id": "default", "name": "D", "shortcut": "Option+Space", "language": "sv"}],
            "profile_models": {"default": "auto"}
        }));
        assert_eq!(starting_version(&newer), 9);
        let before = newer.clone();
        let migration = migrate_all(&mut newer);
        assert!(!migration.ran());
        assert_eq!(migration.to, SCHEMA_VERSION);
        newer.insert("schema_version".into(), json!(9));
        assert_eq!(newer, before);
        // Not a number: as if absent.
        let mut odd = before;
        odd.insert("schema_version".into(), json!("six"));
        assert_eq!(starting_version(&odd), UNVERSIONED_MAX);
    }

    #[test]
    fn onboarding_key_step_keeps_the_canonical_value() {
        let mut legacy = object(json!({"hasCompletedOnboarding": true, "future": 1}));
        canonical_onboarding_key(&mut legacy);
        assert_eq!(
            legacy,
            object(json!({"has_completed_onboarding": true, "future": 1}))
        );
        let mut both =
            object(json!({"hasCompletedOnboarding": false, "has_completed_onboarding": true}));
        canonical_onboarding_key(&mut both);
        assert_eq!(both, object(json!({"has_completed_onboarding": true})));
    }

    #[test]
    fn dictionary_files_step_drops_embedded_dictionaries_that_cannot_be_files() {
        let mut embedded = object(json!({
            "initial_prompt": "OpenRouter",
            "profile_glossaries": {"Bad/Id": "unsafe", "swedish": "merge = merch"}
        }));
        dictionary_files(&mut embedded).unwrap();
        assert_eq!(
            embedded,
            object(json!({
                "initial_prompt": "OpenRouter",
                "profile_glossaries": {"swedish": "merge = merch"}
            }))
        );
    }

    #[test]
    fn profile_models_step_materializes_the_old_global_selection() {
        let mut legacy = object(json!({
            "language": "sv",
            "hotkey": "Super+S",
            "pianissimo_dictation": true,
            "auto_select_model": false,
            "whisper_model": "kb-whisper-small",
            "hotkey_profiles": [
                {"id": "default", "name": "Swedish", "shortcut": "Super+S", "language": "sv"},
                {"id": "english", "name": "English", "shortcut": "Super+E", "language": "en"}
            ],
            "future_key": [1, 2]
        }));
        profile_models(&mut legacy).unwrap();
        assert_eq!(
            legacy["profile_models"],
            json!({"default": "pianissimo-sv", "english": "auto"})
        );
        assert_eq!(legacy["future_key"], json!([1, 2]));
        assert!(profile_models_recorded(&legacy));

        // No profiles yet: the default profile is created from the legacy fields.
        let mut single = object(
            json!({"language": "no", "hotkey": "Option+Space", "auto_select_model": false, "whisper_model": "nb-whisper-small"}),
        );
        profile_models(&mut single).unwrap();
        assert_eq!(single["hotkey_profiles"][0]["id"], "default");
        assert_eq!(single["hotkey_profiles"][0]["shortcut"], "Option+Space");
        assert_eq!(single["hotkey_profiles"][0]["language"], "no");
        assert_eq!(
            single["profile_models"],
            json!({"default": "nb-whisper-small"})
        );
        // An existing choice is never replaced.
        let mut chosen = single.clone();
        chosen.insert(
            "profile_models".into(),
            json!({"default": "nb-whisper-large"}),
        );
        let before = chosen.clone();
        profile_models(&mut chosen).unwrap();
        assert_eq!(chosen, before);
    }

    #[test]
    fn profile_dictionaries_step_copies_global_hints_once() {
        let mut legacy = object(json!({
            "initial_prompt": "OpenRouter = open router",
            "hotkey_profiles": [
                {"id": "default", "name": "Swedish", "shortcut": "Super+S", "language": "sv"},
                {"id": "english", "name": "English", "shortcut": "Super+E", "language": "en"}
            ],
            "profile_glossaries": {"default": "merge = merch"}
        }));
        profile_dictionaries(&mut legacy).unwrap();
        assert_eq!(
            legacy["profile_glossaries"],
            json!({"default": "merge = merch\nOpenRouter", "english": "OpenRouter"})
        );
        assert_eq!(legacy["profile_glossary_migrated"], true);
        // The marker makes a second run a no-op even after the user edits.
        legacy["profile_glossaries"]["english"] = json!("");
        let before = legacy.clone();
        profile_dictionaries(&mut legacy).unwrap();
        assert_eq!(legacy, before);
    }

    #[test]
    fn swedish_lineup_step_probes_the_machine_only_while_pending() {
        struct Probe<'a> {
            calls: std::cell::Cell<u32>,
            env: LineupEnv<'a>,
        }
        impl MachineProbe for Probe<'_> {
            fn pianissimo_supported(&self) -> bool {
                self.env.pianissimo_supported
            }
            fn lineup_env(&self) -> LineupEnv<'_> {
                self.calls.set(self.calls.get() + 1);
                self.env.lineup_env()
            }
        }
        let probe = Probe {
            calls: std::cell::Cell::new(0),
            env: machine(),
        };
        let mut pending = object(json!({
            "language": "sv",
            "hotkey_profiles": [{"id": "default", "name": "Swedish", "shortcut": "Super+S", "language": "sv"}],
            "profile_models": {"default": "kb-whisper-small"}
        }));
        swedish_lineup(&mut pending, &probe).unwrap();
        assert_eq!(probe.calls.get(), 1);
        assert_eq!(pending["profile_models"]["default"], "pianissimo-sv");
        assert_eq!(pending["model_lineup_version"], MODEL_LINEUP_VERSION);

        // Recorded: no probe, and an explicit later choice of a retired model stays.
        pending["profile_models"]["default"] = json!("kb-whisper-small");
        let before = pending.clone();
        swedish_lineup(&mut pending, &probe).unwrap();
        assert_eq!(probe.calls.get(), 1);
        assert_eq!(pending, before);
    }

    #[test]
    fn engine_defaults_step_moves_only_the_old_defaults_once() {
        let mut old =
            object(json!({"engine_prewarm": "on_key_down", "engine_idle_unload_minutes": 10}));
        engine_defaults(&mut old).unwrap();
        assert_eq!(
            old,
            object(json!({
                "engine_prewarm": "on_app_start",
                "engine_idle_unload_minutes": 60,
                "engine_defaults_version": ENGINE_DEFAULTS_VERSION
            }))
        );
        let mut chosen = object(json!({"engine_prewarm": "off", "engine_idle_unload_minutes": 5}));
        engine_defaults(&mut chosen).unwrap();
        assert_eq!(chosen["engine_prewarm"], "off");
        assert_eq!(chosen["engine_idle_unload_minutes"], 5);
        // Recorded: the old values picked again on purpose are kept.
        let mut again = object(
            json!({"engine_prewarm": "on_key_down", "engine_idle_unload_minutes": 10, "engine_defaults_version": 1}),
        );
        let before = again.clone();
        engine_defaults(&mut again).unwrap();
        assert_eq!(again, before);
    }

    #[test]
    fn without_machine_state_the_lineup_and_later_steps_wait() {
        // A 1.3 file: steps 3 and 4 run, the lineup and engine steps do not.
        let mut legacy = fixture("v1.3.0");
        let from = starting_version(&legacy);
        let migration = migrate(&mut legacy, from, None).unwrap();
        assert_eq!(migration, Migration { from: 2, to: 4 });
        assert!(legacy.get("model_lineup_version").is_none());
        assert!(legacy.get("engine_defaults_version").is_none());
        assert_eq!(legacy["schema_version"], 4);

        // A 1.4.0 file: only the engine step is pending; it waits too.
        let mut v140 = fixture("v1.4.0");
        let before = v140.clone();
        let migration = migrate(&mut v140, 5, None).unwrap();
        assert_eq!(migration, Migration { from: 5, to: 5 });
        v140.remove("schema_version");
        assert_eq!(v140, before);

        // Defaults record every marker, so they reach the current version.
        let Value::Object(mut defaults) = serde_json::to_value(Settings::default()).unwrap() else {
            unreachable!()
        };
        let from = starting_version(&defaults);
        assert_eq!(
            migrate(&mut defaults, from, None).unwrap().to,
            SCHEMA_VERSION
        );
    }

    #[test]
    fn every_fixture_migrates_to_a_strictly_valid_object_idempotently() {
        for (name, input) in fixture_inputs() {
            let mut once = input.clone();
            let migration = migrate_all(&mut once);
            assert_eq!(migration.to, SCHEMA_VERSION, "{name}");
            assert_eq!(migration.ran(), name != "v1.4.2", "{name}");
            let settings: Settings = serde_json::from_value(Value::Object(once.clone()))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(settings.schema_version, SCHEMA_VERSION, "{name}");
            assert!(once.get(LEGACY_ONBOARDING_KEY).is_none(), "{name}");

            let mut twice = once.clone();
            let again = migrate_from_disk(&mut twice, &on_disk(&once));
            assert!(!again.ran(), "{name}");
            assert_eq!(twice, once, "{name}: a second run changed the object");

            // What an older release keeps: everything but schema_version.
            let mut downgraded = once.clone();
            downgraded.remove(SCHEMA_VERSION_KEY);
            assert_eq!(
                starting_version(&on_disk(&downgraded)),
                SCHEMA_VERSION,
                "{name}"
            );
            let disk = on_disk(&downgraded);
            migrate_from_disk(&mut downgraded, &disk);
            assert_eq!(
                downgraded, once,
                "{name}: re-migrated without schema_version"
            );

            // Even replaying every step from scratch changes nothing: the
            // markers guard the one-time value migrations.
            let mut replayed = once.clone();
            migrate(&mut replayed, 0, Some(&machine())).unwrap();
            assert_eq!(replayed, once, "{name}: a replay from version 0 changed it");
        }
    }

    #[test]
    fn markers_keep_user_choices_when_schema_version_is_lost() {
        // Written by this build, then the user chose values each step would
        // change if it ran again; an older release dropped nothing but
        // schema_version is gone (as if the file came from a backup).
        let mut file = object(json!({
            "language": "sv",
            "auto_select_model": false,
            "whisper_model": "kb-whisper-small",
            "engine_prewarm": "on_key_down",
            "engine_idle_unload_minutes": 10,
            "engine_defaults_version": 1,
            "model_lineup_version": 1,
            "profile_glossary_migrated": true,
            "hotkey_profiles": [{"id": "default", "name": "Swedish", "shortcut": "Super+S", "language": "sv"}],
            "profile_models": {"default": "kb-whisper-small"},
            "initial_prompt": "",
            "profile_glossaries": {"default": "only mine"}
        }));
        // The embedded dictionaries are what the store reads in from files.
        assert_eq!(starting_version(&on_disk(&file)), SCHEMA_VERSION);
        let before = file.clone();
        // From the derived version, and replaying every step: nothing moves.
        for from in [SCHEMA_VERSION, 0] {
            let mut migrated = file.clone();
            migrate(&mut migrated, from, Some(&machine())).unwrap();
            migrated.remove(SCHEMA_VERSION_KEY);
            assert_eq!(migrated, before, "from {from}");
        }
        file.remove(SCHEMA_VERSION_KEY);
        let settings: Settings = serde_json::from_value(Value::Object(file)).unwrap();
        assert_eq!(settings.engine_prewarm, EnginePrewarm::OnKeyDown);
        assert_eq!(
            settings.profile_models["default"],
            FileModelPreference::Whisper(WhisperModel::KbWhisperSmall)
        );
        assert_eq!(settings.default_profile().language, Language::Swedish);
    }

    #[test]
    fn legacy_location_copy_resets_auto_paste_for_another_bundle() {
        let mut other = object(json!({"auto_paste": true, "hasCompletedOnboarding": true}));
        from_legacy_location(&mut other, false);
        assert_eq!(
            other,
            object(json!({"auto_paste": false, "has_completed_onboarding": true}))
        );
        let mut same = object(json!({"auto_paste": true}));
        from_legacy_location(&mut same, true);
        assert_eq!(same["auto_paste"], true);
    }
}
