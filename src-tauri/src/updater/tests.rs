use super::*;

#[test]
fn checking_phase_shows_a_disabled_check_action() {
    let texts = texts_for(&UpdatePhase::Checking);
    assert_eq!(texts.status, "Checking for updates…");
    assert_eq!(texts.action, UPDATE_CHECK_ACTION);
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
    assert_eq!(texts.action, "Install and Restart v1.2.0…");
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
    assert_eq!(texts.action, "Open Release v1.2.0…");
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Available("1.2.0".to_string()));
}

#[test]
fn up_to_date_keeps_a_clear_recheck_action() {
    let texts = texts_for(&UpdatePhase::UpToDate);
    assert_eq!(texts.status, "Sagascript is up to date");
    assert_eq!(texts.action, "Check Again…");
    assert!(texts.enabled);
    assert_eq!(texts.mode, UpdateMenuMode::Idle);
}

#[test]
fn check_failure_offers_a_retry() {
    let texts = texts_for(&UpdatePhase::Failed {
        failure: UpdateFailure::Check,
    });
    assert_eq!(texts.status, "Couldn't check for updates");
    assert_eq!(texts.action, "Try Again…");
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
    assert_eq!(without_total.action, "Downloading update…");
    assert!(!without_total.enabled);
    assert_eq!(without_total.mode, UpdateMenuMode::Installing);

    let with_percent = texts_for(&UpdatePhase::Downloading {
        version: "1.2.0".to_string(),
        percent: Some(42),
    });
    assert_eq!(with_percent.status, "Downloading update v1.2.0…");
    assert_eq!(with_percent.action, "Downloading update… 42%");
    assert!(!with_percent.enabled);
}

#[test]
fn install_flow_phases_keep_ownership_of_the_menu() {
    let waiting = texts_for(&UpdatePhase::WaitingForWork);
    assert_eq!(
        waiting.status,
        "Waiting for current dictation or transcription to finish…"
    );
    assert_eq!(waiting.action, "Waiting for current work…");
    assert!(!waiting.enabled);
    assert_eq!(waiting.mode, UpdateMenuMode::Installing);

    let preparing = texts_for(&UpdatePhase::PreparingDrafts);
    assert_eq!(preparing.status, "Saving recoverable drafts…");
    assert_eq!(preparing.action, "Preparing update…");
    assert!(!preparing.enabled);

    let installing = texts_for(&UpdatePhase::Installing {
        version: "1.2.0".to_string(),
    });
    assert_eq!(installing.status, "Installing update v1.2.0…");
    assert_eq!(installing.action, "Installing update…");

    let installed = texts_for(&UpdatePhase::Installed {
        version: "1.2.0".to_string(),
    });
    assert_eq!(
        installed.status,
        "Update v1.2.0 installed; restarting Sagascript…"
    );
    assert_eq!(installed.action, "Restarting…");
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
        assert_eq!(texts.action, "Try Again…");
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
