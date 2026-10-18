//! Presentation tests for the tray wording. The orchestration tests cover
//! *when* a phase happens; these cover *what the user reads*.

use super::TrayUpdateMenu;
use crate::updater::{UpdateFailure, UpdatePhase};
use crate::UpdateMenuMode;

fn texts(phase: &UpdatePhase) -> (String, String, bool) {
    let texts = TrayUpdateMenu::texts_for(phase);
    (texts.status, texts.action, texts.enabled)
}

#[test]
fn checking_disables_the_action_without_offering_a_redundant_one() {
    let (status, action, enabled) = texts(&UpdatePhase::Checking);
    assert_eq!(status, "Checking for updates…");
    assert_eq!(action, "Check for Updates…");
    assert!(!enabled);
    assert!(matches!(
        TrayUpdateMenu::texts_for(&UpdatePhase::Checking).mode,
        UpdateMenuMode::Checking
    ));
}

#[test]
fn an_available_signed_update_offers_an_in_app_restart() {
    let texts = TrayUpdateMenu::texts_for(&UpdatePhase::Available {
        version: "1.2.3".into(),
        install_offered: true,
    });
    assert_eq!(texts.status, "Update available — v1.2.3");
    assert_eq!(texts.action, "Install and Restart v1.2.3…");
    assert!(texts.enabled);
    assert_eq!(
        texts.mode,
        UpdateMenuMode::Available("1.2.3".to_owned()),
        "the tray click handler needs the exact version to install"
    );
}

#[test]
fn an_available_update_without_the_signed_updater_opens_the_release_page() {
    let (status, action, enabled) = texts(&UpdatePhase::Available {
        version: "1.2.0".into(),
        install_offered: false,
    });
    assert_eq!(status, "Update available — v1.2.0");
    assert_eq!(action, "Open Release v1.2.0…");
    assert!(enabled);
}

#[test]
fn up_to_date_stays_recheckable() {
    let (status, action, enabled) = texts(&UpdatePhase::UpToDate);
    assert_eq!(status, "Sagascript is up to date");
    assert_eq!(action, "Check Again…");
    assert!(enabled);
}

#[test]
fn download_progress_reports_the_percentage_when_the_total_is_known() {
    let (status, action, enabled) = texts(&UpdatePhase::Downloading {
        version: "2.0.0".into(),
        percent: Some(42),
    });
    assert_eq!(status, "Downloading update v2.0.0…");
    assert_eq!(action, "Downloading update… 42%");
    assert!(!enabled);

    let (status, action, _) = texts(&UpdatePhase::Downloading {
        version: "2.0.0".into(),
        percent: None,
    });
    assert_eq!(status, "Downloading update v2.0.0…");
    assert_eq!(action, "Downloading update…");
}

#[test]
fn every_install_phase_blocks_the_action_until_the_process_restarts() {
    for phase in [
        UpdatePhase::WaitingForWork {
            version: "2.0.0".into(),
        },
        UpdatePhase::PreparingDrafts {
            version: "2.0.0".into(),
        },
        UpdatePhase::Installing {
            version: "2.0.0".into(),
        },
        UpdatePhase::Installed {
            version: "2.0.0".into(),
        },
    ] {
        let texts = TrayUpdateMenu::texts_for(&phase);
        assert!(!texts.enabled, "{phase:?} must disable the action");
        assert!(matches!(texts.mode, UpdateMenuMode::Installing));
    }
    assert_eq!(
        texts(&UpdatePhase::WaitingForWork {
            version: "2.0.0".into()
        })
        .0,
        "Waiting for current dictation or transcription to finish…"
    );
    assert_eq!(
        texts(&UpdatePhase::PreparingDrafts {
            version: "2.0.0".into()
        })
        .1,
        "Preparing update…"
    );
    assert_eq!(
        texts(&UpdatePhase::Installed {
            version: "2.0.0".into()
        })
        .0,
        "Update v2.0.0 installed; restarting Sagascript…"
    );
}

#[test]
fn every_failure_explains_itself_and_offers_a_retry() {
    for failure in [
        UpdateFailure::Check,
        UpdateFailure::Download,
        UpdateFailure::WaitTimedOut,
        UpdateFailure::PreparationBlocked,
        UpdateFailure::PreparationFailed,
        UpdateFailure::RecoveryConflict,
        UpdateFailure::Install,
    ] {
        let texts = TrayUpdateMenu::texts_for(&UpdatePhase::Failed { failure });
        assert!(texts.enabled, "{failure} must re-enable the action");
        assert_eq!(texts.action, "Try Again…");
        assert!(matches!(texts.mode, UpdateMenuMode::Idle));
        assert!(!texts.status.is_empty());
    }
    assert_eq!(
        texts(&UpdatePhase::Failed {
            failure: UpdateFailure::RecoveryConflict
        })
        .0,
        "Could not save drafts for update"
    );
    assert_eq!(
        texts(&UpdatePhase::Failed {
            failure: UpdateFailure::WaitTimedOut
        })
        .0,
        "Update wait timed out"
    );
}
