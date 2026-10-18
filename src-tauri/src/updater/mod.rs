//! Signed-update orchestration (#376).
//!
//! The install sequence — download, wait until no dictation or transcription
//! is running, save recoverable drafts, install, restart — is one linear
//! story told once, here, instead of being spread across `main.rs` and the
//! tray menu. Every external dependency is an injected seam:
//!
//! * [`UpdateProvider`] — download/verify and install the release payload,
//! * [`Recovery`] — exclusive activity permit plus draft preparation,
//! * [`MenuSink`] — every user-facing tray string.
//!
//! The orchestration is therefore testable without Tauri, without network and
//! without waiting six hundred seconds.

pub mod tray_menu;

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sagascript_core::sync::lock_or_recover;
use tracing::warn;

pub use tray_menu::TrayUpdateMenu;
#[cfg(target_os = "macos")]
pub use tray_menu::{DraftRecovery, SignedUpdate};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Progress callback for a download: `None` while the total size is unknown.
pub type ProgressSink = Arc<dyn Fn(Option<u8>) + Send + Sync>;

const PROGRESS_LOCK: &str = "update.orchestrator.progress";

/// Semantic update states. Nothing here is user-facing text: the tray
/// wording belongs to the [`MenuSink`] implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePhase {
    Checking,
    UpToDate,
    Available {
        version: String,
        /// The signed updater can install in-app; otherwise the release page
        /// is opened instead.
        install_offered: bool,
    },
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    WaitingForWork {
        version: String,
    },
    PreparingDrafts {
        version: String,
    },
    Installing {
        version: String,
    },
    Installed {
        version: String,
    },
    Failed {
        failure: UpdateFailure,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateFailure {
    Check,
    Download,
    WaitTimedOut,
    PreparationBlocked,
    PreparationFailed,
    RecoveryConflict,
    Install,
}

impl fmt::Display for UpdateFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            UpdateFailure::Check => "update check failed",
            UpdateFailure::Download => "update download failed",
            UpdateFailure::WaitTimedOut => "update wait timed out",
            UpdateFailure::PreparationBlocked => "update preparation blocked",
            UpdateFailure::PreparationFailed => "update preparation failed",
            UpdateFailure::RecoveryConflict => "recovery drafts could not be saved",
            UpdateFailure::Install => "update installation failed",
        };
        formatter.write_str(text)
    }
}

/// Owns the tray wording. `texts_for` is an associated function so the
/// mapping can be asserted without a running app.
pub trait MenuSink: Send + Sync + 'static {
    fn show(&self, phase: &UpdatePhase);
}

/// Downloads, verifies and installs a release payload.
pub trait UpdateProvider: Send + Sync + 'static {
    fn download(&self, on_progress: ProgressSink) -> BoxFuture<'_, Result<Vec<u8>, String>>;

    fn install(&self, bytes: &[u8]) -> Result<(), String>;
}

/// The result of asking whether the app may be replaced right now.
#[derive(Debug, PartialEq, Eq)]
pub enum ActivityAttempt<Permit> {
    Ready(Permit),
    WorkActive,
    /// Unsaved results rejected the exclusive permit.
    Blocked,
    Failed(String),
}

/// Saves recoverable drafts under an exclusive activity permit.
pub trait Recovery: Send + Sync + 'static {
    /// Held by the caller until the process restarts, so nothing starts new
    /// work in the window between install and exit.
    type Permit: Send;

    fn try_begin(&self) -> ActivityAttempt<Self::Permit>;

    fn prepare<'a>(&'a self, permit: &Self::Permit) -> BoxFuture<'a, Result<(), String>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitPolicy {
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl WaitPolicy {
    pub const PRODUCTION: Self = Self {
        poll_interval: Duration::from_millis(250),
        timeout: Duration::from_secs(600),
    };

    pub fn expired(&self, elapsed: Duration) -> bool {
        elapsed >= self.timeout
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome<Permit> {
    /// The payload is installed; the caller restarts while holding `permit`.
    RestartReady {
        permit: Permit,
    },
    Stopped {
        failure: UpdateFailure,
    },
}

/// Clears the tray "checking" flag on every exit of the check task, including
/// a panic. The flag used to depend on each early return remembering it.
pub struct CheckingGuard {
    reset: Option<Box<dyn FnOnce() + Send>>,
}

impl CheckingGuard {
    pub fn new(reset: impl FnOnce() + Send + 'static) -> Self {
        Self {
            reset: Some(Box::new(reset)),
        }
    }
}

impl Drop for CheckingGuard {
    fn drop(&mut self) {
        if let Some(reset) = self.reset.take() {
            reset();
        }
    }
}

pub struct Orchestrator<P, S, R> {
    provider: P,
    sink: Arc<S>,
    recovery: R,
    wait: WaitPolicy,
}

impl<P, S, R> Orchestrator<P, S, R> {
    pub fn new(provider: P, sink: S, recovery: R) -> Self {
        Self {
            provider,
            sink: Arc::new(sink),
            recovery,
            wait: WaitPolicy::PRODUCTION,
        }
    }

    /// Tests inject a wait window that expires immediately instead of
    /// sleeping through the production ten-minute budget.
    #[cfg(test)]
    pub fn with_wait_policy(mut self, wait: WaitPolicy) -> Self {
        self.wait = wait;
        self
    }
}

impl<P, S, R> Orchestrator<P, S, R>
where
    P: UpdateProvider,
    S: MenuSink,
    R: Recovery,
{
    /// Runs download → wait for idle work → save drafts → install.
    ///
    /// Returns the exclusive permit on success so the caller can restart
    /// without releasing it; every failure path reports the phase first.
    pub async fn install(self, version: &str) -> InstallOutcome<R::Permit> {
        let Self {
            provider,
            sink,
            recovery,
            wait,
        } = self;

        sink.show(&UpdatePhase::Downloading {
            version: version.to_owned(),
            percent: None,
        });

        let last_percent = Arc::new(Mutex::new(None));
        let on_progress: ProgressSink = {
            let sink = Arc::clone(&sink);
            let version = version.to_owned();
            let last_percent = Arc::clone(&last_percent);
            Arc::new(move |percent| {
                let mut last = lock_or_recover(&last_percent, PROGRESS_LOCK);
                if *last != percent {
                    *last = percent;
                    sink.show(&UpdatePhase::Downloading {
                        version: version.clone(),
                        percent,
                    });
                }
            })
        };

        let bytes = match provider.download(on_progress).await {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!("Signed updater download/verification failed: {error}");
                let failure = UpdateFailure::Download;
                sink.show(&UpdatePhase::Failed { failure });
                return InstallOutcome::Stopped { failure };
            }
        };

        let started = Instant::now();
        let permit = loop {
            match recovery.try_begin() {
                ActivityAttempt::Ready(permit) => break permit,
                ActivityAttempt::WorkActive => {
                    if wait.expired(started.elapsed()) {
                        warn!(
                            "Update wait exceeded {:?}; current work remains untouched",
                            wait.timeout
                        );
                        let failure = UpdateFailure::WaitTimedOut;
                        sink.show(&UpdatePhase::Failed { failure });
                        return InstallOutcome::Stopped { failure };
                    }
                    sink.show(&UpdatePhase::WaitingForWork {
                        version: version.to_owned(),
                    });
                }
                ActivityAttempt::Blocked => {
                    warn!("Update activity unexpectedly rejected the recovery path");
                    let failure = UpdateFailure::PreparationBlocked;
                    sink.show(&UpdatePhase::Failed { failure });
                    return InstallOutcome::Stopped { failure };
                }
                ActivityAttempt::Failed(error) => {
                    warn!("Could not safely apply update: {error}");
                    let failure = UpdateFailure::PreparationFailed;
                    sink.show(&UpdatePhase::Failed { failure });
                    return InstallOutcome::Stopped { failure };
                }
            }
            tokio::time::sleep(wait.poll_interval).await;
        };

        sink.show(&UpdatePhase::PreparingDrafts {
            version: version.to_owned(),
        });
        if let Err(error) = recovery.prepare(&permit).await {
            warn!("Could not safely prepare update: {error}");
            let failure = UpdateFailure::RecoveryConflict;
            sink.show(&UpdatePhase::Failed { failure });
            return InstallOutcome::Stopped { failure };
        }

        sink.show(&UpdatePhase::Installing {
            version: version.to_owned(),
        });
        if let Err(error) = provider.install(&bytes) {
            warn!("Signed updater installation failed: {error}");
            let failure = UpdateFailure::Install;
            sink.show(&UpdatePhase::Failed { failure });
            return InstallOutcome::Stopped { failure };
        }

        sink.show(&UpdatePhase::Installed {
            version: version.to_owned(),
        });
        InstallOutcome::RestartReady { permit }
    }
}

/// Test double helpers shared by the orchestration tests.
#[cfg(test)]
mod tests_support {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    pub(crate) struct RecordingSink {
        pub(crate) phases: Mutex<Vec<UpdatePhase>>,
    }

    impl RecordingSink {
        pub(crate) fn phases(&self) -> Vec<UpdatePhase> {
            lock_or_recover(&self.phases, PROGRESS_LOCK).clone()
        }
    }

    impl MenuSink for RecordingSink {
        fn show(&self, phase: &UpdatePhase) {
            lock_or_recover(&self.phases, PROGRESS_LOCK).push(phase.clone());
        }
    }

    #[derive(Clone)]
    pub(crate) struct FakeProvider {
        pub(crate) download_result: Result<Vec<u8>, String>,
        pub(crate) install_error: Option<String>,
        pub(crate) progress: Vec<Option<u8>>,
        pub(crate) installs: Arc<AtomicUsize>,
    }

    impl FakeProvider {
        pub(crate) fn ok() -> Self {
            Self {
                download_result: Ok(vec![1, 2, 3]),
                install_error: None,
                progress: vec![Some(40), Some(40), Some(100)],
                installs: Arc::new(AtomicUsize::new(0)),
            }
        }

        pub(crate) fn download_error(message: &str) -> Self {
            Self {
                download_result: Err(message.to_owned()),
                ..Self::ok()
            }
        }

        pub(crate) fn install_error(mut self, message: &str) -> Self {
            self.install_error = Some(message.to_owned());
            self
        }
    }

    impl UpdateProvider for FakeProvider {
        fn download(&self, on_progress: ProgressSink) -> BoxFuture<'_, Result<Vec<u8>, String>> {
            let progress = self.progress.clone();
            let result = self.download_result.clone();
            Box::pin(async move {
                for percent in progress {
                    on_progress(percent);
                }
                result
            })
        }

        fn install(&self, _bytes: &[u8]) -> Result<(), String> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            match &self.install_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Script {
        Ready,
        WorkActive,
        Blocked,
        Failed,
    }

    pub(crate) struct FakeRecovery {
        pub(crate) script: Mutex<VecDeque<Script>>,
        pub(crate) prepare_error: Option<String>,
        pub(crate) preparations: Arc<AtomicUsize>,
        pub(crate) permit: usize,
    }

    impl FakeRecovery {
        pub(crate) fn always_ready() -> Self {
            Self {
                script: Mutex::new(VecDeque::new()),
                prepare_error: None,
                preparations: Arc::new(AtomicUsize::new(0)),
                permit: 7,
            }
        }

        pub(crate) fn scripted(steps: impl IntoIterator<Item = Script>) -> Self {
            Self {
                script: Mutex::new(steps.into_iter().collect()),
                ..Self::always_ready()
            }
        }

        pub(crate) fn prepare_error(mut self, message: &str) -> Self {
            self.prepare_error = Some(message.to_owned());
            self
        }
    }

    impl Recovery for FakeRecovery {
        type Permit = usize;

        fn try_begin(&self) -> ActivityAttempt<Self::Permit> {
            let next = lock_or_recover(&self.script, PROGRESS_LOCK)
                .pop_front()
                .unwrap_or(Script::Ready);
            match next {
                Script::Ready => ActivityAttempt::Ready(self.permit),
                Script::WorkActive => ActivityAttempt::WorkActive,
                Script::Blocked => ActivityAttempt::Blocked,
                Script::Failed => ActivityAttempt::Failed("activity rejected".to_owned()),
            }
        }

        fn prepare<'a>(&'a self, _permit: &Self::Permit) -> BoxFuture<'a, Result<(), String>> {
            self.preparations.fetch_add(1, Ordering::SeqCst);
            let result = self.prepare_error.clone().map_or_else(|| Ok(()), Err);
            Box::pin(std::future::ready(result))
        }
    }
}
