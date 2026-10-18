//! Orchestration tests driven by fakes: no Tauri, no network, no six-minute
//! waits. Each branch of the install sequence gets its own assertion.

use super::tests_support::{FakeProvider, FakeRecovery, RecordingSink, Script};
use super::*;
use std::sync::atomic::Ordering;

fn instant_wait() -> WaitPolicy {
    WaitPolicy {
        poll_interval: Duration::ZERO,
        timeout: Duration::ZERO,
    }
}

fn patient_wait() -> WaitPolicy {
    WaitPolicy {
        poll_interval: Duration::ZERO,
        timeout: Duration::from_secs(600),
    }
}

fn install_with_sink(
    provider: FakeProvider,
    recovery: FakeRecovery,
    wait: WaitPolicy,
) -> (InstallOutcome<usize>, Vec<UpdatePhase>) {
    let sink = Arc::new(RecordingSink::default());
    let orchestrator = Orchestrator::new(provider, RecordingSinkHandle(sink.clone()), recovery)
        .with_wait_policy(wait);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("test runtime");
    let outcome = runtime.block_on(orchestrator.install("1.2.3"));
    (outcome, sink.phases())
}

/// Wraps the shared recording sink so the test keeps a handle after the
/// orchestrator takes ownership.
struct RecordingSinkHandle(Arc<RecordingSink>);

impl MenuSink for RecordingSinkHandle {
    fn show(&self, phase: &UpdatePhase) {
        self.0.show(phase);
    }
}

#[test]
fn the_full_sequence_downloads_waits_prepares_installs_and_hands_back_the_permit() {
    let provider = FakeProvider::ok();
    let installs = provider.installs.clone();
    let recovery = FakeRecovery::always_ready();
    let preparations = recovery.preparations.clone();

    let (outcome, phases) = install_with_sink(provider, recovery, patient_wait());

    assert_eq!(outcome, InstallOutcome::RestartReady { permit: 7 });
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    assert_eq!(preparations.load(Ordering::SeqCst), 1);
    assert_eq!(
        phases,
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None
            },
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: Some(40)
            },
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: Some(100)
            },
            UpdatePhase::PreparingDrafts {
                version: "1.2.3".into()
            },
            UpdatePhase::Installing {
                version: "1.2.3".into()
            },
            UpdatePhase::Installed {
                version: "1.2.3".into()
            },
        ]
    );
}

#[test]
fn repeated_progress_percentages_are_reported_once() {
    let provider = FakeProvider {
        progress: vec![Some(10), Some(10), Some(10), Some(11)],
        ..FakeProvider::ok()
    };

    let (_, phases) = install_with_sink(provider, FakeRecovery::always_ready(), patient_wait());

    let reported: Vec<Option<u8>> = phases
        .iter()
        .filter_map(|phase| match phase {
            UpdatePhase::Downloading { percent, .. } => Some(*percent),
            _ => None,
        })
        .collect();
    assert_eq!(reported, vec![None, Some(10), Some(11)]);
}

#[test]
fn a_download_failure_stops_before_any_wait_preparation_or_install() {
    let provider = FakeProvider::download_error("manifest unreachable");
    let installs = provider.installs.clone();
    let recovery = FakeRecovery::always_ready();
    let preparations = recovery.preparations.clone();

    let (outcome, phases) = install_with_sink(provider, recovery, patient_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::Download
        }
    );
    assert_eq!(installs.load(Ordering::SeqCst), 0);
    assert_eq!(preparations.load(Ordering::SeqCst), 0);
    assert!(phases.iter().any(|phase| matches!(
        phase,
        UpdatePhase::Failed {
            failure: UpdateFailure::Download
        }
    )));
    assert!(!phases
        .iter()
        .any(|phase| matches!(phase, UpdatePhase::WaitingForWork { .. })));
}

#[test]
fn work_that_never_idles_times_out_without_touching_the_release() {
    let recovery = FakeRecovery::scripted([Script::WorkActive; 4]);
    let preparations = recovery.preparations.clone();

    // A zero-length window expires on the first poll, so the outcome does not
    // depend on how long the test process happens to take.
    let (outcome, phases) = install_with_sink(FakeProvider::ok(), recovery, instant_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::WaitTimedOut
        }
    );
    assert_eq!(preparations.load(Ordering::SeqCst), 0);
    assert!(phases.iter().any(|phase| matches!(
        phase,
        UpdatePhase::Failed {
            failure: UpdateFailure::WaitTimedOut
        }
    )));
}

#[test]
fn work_finishing_inside_the_wait_window_lets_the_update_through() {
    let recovery = FakeRecovery::scripted([Script::WorkActive, Script::WorkActive, Script::Ready]);

    let (outcome, phases) = install_with_sink(FakeProvider::ok(), recovery, patient_wait());

    assert_eq!(outcome, InstallOutcome::RestartReady { permit: 7 });
    assert!(
        phases
            .iter()
            .any(|phase| matches!(phase, UpdatePhase::WaitingForWork { .. })),
        "the tray must explain why the update is not progressing yet"
    );
}

#[test]
fn an_exclusive_permit_rejected_for_unsaved_results_stops_before_preparation() {
    let recovery = FakeRecovery::scripted([Script::Blocked]);
    let preparations = recovery.preparations.clone();

    let (outcome, _) = install_with_sink(FakeProvider::ok(), recovery, patient_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::PreparationBlocked
        }
    );
    assert_eq!(preparations.load(Ordering::SeqCst), 0);
}

#[test]
fn an_unexpected_exclusive_failure_stops_before_preparation() {
    let recovery = FakeRecovery::scripted([Script::Failed]);
    let preparations = recovery.preparations.clone();

    let (outcome, _) = install_with_sink(FakeProvider::ok(), recovery, patient_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::PreparationFailed
        }
    );
    assert_eq!(preparations.load(Ordering::SeqCst), 0);
}

#[test]
fn a_recovery_conflict_during_draft_save_stops_before_install() {
    let provider = FakeProvider::ok();
    let installs = provider.installs.clone();
    let recovery = FakeRecovery::always_ready().prepare_error("different draft on disk");
    let preparations = recovery.preparations.clone();

    let (outcome, phases) = install_with_sink(provider, recovery, patient_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::RecoveryConflict
        }
    );
    assert_eq!(preparations.load(Ordering::SeqCst), 1);
    assert_eq!(installs.load(Ordering::SeqCst), 0);
    assert!(phases.iter().any(|phase| matches!(
        phase,
        UpdatePhase::Failed {
            failure: UpdateFailure::RecoveryConflict
        }
    )));
}

#[test]
fn a_failed_install_never_reports_a_restart() {
    let provider = FakeProvider::ok().install_error("signature mismatch");
    let installs = provider.installs.clone();

    let (outcome, phases) =
        install_with_sink(provider, FakeRecovery::always_ready(), patient_wait());

    assert_eq!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::Install
        }
    );
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    assert!(phases.iter().any(|phase| matches!(
        phase,
        UpdatePhase::Failed {
            failure: UpdateFailure::Install
        }
    )));
    assert!(!phases
        .iter()
        .any(|phase| matches!(phase, UpdatePhase::Installed { .. })));
}

#[test]
fn the_wait_window_expires_exactly_at_the_timeout() {
    let policy = WaitPolicy::PRODUCTION;
    assert!(!policy.expired(Duration::from_secs(599)));
    assert!(policy.expired(Duration::from_secs(600)));
}

#[test]
fn the_checking_flag_is_cleared_when_the_check_task_panics() {
    use std::sync::atomic::AtomicBool;

    let checking = Arc::new(AtomicBool::new(true));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = CheckingGuard::new({
            let checking = Arc::clone(&checking);
            move || checking.store(false, Ordering::SeqCst)
        });
        panic!("the spawned update-check task blew up");
    }));
    assert!(panicked.is_err());
    assert!(
        !checking.load(Ordering::SeqCst),
        "a panicking check task must still release the tray action"
    );
}

#[test]
fn the_checking_flag_is_cleared_when_the_check_task_returns_early() {
    use std::sync::atomic::AtomicBool;

    fn run_check_task(checking: &Arc<AtomicBool>) {
        let _guard = CheckingGuard::new({
            let checking = Arc::clone(checking);
            move || checking.store(false, Ordering::SeqCst)
        });
        // Early return, as when no tray items exist yet.
    }

    let checking = Arc::new(AtomicBool::new(true));
    run_check_task(&checking);
    assert!(
        !checking.load(Ordering::SeqCst),
        "an early return must still release the tray action"
    );
}
