use std::collections::BTreeSet;

use sagascript_core::settings::{HotkeyProfile, Language, WhisperModel};
use sagascript_core::transcription::engine_host::LoadEvent;
use sagascript_core::transcription::pianissimo_backend::{load_state_payload, LoadSource};
use serde_json::{json, Map, Value};

use super::event::*;
use super::*;
use crate::contract;
use crate::hotkey::{HotkeyHealth, OperationalHotkey};

/// Event names that predate the kebab-case rule. Renaming one changes the
/// wire, so it stays, named here (#347).
const KEBAB_CASE_EXCEPTIONS: &[&str] = &["navigate_tab"];

#[test]
fn event_names_are_unique() {
    let mut seen = BTreeSet::new();
    for info in ALL {
        assert!(
            seen.insert(info.name),
            "duplicate event name: {}",
            info.name
        );
    }
    // Every constant is in ALL by construction (the catalogue! macro); this
    // one was once missing from a hand-written list.
    assert!(seen.contains(ENGINE_LOAD_STATE.name()));
}

#[test]
fn event_names_are_kebab_case_except_named_legacy_exceptions() {
    for info in ALL {
        let name = info.name;
        let kebab = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        assert_eq!(
            kebab,
            !KEBAB_CASE_EXCEPTIONS.contains(&name),
            "event '{name}': names are kebab-case; only KEBAB_CASE_EXCEPTIONS may differ"
        );
    }
}

#[test]
fn internal_events_say_why_and_scopes_name_a_field() {
    for info in ALL {
        if let Consumer::Internal(reason) = info.consumer {
            assert!(
                !reason.trim().is_empty(),
                "say why '{}' has no GUI listener",
                info.name
            );
        }
        if let Some(scope) = info.scope {
            assert!(
                !scope.trim().is_empty(),
                "'{}' names its scope field",
                info.name
            );
        }
    }
}

/// Every [`UiState`]; the match fails to compile when a variant is added.
fn ui_states() -> Vec<UiState> {
    let all = vec![
        UiState::Idle,
        UiState::Recording,
        UiState::LoadingModel,
        UiState::Transcribing,
        UiState::Error,
        UiState::SettingsReloaded,
    ];
    for state in &all {
        match state {
            UiState::Idle
            | UiState::Recording
            | UiState::LoadingModel
            | UiState::Transcribing
            | UiState::Error
            | UiState::SettingsReloaded => {}
        }
    }
    all
}

/// Every [`PlainPhase`]; the match fails to compile when a variant is added.
fn plain_phases() -> Vec<PlainPhase> {
    let all = vec![
        PlainPhase::Decoding,
        PlainPhase::Resampling,
        PlainPhase::Loading,
        PlainPhase::Preparing,
        PlainPhase::Encoding,
        PlainPhase::Transcribing,
        PlainPhase::Finalizing,
    ];
    for phase in &all {
        match phase {
            PlainPhase::Decoding
            | PlainPhase::Resampling
            | PlainPhase::Loading
            | PlainPhase::Preparing
            | PlainPhase::Encoding
            | PlainPhase::Transcribing
            | PlainPhase::Finalizing => {}
        }
    }
    all
}

trait Named {
    fn name(self) -> &'static str;
}

impl<E: Event> Named for E {
    fn name(self) -> &'static str {
        E::NAME
    }
}

/// Representative payloads for one event, typed by the catalogue: a sample
/// of the wrong type does not compile.
fn samples<E: Event>(_event: E, payloads: Vec<E::Payload<'_>>) -> (&'static str, Vec<Value>) {
    (
        E::NAME,
        payloads.into_iter().map(contract::to_value).collect(),
    )
}

fn profile(
    id: &str,
    language: Language,
    push: Option<&str>,
    toggle: Option<&str>,
) -> HotkeyProfile {
    HotkeyProfile {
        id: id.into(),
        name: format!("Profile {id}"),
        shortcut: push.or(toggle).unwrap_or("Control+Shift+Space").into(),
        language,
        push_to_talk_shortcut: push.map(Into::into),
        toggle_shortcut: toggle.map(Into::into),
    }
}

/// One sample list per event, produced through the same types and builders
/// as the emit sites.
fn all_samples() -> Vec<(&'static str, Vec<Value>)> {
    let health = HotkeyHealth::new("Control+Shift+Space");
    let registered = health
        .record(
            "Control+Shift+Space",
            None,
            OperationalHotkey::registered_many(&["Control+Shift+Space".into(), "F13".into()]),
        )
        .status;
    let failed = health
        .record(
            "F13",
            Some("failed to register hotkey 'F13': already in use".into()),
            OperationalHotkey::Inactive,
        )
        .status;
    let run_id = "run-1";
    let progress = |phase, percent| PlainTranscriptionProgress {
        run_id,
        phase,
        percent,
    };
    let mut plain = vec![
        progress(PlainPhase::Decoding, Some(0)),
        progress(PlainPhase::Decoding, Some(100)),
        progress(PlainPhase::Resampling, Some(60)),
        progress(PlainPhase::Loading, None),
        progress(PlainPhase::Preparing, None),
        progress(PlainPhase::Encoding, None),
        progress(PlainPhase::Transcribing, Some(40)),
        progress(PlainPhase::Finalizing, None),
    ];
    let covered: BTreeSet<_> = plain
        .iter()
        .map(|event| format!("{:?}", event.phase))
        .collect();
    let all_phases: BTreeSet<_> = plain_phases()
        .iter()
        .map(|phase| format!("{phase:?}"))
        .collect();
    assert_eq!(
        covered, all_phases,
        "the progress samples cover every phase"
    );
    plain.push(PlainTranscriptionProgress {
        run_id: "run-2",
        phase: PlainPhase::Transcribing,
        percent: Some(90),
    });
    let swedish = profile("swedish", Language::Swedish, Some("F13"), Some("F14"));
    let english = profile("english", Language::English, None, None);
    vec![
        samples(STATE_CHANGED, ui_states()),
        samples(TRANSCRIPTION_RESULT, vec!["Hej, det här är en diktering."]),
        samples(ERROR, vec!["Recording failed: no audio input device"]),
        samples(
            MODEL_DOWNLOAD_PROGRESS,
            vec![
                ModelDownloadProgress::new(format!("{:?}", WhisperModel::KbWhisperLarge), 0, 0),
                ModelDownloadProgress::new("pianissimo-sv".into(), 512, 2048),
            ],
        ),
        samples(MODEL_READY, vec![()]),
        samples(TRANSCRIPTION_PROGRESS, vec![1, 42]),
        samples(PLAIN_TRANSCRIPTION_PROGRESS, plain),
        samples(
            TRANSCRIPTION_PHASE,
            vec![
                PlainPhase::Loading,
                PlainPhase::Preparing,
                PlainPhase::Encoding,
            ],
        ),
        samples(HOTKEY_REGISTRATION_CHANGED, vec![&registered, &failed]),
        samples(ACTIVE_HOTKEY_PROFILE_CHANGED, vec![&swedish, &english]),
        samples(
            ENGINE_LOAD_STATE,
            vec![
                load_state_payload(None),
                load_state_payload(Some(&(LoadEvent::Started, LoadSource::Warm))),
                load_state_payload(Some(&(
                    LoadEvent::Ready { load_ms: 1250 },
                    LoadSource::Request,
                ))),
                load_state_payload(Some(&(
                    LoadEvent::Failed("host exited".into()),
                    LoadSource::Request,
                ))),
                load_state_payload(Some(&(LoadEvent::Cancelled, LoadSource::Warm))),
            ],
        ),
        samples(
            UPDATE_PREPARING,
            vec!["8f14e45f-ceea-4e67-a0d9-9c0f8c5d1b2a"],
        ),
        samples(
            UPDATE_ABORTED,
            vec!["Settings closed before recovery drafts were saved."],
        ),
        samples(NAVIGATE_TAB, vec!["dictate", "transcribe"]),
    ]
}

/// The catalogue and its samples, as the shared fixture
/// contracts/transport/events.json: `{ name: { consumer, reason?, scope?,
/// samples } }`.
fn catalogue_fixture() -> Value {
    let mut samples: Vec<_> = all_samples();
    let mut events = Map::new();
    for info in ALL {
        let index = samples
            .iter()
            .position(|(name, _)| *name == info.name)
            .unwrap_or_else(|| panic!("event '{}' has no samples in all_samples()", info.name));
        let (_, payloads) = samples.remove(index);
        assert!(!payloads.is_empty(), "event '{}' has no samples", info.name);
        let mut entry = Map::new();
        match info.consumer {
            Consumer::Gui => {
                entry.insert("consumer".into(), json!("gui"));
            }
            Consumer::Internal(reason) => {
                entry.insert("consumer".into(), json!("internal"));
                entry.insert("reason".into(), json!(reason));
            }
        }
        if let Some(scope) = info.scope {
            for payload in &payloads {
                assert!(
                    payload.get(scope).is_some_and(Value::is_string),
                    "every '{}' sample carries its scope field '{scope}'",
                    info.name
                );
            }
            entry.insert("scope".into(), json!(scope));
        }
        entry.insert("samples".into(), Value::Array(payloads));
        events.insert(info.name.into(), Value::Object(entry));
    }
    let extra: Vec<_> = samples.iter().map(|(name, _)| *name).collect();
    assert!(
        extra.is_empty(),
        "samples for events not in the catalogue: {extra:?}"
    );
    Value::Object(events)
}

#[test]
fn transport_contract_events() {
    contract::assert_fixture("events.json", &catalogue_fixture());
}

#[test]
fn transport_contract_event_payloads_read_back() {
    if contract::blessing() {
        return; // the fixture is being rewritten; the next run reads it
    }
    let fixture = contract::read_fixture("events.json");
    let samples = |name: &str| -> Vec<Value> {
        fixture[name]["samples"]
            .as_array()
            .unwrap_or_else(|| panic!("events.json has samples for {name}"))
            .clone()
    };
    for value in samples(STATE_CHANGED.name()) {
        let state: UiState = serde_json::from_value(value.clone()).expect("a UiState");
        assert_eq!(contract::to_value(state), value);
    }
    for value in samples(TRANSCRIPTION_PHASE.name()) {
        let phase: PlainPhase = serde_json::from_value(value.clone()).expect("a PlainPhase");
        assert_eq!(contract::to_value(phase), value);
    }
    for value in samples(PLAIN_TRANSCRIPTION_PROGRESS.name()) {
        let phase: PlainPhase =
            serde_json::from_value(value["phase"].clone()).expect("a PlainPhase");
        assert_eq!(contract::to_value(phase), value["phase"]);
    }
    for value in samples(MODEL_DOWNLOAD_PROGRESS.name()) {
        let progress: ModelDownloadProgress =
            serde_json::from_value(value.clone()).expect("a ModelDownloadProgress");
        assert_eq!(contract::to_value(progress), value);
    }
    for value in samples(ACTIVE_HOTKEY_PROFILE_CHANGED.name()) {
        let profile: HotkeyProfile = serde_json::from_value(value.clone()).expect("a profile");
        assert_eq!(contract::to_value(profile), value);
    }
}

#[test]
fn model_download_progress_is_a_whole_percent() {
    assert_eq!(ModelDownloadProgress::new("m".into(), 0, 0).progress, 0);
    assert_eq!(ModelDownloadProgress::new("m".into(), 1, 3).progress, 33);
    assert_eq!(ModelDownloadProgress::new("m".into(), 3, 3).progress, 100);
}
