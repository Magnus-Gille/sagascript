use super::*;

/// Uses only updater state and native menus, never the user's settings or models.
#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires a Windows desktop; run alone with --test-threads=1"]
fn native_runner_update_menu_smoke() {
    let app = tauri::Builder::default()
        .any_thread()
        .build(tauri::generate_context!())
        .expect("build native menu test runtime");
    manage_update_menu_state(&app);
    let status = MenuItem::with_id(
        &app,
        "update_status",
        "Updates: not checked",
        false,
        None::<&str>,
    )
    .unwrap();
    let action = MenuItem::with_id(
        &app,
        "check_for_updates",
        UPDATE_CHECK_ACTION,
        true,
        None::<&str>,
    )
    .unwrap();
    register_update_menu_items(&app, &status, &action);
    assert!(!status.is_enabled().unwrap());

    let version = semver::Version::new(1, 4, 6);
    set_update_menu(
        app.handle(),
        "Update available — v1.4.6",
        "Open Release v1.4.6…",
        true,
        UpdateMenuMode::Available(version.clone()),
    );
    assert!(status.is_enabled().unwrap());
    assert!(action.is_enabled().unwrap());
    assert_eq!(status.text().unwrap(), "Update available — v1.4.6");
    {
        let state = app.state::<SharedUpdateMenuState>();
        let state = lock_or_recover(&state, UPDATE_MENU_LOCK);
        assert_eq!(state.available_version, Some(version.clone()));
        assert_eq!(state.action(true), UpdateMenuAction::OpenRelease);
    }

    set_update_menu(
        app.handle(),
        "Update unavailable while busy",
        "Please wait…",
        false,
        UpdateMenuMode::Available(version),
    );
    assert!(!status.is_enabled().unwrap());
    assert!(!action.is_enabled().unwrap());

    set_update_menu(
        app.handle(),
        "Sagascript is up to date",
        "Check Again…",
        true,
        UpdateMenuMode::Idle,
    );
    assert!(!status.is_enabled().unwrap());
    assert!(action.is_enabled().unwrap());
    app.cleanup_before_exit();
}

#[test]
fn available_status_is_clickable_only_while_the_action_is_enabled() {
    let mode = UpdateMenuMode::Available(semver::Version::new(1, 4, 6));
    assert!(mode.status_enabled(true));
    assert!(!mode.status_enabled(false));
    assert!(!UpdateMenuMode::Idle.status_enabled(true));
    #[cfg(target_os = "macos")]
    assert!(!UpdateMenuMode::Installing.status_enabled(true));
}

#[test]
fn release_action_respects_platform_support_and_busy_state() {
    let mut state = UpdateMenuState {
        items: None,
        checking: false,
        installing: false,
        available_version: Some(semver::Version::new(1, 4, 6)),
    };
    assert_eq!(state.action(false), UpdateMenuAction::OpenRelease);
    assert_eq!(
        state.action(true),
        if cfg!(target_os = "macos") {
            UpdateMenuAction::Install
        } else {
            UpdateMenuAction::OpenRelease
        }
    );
    state.checking = true;
    assert_eq!(state.action(true), UpdateMenuAction::None);
    state.checking = false;
    state.installing = true;
    assert_eq!(state.action(false), UpdateMenuAction::None);
    state.installing = false;
    state.available_version = None;
    assert_eq!(state.action(false), UpdateMenuAction::Check);
}

#[test]
fn update_status_describes_available_and_current_releases() {
    assert_eq!(
        update_status_text(&updates::UpdateCheck::Available {
            version: semver::Version::new(1, 2, 3)
        }),
        "Update available — v1.2.3"
    );
    assert_eq!(
        update_status_text(&updates::UpdateCheck::UpToDate),
        "Sagascript is up to date"
    );
    assert_eq!(
        update_action_text(&updates::UpdateCheck::Available {
            version: semver::Version::new(1, 2, 3)
        }),
        "Open Release v1.2.3…"
    );
}

#[test]
fn update_menu_makes_available_release_actionable() {
    let result = updates::UpdateCheck::Available {
        version: semver::Version::new(1, 2, 0),
    };

    assert_eq!(update_status_text(&result), "Update available — v1.2.0");
    assert_eq!(update_action_text(&result), "Open Release v1.2.0…");
}

#[test]
fn update_menu_keeps_a_clear_recheck_action_when_current() {
    assert_eq!(
        update_action_text(&updates::UpdateCheck::UpToDate),
        "Check Again…"
    );
}

#[test]
fn update_action_targets_the_exact_stable_release() {
    assert_eq!(
        stable_release_url(&semver::Version::new(1, 2, 0)),
        "https://github.com/Magnus-Gille/sagascript/releases/tag/v1.2.0"
    );
}
