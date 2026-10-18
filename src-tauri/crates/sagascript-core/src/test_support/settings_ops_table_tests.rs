use std::collections::BTreeSet;

use super::*;

#[test]
fn the_names_are_unique() {
    let cases = cases();
    let names: BTreeSet<&str> = cases.iter().map(|case| case.name.as_str()).collect();
    assert_eq!(names.len(), cases.len(), "a case name is used twice");
}

/// The surfaces persist the same state for every case but the ones that say
/// otherwise, and then differ in exactly the fields they name.
#[test]
fn the_surfaces_differ_only_where_the_table_says() {
    for case in cases() {
        let cli = expected(&case, Surface::Cli, &case.start);
        let app = expected(&case, Surface::App, &case.start);
        assert_eq!(
            cli.is_ok(),
            app.is_ok(),
            "{}: one surface refuses",
            case.name
        );
        if let (Ok(cli), Ok(app)) = (&cli, &app) {
            let differing = differing_fields(cli, app);
            let named: Vec<String> = case.differs_in.iter().map(|f| f.to_string()).collect();
            assert_eq!(differing, named, "{}", case.name);
        } else {
            assert!(case.differs_in.is_empty(), "{}", case.name);
        }
    }
}

/// The table is worth running: it contains refusals and accepted changes of
/// every kind of operation, so a surface cannot pass by refusing everything.
#[test]
fn the_table_has_accepted_and_refused_cases_for_each_operation() {
    let mut accepted = BTreeSet::new();
    let mut refused = BTreeSet::new();
    for case in cases() {
        let kind = format!("{:?}", case.op);
        let kind = kind.split(['(', ' ', '{']).next().unwrap().to_string();
        if expected(&case, Surface::App, &case.start).is_ok() {
            accepted.insert(kind);
        } else {
            refused.insert(kind);
        }
    }
    for kind in [
        "Language",
        "WhisperModel",
        "AutoSelectModel",
        "PianissimoDictation",
        "AutoPaste",
        "VadEnabled",
        "ShowOverlay",
        "BeamSize",
        "TemperatureFallback",
        "EnginePrewarm",
        "EngineIdleUnloadMinutes",
        "FileTranscriptionModel",
        "HotkeyMode",
        "Hotkey",
        "Profiles",
        "ProfileModel",
    ] {
        assert!(accepted.contains(kind), "{kind}: no accepted case");
    }
    for kind in [
        "Language",
        "WhisperModel",
        "AutoSelectModel",
        "PianissimoDictation",
        "Hotkey",
        "Profiles",
        "ProfileModel",
    ] {
        assert!(refused.contains(kind), "{kind}: no refused case");
    }
}

/// What the legacy fields do on a language change is the drift this table was
/// written for (#373): the persisted state of the CLI and of the app both start
/// over at the recommended model of the language.
#[test]
fn a_language_change_starts_the_legacy_model_fields_over_for_both_surfaces() {
    let case = cases()
        .into_iter()
        .find(|case| case.name == "swedish profile on a chosen model -> Norwegian")
        .expect("the case exists");
    for surface in [Surface::Cli, Surface::App] {
        let persisted = expected(&case, surface, &case.start).unwrap();
        assert_eq!(
            persisted.whisper_model,
            WhisperModel::recommended(Language::Norwegian)
        );
        assert!(persisted.auto_select_model);
        assert_eq!(persisted.default_profile().language, Language::Norwegian);
    }
}
