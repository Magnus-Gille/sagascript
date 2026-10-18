//! Scenarios for `persist_and_sync` beyond the table (#370): what an unrelated
//! setter must leave alone, and what two setters must not do at once. They use
//! the harness of the parent module (`isolated`, `app_with`, the rows).

use std::collections::BTreeMap;

use sagascript_core::settings::HotkeyProfile;

use super::*;

/// A profile of the scenarios' files. Norwegian, the language the table's models
/// fit, unless a scenario says otherwise.
fn profile(id: &str, shortcut: &str, language: Language) -> HotkeyProfile {
    HotkeyProfile {
        id: id.into(),
        name: id.into(),
        shortcut: shortcut.into(),
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
        language,
    }
}

/// The controller of a scenario: a Norwegian default profile on its default
/// shortcut, which every row can run against.
fn norwegian_settings() -> Settings {
    Settings {
        language: Language::Norwegian,
        whisper_model: WhisperModel::NbWhisperBase,
        ..Settings::default()
    }
}

/// A valid shortcut that the OS did not register for the controller (another
/// application holds it, or the hot reload failed). Not a bare function key:
/// those are valid on macOS only, and a setter that validates the profiles
/// would refuse the file on the other platforms.
const UNREGISTERED: &str = "Control+Alt+K";

/// The shortcut-bearing state of a copy: what key presses are resolved against
/// and what the OS has registered.
fn shortcut_state(settings: &Settings) -> (String, HotkeyMode, Vec<(String, String)>) {
    let profiles = settings
        .resolved_hotkey_profiles()
        .into_iter()
        .flat_map(|profile| {
            [
                (profile.id.clone(), profile.shortcut),
                (
                    format!("{}.push_to_talk", profile.id),
                    profile.push_to_talk_shortcut.unwrap_or_default(),
                ),
                (
                    format!("{}.toggle", profile.id),
                    profile.toggle_shortcut.unwrap_or_default(),
                ),
            ]
        })
        .collect();
    (settings.hotkey.clone(), settings.hotkey_mode, profiles)
}

/// The file holds shortcuts the OS could not register (the CLI wrote them, or a
/// hot reload failed), and the controller deliberately stays on the registered
/// ones: the watcher keeps it there, and key presses are resolved against it. A
/// setter of something else must not move it, or the watcher sees controller
/// and file agree, does nothing, and the registered shortcut stops working.
///
/// Every row but the hotkey change: the ones that mirror a single field and the
/// ones that used to take the file's whole copy (a language, a profile's model,
/// a dictionary).
#[test]
fn an_unrelated_setter_keeps_the_controllers_registered_shortcuts() {
    let Some(_) = isolated(
        module_path!(),
        "an_unrelated_setter_keeps_the_controllers_registered_shortcuts",
        || {
            let runtime = runtime();
            let mut problems = Vec::new();
            let mut checked = Vec::new();
            for row in ROWS
                .iter()
                .filter(|row| !matches!(row.mirror, Mirror::Whole))
            {
                let name = row.commands.join("+");
                let app = app_with(norwegian_settings());
                let registered = lock_controller(&controller(&app)).settings().clone();
                store::update(|s| {
                    s.hotkey = UNREGISTERED.into();
                    s.hotkey_profiles = vec![profile("default", UNREGISTERED, Language::Norwegian)];
                })
                .expect("the fake file is writable");
                (row.arrange)();
                assert_ne!(
                    store::load().resolved_shortcuts(),
                    registered.resolved_shortcuts(),
                    "the file must hold other shortcuts than the controller"
                );

                (row.run)(&app, &runtime)
                    .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

                let after = lock_controller(&controller(&app)).settings().clone();
                if after.resolved_shortcuts() != registered.resolved_shortcuts() {
                    problems.push(format!(
                        "{name}: the controller adopted shortcuts the OS never registered"
                    ));
                }
                if shortcut_state(&after) != shortcut_state(&registered) {
                    problems.push(format!(
                        "{name}: the controller's shortcut configuration moved"
                    ));
                }
                if store::load().hotkey != UNREGISTERED {
                    problems.push(format!("{name}: the file lost what was written"));
                }
                checked.push(name);
            }
            // Every row is checked before the test fails, so a failure lists
            // each setter that moved the shortcuts.
            assert!(problems.is_empty(), "{}", problems.join("\n"));
            checked.join("\n")
        },
    ) else {
        return;
    };
}

/// A language is a profile's, and the profiles are where the shortcuts are. The
/// controller takes the new language of its default profile from the file, and
/// the profile stays on the shortcut the OS registered.
#[test]
fn a_language_change_follows_the_file_but_keeps_the_controllers_shortcuts() {
    let Some(_) = isolated(
        module_path!(),
        "a_language_change_follows_the_file_but_keeps_the_controllers_shortcuts",
        || {
            let registered = Settings::default();
            let app = app_with(registered.clone());
            store::update(|s| {
                s.hotkey = UNREGISTERED.into();
                s.hotkey_profiles = vec![profile("default", UNREGISTERED, Language::English)];
            })
            .expect("the fake file is writable");

            persist_language(&controller(&app), Language::Norwegian).expect("the setter works");

            let after = lock_controller(&controller(&app)).settings().clone();
            let default = after.default_profile();
            assert_eq!(default.language, Language::Norwegian);
            assert_eq!(after.language, Language::Norwegian);
            assert_eq!(default.shortcut, registered.hotkey);
            assert_eq!(after.hotkey, registered.hotkey);
            assert_eq!(after.resolved_shortcuts(), registered.resolved_shortcuts());
            let on_disk = store::load_from(&fake_file());
            assert_eq!(on_disk.default_profile().shortcut, UNREGISTERED);
            assert_eq!(on_disk.default_profile().language, Language::Norwegian);
            "followed".into()
        },
    ) else {
        return;
    };
}

/// After a hot reload that could not register the file's profiles, the watcher
/// keeps the controller's profile list and its models together, as a pair that
/// differs from the file. A setter hands the controller the models of the
/// profiles the controller has, never those of profiles only the file has, and
/// never drops the model of a profile only the controller has: its shortcut is
/// still registered, and a press on it must run the model the user chose.
#[test]
fn a_setter_keeps_the_models_of_the_controllers_own_profiles() {
    let Some(_) = isolated(
        module_path!(),
        "a_setter_keeps_the_models_of_the_controllers_own_profiles",
        || {
            let runtime = runtime();
            let mut problems = Vec::new();
            let mut checked = Vec::new();
            for row in ROWS
                .iter()
                .filter(|row| !matches!(row.mirror, Mirror::Whole))
            {
                let name = row.commands.join("+");
                // The controller runs a Norwegian default and a Swedish profile
                // on KB-Whisper Large; the file dropped that profile and has
                // one the controller never registered.
                let memory = Settings {
                    hotkey_profiles: vec![
                        profile("default", "Control+Shift+Space", Language::Norwegian),
                        profile("sv", "Control+Alt+S", Language::Swedish),
                    ],
                    profile_models: BTreeMap::from([
                        ("default".to_string(), FileModelPreference::Auto),
                        (
                            "sv".to_string(),
                            FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
                        ),
                    ]),
                    ..norwegian_settings()
                };
                let app = app_with(memory);
                store::update(|s| {
                    s.whisper_model = WhisperModel::NbWhisperBase;
                    s.hotkey_profiles = vec![
                        profile("default", "Control+Shift+Space", Language::Norwegian),
                        profile("extra", "Control+Alt+E", Language::Norwegian),
                    ];
                    s.profile_models = BTreeMap::from([
                        ("default".to_string(), FileModelPreference::Auto),
                        (
                            "extra".to_string(),
                            FileModelPreference::Whisper(WhisperModel::NbWhisperBase),
                        ),
                    ]);
                })
                .expect("the fake file is writable");
                (row.arrange)();

                (row.run)(&app, &runtime)
                    .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

                let after = lock_controller(&controller(&app)).settings().clone();
                if after.profile_models.get("sv")
                    != Some(&FileModelPreference::Whisper(WhisperModel::KbWhisperLarge))
                {
                    problems.push(format!(
                        "{name}: the controller lost the model of a profile it still has"
                    ));
                }
                if after.profile_models.contains_key("extra") {
                    problems.push(format!(
                        "{name}: the controller took the model of a profile only the file has"
                    ));
                }
                let ids: Vec<_> = after
                    .hotkey_profiles
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect();
                if ids != ["default", "sv"] {
                    problems.push(format!(
                        "{name}: the controller's profiles changed: {ids:?}"
                    ));
                }
                checked.push(name);
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
            checked.join("\n")
        },
    ) else {
        return;
    };
}

/// A profile called `default` is not mandatory: the default is the first one
/// when none is. File and controller can then have different defaults (the same
/// profiles in another order, after a reload that failed), and the user's
/// choice in the window applies to the controller's, which is the one the
/// window shows.
#[test]
fn a_language_change_lands_on_the_controllers_default_profile() {
    let Some(_) = isolated(
        module_path!(),
        "a_language_change_lands_on_the_controllers_default_profile",
        || {
            let first = profile("a", "Control+Alt+A", Language::English);
            let second = profile("b", "Control+Alt+B", Language::Swedish);
            let memory = Settings {
                hotkey: first.shortcut.clone(),
                language: first.language,
                hotkey_profiles: vec![first.clone(), second.clone()],
                profile_models: BTreeMap::from([
                    ("a".to_string(), FileModelPreference::Auto),
                    ("b".to_string(), FileModelPreference::Auto),
                ]),
                ..Settings::default()
            };
            let app = app_with(memory.clone());
            store::update(|s| {
                s.hotkey = second.shortcut.clone();
                s.language = second.language;
                s.hotkey_profiles = vec![second.clone(), first.clone()];
                s.profile_models = memory.profile_models.clone();
            })
            .expect("the fake file is writable");
            assert_eq!(store::load().default_profile().id, "b");

            persist_language(&controller(&app), Language::Norwegian).expect("the setter works");

            let after = lock_controller(&controller(&app)).settings().clone();
            assert_eq!(after.default_profile().id, "a");
            assert_eq!(after.default_profile().language, Language::Norwegian);
            assert_eq!(after.language, Language::Norwegian);
            let other = after.hotkey_profiles.iter().find(|p| p.id == "b");
            assert_eq!(other.map(|p| p.language), Some(Language::Swedish));
            assert_eq!(after.resolved_shortcuts(), memory.resolved_shortcuts());
            assert_eq!(
                store::load_from(&fake_file()).default_profile().language,
                Language::Norwegian,
                "the file's own default is the one the file changed"
            );
            "landed".into()
        },
    ) else {
        return;
    };
}

/// A file that selects Pianissimo, as copied from a machine that runs it: a
/// Swedish profile on it next to the Norwegian default, file transcription on
/// it, and the legacy flag on.
fn file_selecting_pianissimo() {
    store::update(|s| {
        s.language = Language::Norwegian;
        s.whisper_model = WhisperModel::NbWhisperBase;
        s.engine_prewarm = EnginePrewarm::OnAppStart;
        s.pianissimo_dictation = true;
        s.file_transcription_model = FileModelPreference::PianissimoOriginal;
        s.hotkey_profiles = vec![
            profile("default", "Control+Shift+Space", Language::Norwegian),
            profile("sv", "Control+Alt+S", Language::Swedish),
        ];
        s.profile_models = BTreeMap::from([
            ("default".to_string(), FileModelPreference::Auto),
            ("sv".to_string(), FileModelPreference::PianissimoOriginal),
        ]);
    })
    .expect("the fake file is writable");
}

/// The controller's copy as the startup load of a host that cannot run
/// Pianissimo leaves it: demoted in memory, the file untouched.
fn demoted_copy_of_the_file() -> Settings {
    let mut loaded = store::load_from(&fake_file());
    assert!(loaded.demote_unsupported_pianissimo(false));
    loaded
}

/// Whether a copy selects the Pianissimo engine anywhere a host that cannot run
/// it would have demoted.
fn selects_pianissimo(settings: &Settings) -> bool {
    settings.pianissimo_dictation
        || settings.file_transcription_model == FileModelPreference::PianissimoOriginal
        || settings
            .profile_models
            .values()
            .any(|model| *model == FileModelPreference::PianissimoOriginal)
}

/// A host that cannot run Pianissimo demotes a file's selection of it in
/// memory only (`store::load`): the file keeps the user's choice, the
/// controller runs on Auto. A setter of something else must not bring the
/// selection back into the controller.
#[test]
fn an_unrelated_setter_keeps_the_demotion_of_an_unsupported_engine() {
    let Some(_) = isolated(
        module_path!(),
        "an_unrelated_setter_keeps_the_demotion_of_an_unsupported_engine",
        || {
            store::update(|s| {
                s.file_transcription_model = FileModelPreference::PianissimoOriginal;
                s.profile_models
                    .insert("default".into(), FileModelPreference::PianissimoOriginal);
            })
            .expect("the fake file is writable");
            // The startup load of an unsupported host, whatever this host is.
            let mut loaded = store::load_from(&fake_file());
            assert!(loaded.demote_unsupported_pianissimo(false));
            let app = app_with(loaded);

            runtime()
                .block_on(set_beam_size(controller(&app), 7))
                .expect("the setter works");

            let shown = runtime()
                .block_on(get_settings(controller(&app)))
                .expect("get_settings works");
            assert_eq!(shown.beam_size, 7);
            assert_eq!(shown.file_transcription_model, FileModelPreference::Auto);
            assert_eq!(
                shown.profile_models.get("default"),
                Some(&FileModelPreference::Auto)
            );
            let on_disk = store::load_from(&fake_file());
            assert_eq!(
                on_disk.file_transcription_model,
                FileModelPreference::PianissimoOriginal,
                "the file keeps the user's choice"
            );
            "demoted".into()
        },
    ) else {
        return;
    };
}

/// A host that cannot run Pianissimo demotes a file's selection of it in
/// memory only (`store::load`): the file keeps the user's choice, the
/// controller runs on Auto. No setter may bring the selection back into the
/// controller, whether it mirrors a field or takes the whole copy. The file
/// keeps the choice through every setter.
#[test]
fn no_setter_brings_the_demoted_engine_back_into_the_controller() {
    let Some(_) = isolated(
        module_path!(),
        "no_setter_brings_the_demoted_engine_back_into_the_controller",
        || {
            let _host = on_a_host_without_pianissimo();
            let runtime = runtime();
            let mut problems = Vec::new();
            let mut checked = Vec::new();
            for row in ROWS {
                let name = row.commands.join("+");
                file_selecting_pianissimo();
                (row.arrange)();
                let app = app_with(demoted_copy_of_the_file());
                assert!(!selects_pianissimo(
                    lock_controller(&controller(&app)).settings()
                ));

                (row.run)(&app, &runtime)
                    .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

                let after = lock_controller(&controller(&app)).settings().clone();
                if selects_pianissimo(&after) {
                    problems.push(format!(
                        "{name}: the controller holds a selection of an engine this host cannot run"
                    ));
                }
                if after.profile_models.get("sv") != Some(&FileModelPreference::Auto) {
                    problems.push(format!(
                        "{name}: the demoted profile is back on Pianissimo or gone"
                    ));
                }
                if store::load_from(&fake_file()).profile_models.get("sv")
                    != Some(&FileModelPreference::PianissimoOriginal)
                {
                    problems.push(format!("{name}: the file lost the user's choice"));
                }
                checked.push(name);
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
            checked.join("\n")
        },
    ) else {
        return;
    };
}

/// A profile model change on a host that cannot run Pianissimo must not start
/// the engine because a selection of it is in the file. The decision compares
/// the controller's copy before and after the change, and both are demoted.
#[test]
fn a_profile_model_change_does_not_warm_an_engine_this_host_cannot_run() {
    let Some(_) = isolated(
        module_path!(),
        "a_profile_model_change_does_not_warm_an_engine_this_host_cannot_run",
        || {
            let _host = on_a_host_without_pianissimo();
            file_selecting_pianissimo();
            let app = app_with(demoted_copy_of_the_file());

            let changed = persist_profile_model(
                &controller(&app),
                |settings| {
                    settings.set_profile_model(
                        "default",
                        FileModelPreference::Whisper(WhisperModel::NbWhisperMedium),
                    )
                },
                mirror_profile_models,
            )
            .expect("the setter works");

            assert!(
                !changed.warm,
                "the change would load an engine this host cannot run"
            );
            assert!(!selects_pianissimo(&changed.settings));
            assert!(!selects_pianissimo(
                lock_controller(&controller(&app)).settings()
            ));
            "not warmed".into()
        },
    ) else {
        return;
    };
}

/// Two writers must not interleave between the file write and the controller
/// sync. Without that, the second's write lands in the first's window, and the
/// first's controller update (computed from the file it wrote, before the
/// second) then drops the second's change from the controller.
///
/// The first setter is held between its file write and its controller sync by
/// holding the controller's lock from the test; the second must not reach the
/// file until the first is done.
#[test]
fn two_setters_do_not_interleave_between_the_file_and_the_controller() {
    let Some(_) = isolated(
        module_path!(),
        "two_setters_do_not_interleave_between_the_file_and_the_controller",
        || {
            let file = fake_file();
            store::update(|_| {}).expect("the fake file is writable");
            // The controller starts as the app loads it, so file and controller
            // agree before the setters run.
            let controller: SharedController = Mutex::new(AppController::new(store::load()));
            let intruded = std::thread::scope(|scope| {
                let held = lock_controller(&controller);
                let first = scope.spawn(|| persist_language(&controller, Language::Norwegian));
                wait_until(Duration::from_secs(30), || {
                    store::load_from(&file).language == Language::Norwegian
                })
                .then_some(())
                .expect("the first setter wrote the file");
                let second = scope
                    .spawn(|| persist_and_sync(&controller, |settings| settings.beam_size = 7));
                // A short bounded wait for something that must not happen; the
                // test fails at once when it does.
                let intruded = wait_until(Duration::from_millis(300), || {
                    store::load_from(&file).beam_size == 7
                });
                drop(held);
                first
                    .join()
                    .expect("no panic")
                    .expect("the first setter works");
                second
                    .join()
                    .expect("no panic")
                    .expect("the second setter works");
                intruded
            });
            assert!(
                !intruded,
                "the second setter wrote the file between the first one's file write and controller sync"
            );
            let memory = lock_controller(&controller).settings().clone();
            assert_eq!(as_json(&memory), as_json(&store::load_from(&file)));
            assert_eq!(memory.language, Language::Norwegian);
            assert_eq!(memory.beam_size, 7);
            "serialized".into()
        },
    ) else {
        return;
    };
}

/// Polls `condition` until it holds or `limit` passes; whether it held.
fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    condition()
}
