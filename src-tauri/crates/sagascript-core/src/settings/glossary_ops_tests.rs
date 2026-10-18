use super::*;
use crate::settings::HotkeyProfile;
use crate::test_support::settings_ops_table::english;

/// English default profile, a second profile `auto` that detects its language
/// and a third, `work`, in Swedish.
fn settings() -> Settings {
    let mut settings = english();
    for (id, language, shortcut) in [
        ("auto", Language::Auto, "Super+A"),
        ("work", Language::Swedish, "Super+W"),
    ] {
        settings.hotkey_profiles.push(HotkeyProfile {
            id: id.into(),
            name: id.into(),
            shortcut: shortcut.into(),
            language,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        });
    }
    settings
}

fn aliases(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| item.to_string()).collect()
}

#[test]
fn the_texts_of_a_refusal_are_the_ones_the_surfaces_printed() {
    assert_eq!(
        settings()
            .ensure_alias_profile("ghost")
            .unwrap_err()
            .to_string(),
        "Unknown dictation profile 'ghost'"
    );
    assert_eq!(
        settings()
            .ensure_alias_profile("auto")
            .unwrap_err()
            .to_string(),
        "Auto-detect profiles support dictionary hints, not deterministic aliases"
    );
}

#[test]
fn only_a_profile_with_a_known_language_may_keep_aliases() {
    let settings = settings();
    assert_eq!(settings.ensure_alias_profile("default"), Ok(()));
    assert_eq!(settings.ensure_alias_profile("work"), Ok(()));
    assert_eq!(
        settings.ensure_alias_profile("auto"),
        Err(DictionaryRefusal::AutoDetectAliases)
    );
    assert_eq!(
        settings.ensure_alias_profile("ghost"),
        Err(DictionaryRefusal::UnknownProfile(UnknownProfile(
            "ghost".into()
        )))
    );
}

#[test]
fn a_dictionary_for_a_detecting_profile_may_hold_hints() {
    let settings = settings();
    assert_eq!(
        settings.check_dictionary_source("auto", "Grimnir\nAlpha"),
        Ok(())
    );
    assert_eq!(settings.check_dictionary_source("auto", ""), Ok(()));
    assert_eq!(
        settings.check_dictionary_source("auto", "Alpha\nGrimnir = grimmer"),
        Err(DictionaryRefusal::AutoDetectAliases)
    );
    assert_eq!(
        settings.check_dictionary_source("default", "Grimnir = grimmer"),
        Ok(())
    );
    assert!(matches!(
        settings.check_dictionary_source("ghost", ""),
        Err(DictionaryRefusal::UnknownProfile(_))
    ));
}

#[test]
fn a_saved_dictionary_is_kept_as_written() {
    let mut settings = settings();
    settings
        .set_profile_dictionary("work", "Grimnir=grimmer ,  Alpha".into())
        .unwrap();
    assert_eq!(
        settings.profile_glossaries["work"],
        "Grimnir=grimmer ,  Alpha"
    );
}

#[test]
fn a_refused_save_changes_nothing() {
    let mut settings = settings();
    let before = settings.clone();
    assert!(settings
        .set_profile_dictionary("auto", "Grimnir = grimmer".into())
        .is_err());
    assert!(settings
        .set_profile_dictionary("ghost", "Grimnir".into())
        .is_err());
    assert_eq!(settings.profile_glossaries, before.profile_glossaries);
}

#[test]
fn an_entry_is_added_and_merged_in_the_canonical_form() {
    let mut settings = settings();
    settings
        .add_dictionary_entry("default", "Grimnir", &aliases(&["grimmer"]))
        .unwrap();
    settings
        .add_dictionary_entry("default", "Alpha", &[])
        .unwrap();
    settings
        .add_dictionary_entry("default", "grimnir", &aliases(&["grimnar", "Grimmer"]))
        .unwrap();
    assert_eq!(
        settings.profile_glossaries["default"],
        "Grimnir = grimmer | grimnar\nAlpha"
    );
}

#[test]
fn a_hint_may_be_added_to_a_detecting_profile_and_an_alias_may_not() {
    let mut settings = settings();
    settings
        .add_dictionary_entry("auto", "Grimnir", &[])
        .unwrap();
    assert_eq!(settings.profile_glossaries["auto"], "Grimnir");

    let before = settings.clone();
    assert_eq!(
        settings.add_dictionary_entry("auto", "Grimnir", &aliases(&["grimmer"])),
        Err(DictionaryRefusal::AutoDetectAliases)
    );
    assert_eq!(settings.profile_glossaries, before.profile_glossaries);
}

#[test]
fn an_entry_for_an_unknown_profile_is_refused_with_or_without_aliases() {
    let mut settings = settings();
    let before = settings.profile_glossaries.clone();
    for given in [aliases(&[]), aliases(&["grimmer"])] {
        assert!(matches!(
            settings.add_dictionary_entry("ghost", "Grimnir", &given),
            Err(DictionaryRefusal::UnknownProfile(_))
        ));
    }
    assert_eq!(settings.profile_glossaries, before);
}

#[test]
fn legacy_hints_merge_into_the_default_dictionary_and_never_replace_it() {
    let mut settings = settings();
    settings
        .profile_glossaries
        .insert("default".into(), "Alpha = a\nBeta".into());
    settings
        .merge_default_dictionary("Alpha = other | more, Gamma")
        .unwrap();
    assert_eq!(
        settings.profile_glossaries["default"],
        "Alpha = a | other | more\nBeta\nGamma"
    );
    // The same text again changes nothing.
    let before = settings.clone();
    settings
        .merge_default_dictionary("Alpha = other | more, Gamma")
        .unwrap();
    assert_eq!(settings.profile_glossaries, before.profile_glossaries);
}

#[test]
fn legacy_hints_ignore_the_language_of_the_profile() {
    // The one dictionary rule the CLI's `config set initial_prompt` does not
    // apply, where the app's `set_initial_prompt` does (a kept difference).
    let mut settings = settings();
    settings.hotkey_profiles[0].language = Language::Auto;
    settings
        .merge_default_dictionary("Grimnir = grimmer")
        .unwrap();
    assert_eq!(settings.profile_glossaries["default"], "Grimnir = grimmer");
}

#[test]
fn an_empty_text_does_not_erase_a_dictionary_and_an_empty_one_stays_empty() {
    let mut settings = settings();
    settings.merge_default_dictionary("   ").unwrap();
    assert_eq!(settings.profile_glossaries["default"], "");

    settings
        .profile_glossaries
        .insert("default".into(), "Alpha".into());
    let before = settings.clone();
    for empty in ["", "  \n "] {
        assert_eq!(
            settings.merge_default_dictionary(empty),
            Err(DictionaryRefusal::WouldEraseEntries)
        );
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }
}

#[test]
fn a_profile_is_found_by_its_id_or_is_unknown() {
    let settings = settings();
    assert_eq!(
        settings.dictation_profile("work").unwrap().language,
        Language::Swedish
    );
    assert_eq!(
        settings.dictation_profile("ghost"),
        Err(UnknownProfile("ghost".into()))
    );
    // Settings without profiles still have the one they resolve to.
    assert_eq!(
        Settings::default().dictation_profile("default").unwrap().id,
        "default"
    );
    assert_eq!(
        UnknownProfile("x y".into()).to_string(),
        "Unknown dictation profile 'x y'"
    );
}
