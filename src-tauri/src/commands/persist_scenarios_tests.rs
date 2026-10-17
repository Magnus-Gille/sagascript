//! Scenarios for `persist_and_sync` beyond the table (#370): what an unrelated
//! setter must leave alone, and what two setters must not do at once. They use
//! the harness of the parent module (`isolated`, `app_with`, the rows).

use super::*;

/// The file holds shortcuts the OS could not register (the CLI wrote them, or a
/// hot reload failed), and the controller deliberately stays on the registered
/// ones: the watcher keeps it there, and key presses are resolved against it. A
/// setter of something else must not move it, or the watcher sees controller
/// and file agree, does nothing, and the registered shortcut stops working.
#[test]
fn an_unrelated_setter_keeps_the_controllers_registered_shortcuts() {
    let Some(_) = isolated(
        module_path!(),
        "an_unrelated_setter_keeps_the_controllers_registered_shortcuts",
        || {
            let registered = Settings::default().resolved_shortcuts();
            let runtime = runtime();
            let mut checked = Vec::new();
            for row in ROWS
                .iter()
                .filter(|row| matches!(row.mirror, Mirror::Field(_)))
            {
                let name = row.commands.join("+");
                let app = app_with(Settings::default());
                store::update(|s| {
                    s.hotkey = "F13".into();
                    s.hotkey_profiles = vec![sagascript_core::settings::HotkeyProfile {
                        id: "default".into(),
                        name: "Default".into(),
                        shortcut: "F13".into(),
                        push_to_talk_shortcut: None,
                        toggle_shortcut: None,
                        language: Language::English,
                    }];
                })
                .expect("the fake file is writable");
                (row.arrange)();
                assert_ne!(
                    store::load().resolved_shortcuts(),
                    registered,
                    "the file must hold other shortcuts than the controller"
                );

                (row.run)(&app, &runtime)
                    .unwrap_or_else(|error| panic!("{name}: the setter failed: {error}"));

                let after = lock_controller(&controller(&app)).settings().clone();
                assert_eq!(
                    after.resolved_shortcuts(),
                    registered,
                    "{name}: the controller adopted shortcuts the OS never registered"
                );
                assert_eq!(after.hotkey, Settings::default().hotkey, "{name}");
                assert_eq!(
                    store::load().hotkey,
                    "F13",
                    "{name}: the file keeps what was written"
                );
                checked.push(name);
            }
            checked.join("\n")
        },
    ) else {
        return;
    };
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

/// Two writers must not interleave between the file write and the controller
/// sync. Without that, the second's write lands in the first's window, and a
/// replacement of the whole copy by the first (written before the second) then
/// drops the second's change from the controller.
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
            let controller: SharedController = Mutex::new(AppController::new(Settings::default()));
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
