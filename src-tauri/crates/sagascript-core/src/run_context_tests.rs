use super::*;

fn profile(id: &str, language: Language) -> HotkeyProfile {
    HotkeyProfile {
        id: id.to_string(),
        name: id.to_string(),
        shortcut: format!("Super+{id}"),
        language,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

/// Profile dictionaries are in force (the migrated state every current
/// settings file reaches on load); the legacy global hint is left behind.
fn migrated() -> Settings {
    let mut settings = Settings {
        language: Language::English,
        initial_prompt: "LegacyOnly".to_string(),
        hotkey_profiles: vec![
            profile("default", Language::English),
            profile("swedish", Language::Swedish),
            profile("auto", Language::Auto),
        ],
        profile_glossary_migrated: true,
        ..Default::default()
    };
    for (id, source) in [
        ("default", "Sagascript"),
        ("swedish", "merge = merch\nGrimnir"),
        ("auto", "Akronym = akr"),
    ] {
        settings
            .profile_glossaries
            .insert(id.to_string(), source.to_string());
    }
    settings
}

/// A settings file from before profile dictionaries: the saved global hint is
/// the base of every run, and the Swedish profile has a dictionary of its own.
fn legacy() -> Settings {
    let mut settings = Settings {
        initial_prompt: "merge = merch\nOpenRouter".to_string(),
        hotkey_profiles: vec![
            profile("swedish", Language::Swedish),
            profile("english", Language::English),
        ],
        profile_glossary_migrated: false,
        ..Default::default()
    };
    settings
        .profile_glossaries
        .insert("swedish".to_string(), "fika = coffee".to_string());
    settings
}

type Entries = &'static [(&'static str, &'static [&'static str])];

fn entries_of(glossary: &Glossary) -> Vec<(String, Vec<String>)> {
    glossary
        .entries()
        .iter()
        .map(|entry| (entry.canonical.clone(), entry.aliases.clone()))
        .collect()
}

fn expected_entries(entries: Entries) -> Vec<(String, Vec<String>)> {
    entries
        .iter()
        .map(|(canonical, aliases)| {
            (
                canonical.to_string(),
                aliases.iter().map(|alias| alias.to_string()).collect(),
            )
        })
        .collect()
}

struct Case {
    name: &'static str,
    profile: Option<&'static str>,
    language: Option<Language>,
    prompt: Option<&'static str>,
    /// `Ok((profile id, language, language overridden, dictionary))` or the
    /// complete error message.
    expect: Result<(&'static str, Language, bool, Entries), &'static str>,
}

fn run_table(settings: &Settings, cases: &[Case]) {
    for case in cases {
        let overrides = RunOverrides {
            profile: case.profile,
            language: case.language,
        };
        let selection = RunSelection::resolve(settings, overrides);
        let resolved = selection.map(|selection| {
            let overridden = selection.language_overridden();
            (overridden, selection.into_context(settings, case.prompt))
        });
        match (&case.expect, resolved) {
            (Ok((id, language, overridden, entries)), Ok((was_overridden, context))) => {
                assert_eq!(context.profile.id, *id, "{}: profile", case.name);
                assert_eq!(context.language, *language, "{}: language", case.name);
                assert_eq!(
                    was_overridden, *overridden,
                    "{}: language_overridden",
                    case.name
                );
                assert_eq!(
                    entries_of(&context.glossary),
                    expected_entries(entries),
                    "{}: dictionary",
                    case.name
                );
            }
            (Err(message), Err(error)) => {
                assert!(
                    matches!(error, DictationError::SettingsError(_)),
                    "{}: {error:?}",
                    case.name
                );
                assert_eq!(
                    error.to_string(),
                    format!("Settings error: {message}"),
                    "{}: error text",
                    case.name
                );
            }
            (expected, got) => panic!(
                "{}: expected {expected:?}, got {:?}",
                case.name,
                got.map(|(overridden, context)| (
                    overridden,
                    context.profile.id,
                    context.language,
                    entries_of(&context.glossary)
                ))
            ),
        }
    }
}

const NO_ENTRIES: Entries = &[];
const SAGASCRIPT: Entries = &[("Sagascript", &[])];
const SWEDISH_DICTIONARY: Entries = &[("merge", &["merch"]), ("Grimnir", &[])];

#[test]
fn profile_and_language_table() {
    use Language::{Auto, English, Swedish};
    run_table(
        &migrated(),
        &[
            Case {
                name: "no flags use the default profile, its language and its dictionary",
                profile: None,
                language: None,
                prompt: None,
                expect: Ok(("default", English, false, SAGASCRIPT)),
            },
            Case {
                name: "the default profile named explicitly is the same run",
                profile: Some("default"),
                language: None,
                prompt: None,
                expect: Ok(("default", English, false, SAGASCRIPT)),
            },
            Case {
                name: "an explicit profile brings its own language and dictionary",
                profile: Some("swedish"),
                language: None,
                prompt: None,
                expect: Ok(("swedish", Swedish, false, SWEDISH_DICTIONARY)),
            },
            Case {
                name: "an unknown profile is an error",
                profile: Some("nope"),
                language: None,
                prompt: None,
                expect: Err("Unknown dictation profile 'nope'"),
            },
            Case {
                name: "an empty profile id is unknown, not the default",
                profile: Some(""),
                language: None,
                prompt: None,
                expect: Err("Unknown dictation profile ''"),
            },
            Case {
                name: "profile ids are matched exactly",
                profile: Some("Swedish"),
                language: None,
                prompt: None,
                expect: Err("Unknown dictation profile 'Swedish'"),
            },
            Case {
                name: "the unknown profile is reported before any prompt is considered",
                profile: Some("nope"),
                language: Some(Swedish),
                prompt: Some("Acme"),
                expect: Err("Unknown dictation profile 'nope'"),
            },
            Case {
                name: "the same language as the profile keeps the dictionary",
                profile: None,
                language: Some(English),
                prompt: None,
                expect: Ok(("default", English, false, SAGASCRIPT)),
            },
            Case {
                name: "a changed language drops the profile dictionary",
                profile: None,
                language: Some(Swedish),
                prompt: None,
                expect: Ok(("default", Swedish, true, NO_ENTRIES)),
            },
            Case {
                name: "auto is a changed language too",
                profile: None,
                language: Some(Auto),
                prompt: None,
                expect: Ok(("default", Auto, true, NO_ENTRIES)),
            },
            Case {
                name: "an explicit profile with its own language keeps its dictionary",
                profile: Some("swedish"),
                language: Some(Swedish),
                prompt: None,
                expect: Ok(("swedish", Swedish, false, SWEDISH_DICTIONARY)),
            },
            Case {
                name: "an explicit profile with another language drops its dictionary",
                profile: Some("swedish"),
                language: Some(English),
                prompt: None,
                expect: Ok(("swedish", English, true, NO_ENTRIES)),
            },
            Case {
                name: "an auto-detect profile keeps its dictionary as hints only",
                profile: Some("auto"),
                language: None,
                prompt: None,
                expect: Ok(("auto", Auto, false, &[("Akronym", &[])])),
            },
        ],
    );
}

#[test]
fn one_run_prompt_table() {
    use Language::{English, Swedish};
    run_table(
        &migrated(),
        &[
            Case {
                name: "a prompt adds hints in front of the profile dictionary",
                profile: None,
                language: None,
                prompt: Some("Acme, Foo"),
                expect: Ok((
                    "default",
                    English,
                    false,
                    &[("Acme", &[]), ("Foo", &[]), ("Sagascript", &[])],
                )),
            },
            Case {
                name: "a prompt is hint-only: its aliases never become replacements",
                profile: Some("swedish"),
                language: None,
                prompt: Some("Grimmer = grimmer"),
                expect: Ok((
                    "swedish",
                    Swedish,
                    false,
                    &[
                        ("Grimmer", &[]),
                        ("merge", &["merch"]),
                        ("Grimnir", &[]),
                    ],
                )),
            },
            Case {
                name: "a blank prompt is no prompt",
                profile: None,
                language: None,
                prompt: Some("  \n\t"),
                expect: Ok(("default", English, false, SAGASCRIPT)),
            },
            Case {
                name: "a changed language keeps only the prompt",
                profile: None,
                language: Some(Swedish),
                prompt: Some("Acme, Foo"),
                expect: Ok(("default", Swedish, true, &[("Acme", &[]), ("Foo", &[])])),
            },
            Case {
                name: "a changed language keeps the prompt's aliases (the caller's own words)",
                profile: None,
                language: Some(Swedish),
                prompt: Some("Acme = akme"),
                expect: Ok(("default", Swedish, true, &[("Acme", &["akme"])])),
            },
            Case {
                name: "a changed language with a blank prompt has no dictionary",
                profile: None,
                language: Some(Swedish),
                prompt: Some("  "),
                expect: Ok(("default", Swedish, true, NO_ENTRIES)),
            },
        ],
    );
}

#[test]
fn legacy_settings_table() {
    use Language::{English, Swedish};
    // Before profile dictionaries, the saved global hint is the base of every
    // run and only an explicit-language profile adds aliases.
    run_table(
        &legacy(),
        &[
            Case {
                name: "the first profile is the default when none is called default",
                profile: None,
                language: None,
                prompt: None,
                expect: Ok((
                    "swedish",
                    Swedish,
                    false,
                    &[("merge", &[]), ("OpenRouter", &[]), ("fika", &["coffee"])],
                )),
            },
            Case {
                name: "a profile without a dictionary of its own gets the global hints",
                profile: Some("english"),
                language: None,
                prompt: None,
                expect: Ok(("english", English, false, &[("merge", &[]), ("OpenRouter", &[])])),
            },
            Case {
                name: "a prompt replaces the saved global hint",
                profile: Some("english"),
                language: None,
                prompt: Some("Acme"),
                expect: Ok(("english", English, false, &[("Acme", &[])])),
            },
            Case {
                name: "a changed language leaves the saved global hint behind",
                profile: Some("english"),
                language: Some(Swedish),
                prompt: None,
                expect: Ok(("english", Swedish, true, NO_ENTRIES)),
            },
        ],
    );
}

#[test]
fn settings_without_profiles_resolve_the_legacy_default() {
    let settings = Settings {
        language: Language::Swedish,
        hotkey_profiles: Vec::new(),
        ..Default::default()
    };
    let selection = RunSelection::resolve(&settings, RunOverrides::default()).unwrap();
    assert_eq!(selection.profile.id, "default");
    assert_eq!(selection.language, Language::Swedish);
    assert!(!selection.language_overridden());
}

#[test]
fn the_default_profile_is_settings_default_profile() {
    for settings in [migrated(), legacy(), Settings::default()] {
        let selection = RunSelection::resolve(&settings, RunOverrides::default()).unwrap();
        assert_eq!(selection.profile, settings.default_profile());
        assert_eq!(selection.language, settings.default_profile().language);
    }
}

#[test]
fn resolution_leaves_the_settings_untouched() {
    let settings = migrated();
    let before = serde_json::to_string(&settings).unwrap();
    let selection = RunSelection::resolve(
        &settings,
        RunOverrides {
            profile: Some("swedish"),
            language: Some(Language::English),
        },
    )
    .unwrap();
    selection.into_context(&settings, Some("Acme"));
    assert_eq!(serde_json::to_string(&settings).unwrap(), before);
}
