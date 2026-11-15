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

    let version = "1.4.6".to_string();
    set_update_menu(
        app.handle(),
        "Update available — v1.4.6",
        Some("Open Release v1.4.6…".to_string()),
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
        Some("Please wait…".to_string()),
        false,
        UpdateMenuMode::Available(version),
    );
    assert!(!status.is_enabled().unwrap());
    assert!(!action.is_enabled().unwrap());

    set_update_menu(
        app.handle(),
        "Sagascript is up to date",
        Some("Check Again…".to_string()),
        true,
        UpdateMenuMode::Idle,
    );
    assert!(!status.is_enabled().unwrap());
    assert!(action.is_enabled().unwrap());
    app.cleanup_before_exit();
}

#[test]
fn available_status_is_clickable_only_on_windows_while_the_action_is_enabled() {
    let mode = UpdateMenuMode::Available("1.4.6".to_string());
    assert_eq!(mode.status_enabled(true), cfg!(target_os = "windows"));
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
        available_version: Some("1.4.6".to_string()),
        attempt: 0,
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
fn checking_phase_disables_the_action_and_keeps_its_label() {
    // Like main before the orchestrator: a check started from "Check Again…",
    // "Try Again…" or "Install and Restart v…" keeps that label while disabled.
    let texts = texts_for(&UpdatePhase::Checking);
    assert_eq!(texts.status, "Checking for updates…");
    assert_eq!(texts.action, None);
    assert!(!texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Checking);
}

#[test]
fn available_release_with_install_offer_is_actionable() {
    let texts = texts_for(&UpdatePhase::Available {
        version: "1.2.0".to_string(),
        install_offered: true,
    });
    assert_eq!(texts.status, "Update available — v1.2.0");
    assert_eq!(texts.action.as_deref(), Some("Install and Restart v1.2.0…"));
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Available("1.2.0".to_string()));
}

#[test]
fn available_release_without_install_offer_opens_the_release_page() {
    let texts = texts_for(&UpdatePhase::Available {
        version: "1.2.0".to_string(),
        install_offered: false,
    });
    assert_eq!(texts.status, "Update available — v1.2.0");
    assert_eq!(texts.action.as_deref(), Some("Open Release v1.2.0…"));
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Available("1.2.0".to_string()));
}

#[test]
fn up_to_date_keeps_a_clear_recheck_action() {
    let texts = texts_for(&UpdatePhase::UpToDate);
    assert_eq!(texts.status, "Sagascript is up to date");
    assert_eq!(texts.action.as_deref(), Some("Check Again…"));
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Idle);
}

#[test]
fn check_failure_offers_a_retry() {
    let texts = texts_for(&UpdatePhase::Failed {
        failure: UpdateFailure::Check,
    });
    assert_eq!(texts.status, "Couldn't check for updates");
    assert_eq!(texts.action.as_deref(), Some("Try Again…"));
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Idle);
}

#[test]
fn download_progress_drives_the_action_percent() {
    let without_total = texts_for(&UpdatePhase::Downloading {
        version: "1.2.0".to_string(),
        percent: None,
    });
    assert_eq!(without_total.status, "Downloading update v1.2.0…");
    assert_eq!(without_total.action.as_deref(), Some("Downloading update…"));
    assert!(!without_total.enabled);
    assert_eq!(without_total.mode, UpdateMenuMode::Installing);

    let with_percent = texts_for(&UpdatePhase::Downloading {
        version: "1.2.0".to_string(),
        percent: Some(42),
    });
    assert_eq!(with_percent.status, "Downloading update v1.2.0…");
    assert_eq!(
        with_percent.action.as_deref(),
        Some("Downloading update… 42%")
    );
    assert!(!with_percent.enabled);
}

#[test]
fn install_flow_phases_keep_ownership_of_the_menu() {
    let waiting = texts_for(&UpdatePhase::WaitingForWork);
    assert_eq!(
        waiting.status,
        "Waiting for current dictation or transcription to finish…"
    );
    assert_eq!(waiting.action.as_deref(), Some("Waiting for current work…"));
    assert!(!waiting.enabled);
    assert_eq!(waiting.mode, UpdateMenuMode::Installing);

    let preparing = texts_for(&UpdatePhase::PreparingDrafts);
    assert_eq!(preparing.status, "Saving recoverable drafts…");
    assert_eq!(preparing.action.as_deref(), Some("Preparing update…"));
    assert!(!preparing.enabled);

    let installing = texts_for(&UpdatePhase::Installing {
        version: "1.2.0".to_string(),
    });
    assert_eq!(installing.status, "Installing update v1.2.0…");
    assert_eq!(installing.action.as_deref(), Some("Installing update…"));

    let installed = texts_for(&UpdatePhase::Installed {
        version: "1.2.0".to_string(),
    });
    assert_eq!(
        installed.status,
        "Update v1.2.0 installed; restarting Sagascript…"
    );
    assert_eq!(installed.action.as_deref(), Some("Restarting…"));
    assert!(!installed.enabled);
}

#[test]
fn install_failures_map_to_retryable_statuses() {
    let cases = [
        (UpdateFailure::Download, "Update download failed"),
        (UpdateFailure::WaitTimedOut, "Update wait timed out"),
        (
            UpdateFailure::PreparationBlocked,
            "Update preparation blocked",
        ),
        (
            UpdateFailure::PreparationFailed,
            "Update preparation failed",
        ),
        (
            UpdateFailure::RecoveryConflict,
            "Could not save drafts for update",
        ),
        (UpdateFailure::Install, "Update installation failed"),
    ];
    for (failure, status) in cases {
        let texts = texts_for(&UpdatePhase::Failed { failure });
        assert_eq!(texts.status, status);
        assert_eq!(texts.action.as_deref(), Some("Try Again…"));
        assert!(texts.enabled);
        assert_eq!(texts.mode, UpdateMenuMode::Idle);
    }
}

#[test]
fn update_action_targets_the_exact_stable_release() {
    assert_eq!(
        stable_release_url("1.2.0"),
        "https://github.com/Magnus-Gille/sagascript/releases/tag/v1.2.0"
    );
}

fn menu_state(checking: bool, attempt: u64) -> UpdateMenuState {
    UpdateMenuState {
        items: None,
        checking,
        installing: false,
        available_version: None,
        attempt,
    }
}

#[test]
fn a_check_that_ended_without_a_result_is_reset() {
    let mut state = menu_state(true, 4);
    assert!(check_attempt_needs_reset(&mut state, 4));
    assert!(!state.checking);
}

#[test]
fn a_check_ended_by_its_terminal_phase_is_left_alone() {
    // The terminal phase already cleared the flag on the main thread.
    let mut state = menu_state(false, 4);
    assert!(!check_attempt_needs_reset(&mut state, 4));
    assert!(!state.checking);
}

#[test]
fn the_end_of_an_older_check_never_resets_a_newer_one() {
    // Check 4 finished, check 5 started before 4's reset reached the main thread.
    let mut state = menu_state(true, 5);
    assert!(!check_attempt_needs_reset(&mut state, 4));
    assert!(state.checking, "the newer check keeps owning the menu");
}
