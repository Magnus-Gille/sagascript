use std::collections::BTreeSet;

use super::event::*;
use super::*;

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

trait Named {
    fn name(self) -> &'static str;
}

impl<E: Event> Named for E {
    fn name(self) -> &'static str {
        E::NAME
    }
}

#[test]
fn model_download_progress_is_a_whole_percent() {
    assert_eq!(ModelDownloadProgress::new("m".into(), 0, 0).progress, 0);
    assert_eq!(ModelDownloadProgress::new("m".into(), 1, 3).progress, 33);
    assert_eq!(ModelDownloadProgress::new("m".into(), 3, 3).progress, 100);
}
