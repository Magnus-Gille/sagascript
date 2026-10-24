use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

struct RecordingSink {
    phases: Mutex<Vec<UpdatePhase>>,
}

impl RecordingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            phases: Mutex::new(Vec::new()),
        })
    }

    fn phases(&self) -> Vec<UpdatePhase> {
        self.phases.lock().unwrap().clone()
    }
}

// The sink is shared through the orchestrator's `Arc<S>`; the trait lives in
// this crate, so implementing it for the local type is fine.
impl MenuSink for RecordingSink {
    fn show(&self, phase: &UpdatePhase) {
        self.phases.lock().unwrap().push(phase.clone());
    }
}

/// The orchestrator's check source. Its result is what it returns, unchanged.
struct FakeCheck {
    result: Result<Option<CheckedRelease>, String>,
}

impl FakeCheck {
    fn available(version: &str, install_offered: bool) -> Self {
        Self {
            result: Ok(Some(CheckedRelease {
                version: version.to_owned(),
                install_offered,
            })),
        }
    }

    fn up_to_date() -> Self {
        Self { result: Ok(None) }
    }

    fn failing(message: &str) -> Self {
        Self {
            result: Err(message.to_owned()),
        }
    }
}

impl CheckSource for FakeCheck {
    fn check(&self) -> BoxFuture<'_, Result<Option<CheckedRelease>, String>> {
        Box::pin(async { self.result.clone() })
    }
}

/// Permit handed out by [`FakeRecovery`]. Not `Copy`: dropping it is what
/// releases the window, and the drop must run exactly once per acquisition.
/// No `PartialEq`: `AtomicUsize` does not implement it, and permits are
/// compared through their ids and release counters instead.
#[derive(Debug, Clone)]
struct FakePermit {
    id: usize,
    released: Arc<AtomicUsize>,
}

impl Drop for FakePermit {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }
}

enum Step {
    Ready,
    WorkActive,
    Blocked,
    Failed(&'static str),
}

struct FakeRecovery {
    script: Mutex<VecDeque<Step>>,
    prepare_error: Option<String>,
    preparations: Arc<AtomicUsize>,
    acquisitions: Arc<AtomicUsize>,
    released: Arc<AtomicUsize>,
    next_id: AtomicUsize,
}

impl FakeRecovery {
    fn new(script: impl IntoIterator<Item = Step>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            prepare_error: None,
            preparations: Arc::new(AtomicUsize::new(0)),
            acquisitions: Arc::new(AtomicUsize::new(0)),
            released: Arc::new(AtomicUsize::new(0)),
            next_id: AtomicUsize::new(0),
        }
    }

    fn with_prepare_error(mut self, error: &str) -> Self {
        self.prepare_error = Some(error.to_owned());
        self
    }
}

impl Recovery for FakeRecovery {
    type Permit = FakePermit;

    fn try_begin(&self) -> ActivityAttempt<FakePermit> {
        match self.script.lock().unwrap().pop_front() {
            None => ActivityAttempt::Failed("recovery script exhausted".into()),
            Some(Step::Ready) => {
                self.acquisitions.fetch_add(1, Ordering::SeqCst);
                ActivityAttempt::Ready(FakePermit {
                    id: self.next_id.fetch_add(1, Ordering::SeqCst),
                    released: Arc::clone(&self.released),
                })
            }
            Some(Step::WorkActive) => ActivityAttempt::WorkActive,
            Some(Step::Blocked) => ActivityAttempt::Blocked,
            Some(Step::Failed(message)) => ActivityAttempt::Failed(message.into()),
        }
    }

    fn prepare<'a>(&'a self, _permit: &FakePermit) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.preparations.fetch_add(1, Ordering::SeqCst);
            match &self.prepare_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        })
    }
}

struct FakeProvider {
    download_result: Result<Vec<u8>, String>,
    progress: Vec<Option<u8>>,
    install_error: Option<String>,
    installs: AtomicUsize,
}

impl FakeProvider {
    fn new() -> Self {
        Self {
            download_result: Ok(vec![1, 2, 3]),
            progress: Vec::new(),
            install_error: None,
            installs: AtomicUsize::new(0),
        }
    }

    fn failing_download(message: &str) -> Self {
        Self {
            download_result: Err(message.to_owned()),
            ..Self::new()
        }
    }

    fn with_progress(self, progress: Vec<Option<u8>>) -> Self {
        Self { progress, ..self }
    }

    fn with_install_error(self, message: &str) -> Self {
        Self {
            install_error: Some(message.to_owned()),
            ..self
        }
    }
}

impl UpdateProvider for FakeProvider {
    fn download(&self, on_progress: ProgressSink) -> BoxFuture<'_, Result<Vec<u8>, String>> {
        Box::pin(async move {
            for percent in &self.progress {
                on_progress(*percent);
            }
            self.download_result.clone()
        })
    }

    fn install(&self, bytes: &[u8]) -> Result<(), String> {
        assert_eq!(bytes, [1, 2, 3]);
        self.installs.fetch_add(1, Ordering::SeqCst);
        match &self.install_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

fn instant_wait(timeout: Duration) -> WaitPolicy {
    WaitPolicy {
        poll_interval: Duration::ZERO,
        timeout,
    }
}

fn success_wait() -> WaitPolicy {
    instant_wait(Duration::from_secs(5))
}

#[tokio::test]
async fn check_shows_available_release_with_its_install_intent() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());

    let checked = orchestrator
        .check(&FakeCheck::available("1.2.3", true))
        .await
        .unwrap();

    assert_eq!(
        checked,
        Some(CheckedRelease {
            version: "1.2.3".into(),
            install_offered: true,
        })
    );
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Checking,
            UpdatePhase::Available {
                version: "1.2.3".into(),
                install_offered: true,
            },
        ]
    );
}

#[tokio::test]
async fn check_announces_up_to_date() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());

    assert_eq!(orchestrator.check(&FakeCheck::up_to_date()).await, Ok(None));
    assert_eq!(
        sink.phases(),
        vec![UpdatePhase::Checking, UpdatePhase::UpToDate]
    );
}

#[tokio::test]
async fn check_reports_failure_and_marks_the_phase_failed() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());

    let error = orchestrator
        .check(&FakeCheck::failing("network down"))
        .await
        .unwrap_err();

    assert_eq!(error, "network down");
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Checking,
            UpdatePhase::Failed {
                failure: UpdateFailure::Check,
            },
        ]
    );
}

#[tokio::test]
async fn checking_guard_runs_even_when_the_update_task_panics() {
    let ran = Arc::new(AtomicUsize::new(0));
    let guard_ran = Arc::clone(&ran);
    let result = std::panic::catch_unwind(move || {
        let _guard = CheckingGuard::new(move || guard_ran.store(1, Ordering::SeqCst));
        panic!("update task crashed");
    });

    assert!(result.is_err());
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[test]
fn checking_guard_resets_when_the_task_returns_early() {
    let ran = Arc::new(AtomicUsize::new(0));
    {
        let guard_ran = Arc::clone(&ran);
        let _guard = CheckingGuard::new(move || guard_ran.store(1, Ordering::SeqCst));
    }
    assert_eq!(ran.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn install_reports_the_signed_sequence_in_order() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]);
    let preparations = Arc::clone(&recovery.preparations);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    let InstallOutcome::RestartReady { permit } = outcome else {
        panic!("expected the install to finish restart-ready");
    };
    assert_eq!(permit.id, 0);
    assert_eq!(preparations.load(Ordering::SeqCst), 1);
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::PreparingDrafts,
            UpdatePhase::Installing {
                version: "1.2.3".into(),
            },
            UpdatePhase::Installed {
                version: "1.2.3".into(),
            },
        ]
    );
}

#[tokio::test]
async fn install_repeated_progress_reports_do_not_reach_the_sink() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]);
    let provider =
        FakeProvider::new().with_progress(vec![None, None, Some(40), Some(40), Some(40), Some(90)]);

    let outcome = orchestrator
        .install_with_wait(provider, recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(outcome, InstallOutcome::RestartReady { .. }));
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: Some(40),
            },
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: Some(90),
            },
            UpdatePhase::PreparingDrafts,
            UpdatePhase::Installing {
                version: "1.2.3".into(),
            },
            UpdatePhase::Installed {
                version: "1.2.3".into(),
            },
        ]
    );
}

#[tokio::test]
async fn install_stops_on_download_failure_before_touching_recovery() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]);

    let outcome = orchestrator
        .install_with_wait(
            FakeProvider::failing_download("checksum mismatch"),
            recovery,
            "1.2.3",
            success_wait(),
        )
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::Download
        }
    ));
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::Failed {
                failure: UpdateFailure::Download,
            },
        ]
    );
}

#[tokio::test]
async fn install_gives_up_when_active_work_outlasts_the_wait() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::WorkActive]);

    let outcome = orchestrator
        .install_with_wait(
            FakeProvider::new(),
            recovery,
            "1.2.3",
            instant_wait(Duration::ZERO),
        )
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::WaitTimedOut
        }
    ));
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::Failed {
                failure: UpdateFailure::WaitTimedOut,
            },
        ]
    );
}

#[tokio::test]
async fn install_waits_through_work_and_takes_one_permit() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::WorkActive, Step::WorkActive, Step::Ready]);
    let acquisitions = Arc::clone(&recovery.acquisitions);
    let released = Arc::clone(&recovery.released);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    let InstallOutcome::RestartReady { permit } = outcome else {
        panic!("expected the install to finish restart-ready");
    };
    assert_eq!(acquisitions.load(Ordering::SeqCst), 1);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(permit);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::WaitingForWork,
            UpdatePhase::WaitingForWork,
            UpdatePhase::PreparingDrafts,
            UpdatePhase::Installing {
                version: "1.2.3".into(),
            },
            UpdatePhase::Installed {
                version: "1.2.3".into(),
            },
        ]
    );
}

#[tokio::test]
async fn install_stops_when_the_recovery_window_is_blocked() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Blocked]);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::PreparationBlocked
        }
    ));
}

#[tokio::test]
async fn install_stops_when_the_recovery_window_cannot_be_entered() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Failed("an update is already being applied")]);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::PreparationFailed
        }
    ));
}

#[tokio::test]
async fn cancelled_draft_preparation_stops_the_update() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready])
        .with_prepare_error("Settings closed before recovery drafts were saved.");

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::RecoveryConflict
        }
    ));
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::PreparingDrafts,
            UpdatePhase::Failed {
                failure: UpdateFailure::RecoveryConflict,
            },
        ]
    );
}

#[tokio::test]
async fn failed_install_releases_the_permit_and_reports_the_failure() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]);
    let released = Arc::clone(&recovery.released);
    let provider = FakeProvider::new().with_install_error("package rejected");

    let outcome = orchestrator
        .install_with_wait(provider, recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::Install
        }
    ));
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(
        sink.phases(),
        vec![
            UpdatePhase::Downloading {
                version: "1.2.3".into(),
                percent: None,
            },
            UpdatePhase::PreparingDrafts,
            UpdatePhase::Installing {
                version: "1.2.3".into(),
            },
            UpdatePhase::Failed {
                failure: UpdateFailure::Install,
            },
        ]
    );
}

#[tokio::test]
async fn cancelled_preparation_releases_the_permit_it_held() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]).with_prepare_error("draft save failed");
    let released = Arc::clone(&recovery.released);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    assert!(matches!(
        outcome,
        InstallOutcome::Stopped {
            failure: UpdateFailure::RecoveryConflict
        }
    ));
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn the_wait_window_expires_exactly_at_the_timeout() {
    let policy = WaitPolicy::PRODUCTION;
    assert!(!policy.expired(policy.timeout - Duration::from_secs(1)));
    assert!(policy.expired(policy.timeout));
}

#[tokio::test]
async fn successful_install_releases_the_permit_only_when_the_caller_drops_it() {
    let sink = RecordingSink::new();
    let orchestrator = Orchestrator::new(sink.clone());
    let recovery = FakeRecovery::new([Step::Ready]);
    let released = Arc::clone(&recovery.released);

    let outcome = orchestrator
        .install_with_wait(FakeProvider::new(), recovery, "1.2.3", success_wait())
        .await;

    let InstallOutcome::RestartReady { permit } = outcome else {
        panic!("expected the install to finish restart-ready");
    };
    assert_eq!(permit.id, 0);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(permit);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}
