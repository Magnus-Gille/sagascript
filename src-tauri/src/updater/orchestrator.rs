//! The update orchestrator: one state machine behind seams, so the check →
//! download → wait → prepare → install sequence can be driven by fakes in
//! tests on every platform (issue #376).
//!
//! The orchestrator itself has no Tauri types: tray presentation goes through
//! [`MenuSink`], the signed macOS download/install through [`UpdateProvider`]
//! and [`Recovery`], and the release manifest through [`CheckSource`]. The
//! Tauri-backed implementations live in `mod.rs` (tray, manifest) and
//! `signed.rs` (signed install).

use std::sync::Arc;

use tracing::warn;

/// A future ready to poll, boxed for trait-object returns.
pub(super) type BoxFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Reports download progress to the menu sink. The value is a percentage in
/// `0..=100`, or `None` when the total size is unknown.
#[cfg(any(target_os = "macos", test))]
pub(super) type ProgressSink = Arc<dyn Fn(Option<u8>) + Send + Sync>;

/// Sentinel for "no percent yet / unknown", so progress dedup needs no
/// `Mutex<Option<u8>>`. Real percentages never reach `u8::MAX` (they are
/// capped at 100).
#[cfg(any(target_os = "macos", test))]
pub(super) const PROGRESS_UNKNOWN: u8 = u8::MAX;

/// Every user-visible phase of the update flow. The sink maps these to tray
/// texts and menu state; the orchestrator only emits phases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdatePhase {
    Checking,
    UpToDate,
    /// A newer release exists. `install_offered` distinguishes a signed
    /// install offer from a manifest release that only opens in the browser.
    Available {
        version: String,
        install_offered: bool,
    },
    Failed {
        failure: UpdateFailure,
    },
    #[cfg(any(target_os = "macos", test))]
    Downloading {
        version: String,
        percent: Option<u8>,
    },
    #[cfg(any(target_os = "macos", test))]
    WaitingForWork,
    #[cfg(any(target_os = "macos", test))]
    PreparingDrafts,
    #[cfg(any(target_os = "macos", test))]
    Installing {
        version: String,
    },
    #[cfg(any(target_os = "macos", test))]
    Installed {
        version: String,
    },
}

/// Why an update flow stopped, mapped by the sink to a user-visible failure
/// status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpdateFailure {
    Check,
    #[cfg(any(target_os = "macos", test))]
    Download,
    #[cfg(any(target_os = "macos", test))]
    WaitTimedOut,
    #[cfg(any(target_os = "macos", test))]
    PreparationBlocked,
    #[cfg(any(target_os = "macos", test))]
    PreparationFailed,
    #[cfg(any(target_os = "macos", test))]
    RecoveryConflict,
    #[cfg(any(target_os = "macos", test))]
    Install,
}

impl std::fmt::Display for UpdateFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Check => f.write_str("check failed"),
            #[cfg(any(target_os = "macos", test))]
            Self::Download => f.write_str("download failed"),
            #[cfg(any(target_os = "macos", test))]
            Self::WaitTimedOut => f.write_str("wait timed out"),
            #[cfg(any(target_os = "macos", test))]
            Self::PreparationBlocked => f.write_str("preparation blocked"),
            #[cfg(any(target_os = "macos", test))]
            Self::PreparationFailed => f.write_str("preparation failed"),
            #[cfg(any(target_os = "macos", test))]
            Self::RecoveryConflict => f.write_str("recovery drafts could not be saved"),
            #[cfg(any(target_os = "macos", test))]
            Self::Install => f.write_str("installation failed"),
        }
    }
}

/// Presentation seam: turns phases into whatever surface owns the UI (the
/// tray menu on desktop).
pub(super) trait MenuSink: Send + Sync + 'static {
    fn show(&self, phase: &UpdatePhase);
}

/// A release found by a check, with the version the user will see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CheckedRelease {
    pub version: String,
    pub install_offered: bool,
}

/// Check seam: how the orchestrator learns about newer releases.
pub(super) trait CheckSource: Send + Sync {
    fn check(&self) -> BoxFuture<'_, Result<Option<CheckedRelease>, String>>;
}

/// Resets the "checking" menu state when the update task ends — including by
/// panic — so a crashed check can never leave the menu stuck (issue #376).
pub(super) struct CheckingGuard {
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl CheckingGuard {
    pub fn new(on_drop: impl FnOnce() + Send + 'static) -> Self {
        Self {
            on_drop: Some(Box::new(on_drop)),
        }
    }
}

impl Drop for CheckingGuard {
    fn drop(&mut self) {
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
        }
    }
}

/// The update state machine. Generic over the sink so tests can drive the
/// whole sequence without a Tauri app.
pub(super) struct Orchestrator<S> {
    sink: Arc<S>,
}

impl<S: MenuSink> Orchestrator<S> {
    pub fn new(sink: Arc<S>) -> Self {
        Self { sink }
    }

    /// Shows Checking, then the source's terminal phase (Available, UpToDate
    /// or Failed{Check}). Warns on a failed check, like the old inline flow.
    pub async fn check(&self, source: &impl CheckSource) -> Result<Option<CheckedRelease>, String> {
        self.sink.show(&UpdatePhase::Checking);
        match source.check().await {
            Ok(None) => {
                self.sink.show(&UpdatePhase::UpToDate);
                Ok(None)
            }
            Ok(Some(checked)) => {
                self.sink.show(&UpdatePhase::Available {
                    version: checked.version.clone(),
                    install_offered: checked.install_offered,
                });
                Ok(Some(checked))
            }
            Err(error) => {
                warn!("Update check failed: {error}");
                self.sink.show(&UpdatePhase::Failed {
                    failure: UpdateFailure::Check,
                });
                Err(error)
            }
        }
    }
}

/// Download seam: fetches and verifies an update bundle.
#[cfg(any(target_os = "macos", test))]
pub(super) trait UpdateProvider: Send + Sync + 'static {
    fn download(&self, on_progress: ProgressSink) -> BoxFuture<'_, Result<Vec<u8>, String>>;
    fn install(&self, bytes: &[u8]) -> Result<(), String>;
}

/// Coordination seam: when the app is quiet enough to replace itself.
#[cfg(any(target_os = "macos", test))]
pub(super) trait Recovery: Send + Sync + 'static {
    type Permit: Send;

    fn try_begin(&self) -> ActivityAttempt<Self::Permit>;
    fn prepare<'a>(&'a self, permit: &Self::Permit) -> BoxFuture<'a, Result<(), String>>;
}

/// Result of one attempt to enter the update-exclusive window.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ActivityAttempt<Permit> {
    Ready(Permit),
    WorkActive,
    Blocked,
    Failed(String),
}

/// Poll/timeout policy for waiting on active dictation/transcription work.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WaitPolicy {
    pub poll_interval: std::time::Duration,
    pub timeout: std::time::Duration,
}

#[cfg(any(target_os = "macos", test))]
impl WaitPolicy {
    /// The production policy: poll every 250 ms, give up after ten minutes,
    /// matching the wait loop this replaces.
    pub const PRODUCTION: Self = Self {
        poll_interval: std::time::Duration::from_millis(250),
        timeout: std::time::Duration::from_secs(600),
    };

    fn expired(&self, elapsed: std::time::Duration) -> bool {
        elapsed >= self.timeout
    }
}

/// Terminal result of an install attempt. `RestartReady` hands the permit to
/// the caller, which keeps it across `app.restart()`.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug)]
pub(super) enum InstallOutcome<Permit> {
    RestartReady { permit: Permit },
    Stopped { failure: UpdateFailure },
}

#[cfg(any(target_os = "macos", test))]
impl<S: MenuSink> Orchestrator<S> {
    /// The production install path, with the production wait policy.
    pub async fn install<P, R>(
        &self,
        provider: P,
        recovery: R,
        version: &str,
    ) -> InstallOutcome<R::Permit>
    where
        P: UpdateProvider,
        R: Recovery,
    {
        self.install_with_wait(provider, recovery, version, WaitPolicy::PRODUCTION)
            .await
    }

    async fn install_with_wait<P, R>(
        &self,
        provider: P,
        recovery: R,
        version: &str,
        wait: WaitPolicy,
    ) -> InstallOutcome<R::Permit>
    where
        P: UpdateProvider,
        R: Recovery,
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        use std::time::Instant;

        self.sink.show(&UpdatePhase::Downloading {
            version: version.to_owned(),
            percent: None,
        });
        let sink = Arc::clone(&self.sink);
        let last_percent = Arc::new(AtomicU8::new(PROGRESS_UNKNOWN));
        let phase_version = version.to_owned();
        let on_progress: ProgressSink = Arc::new(move |percent| {
            let percent = percent.filter(|percent| *percent != PROGRESS_UNKNOWN);
            let byte = percent.unwrap_or(PROGRESS_UNKNOWN);
            if last_percent.swap(byte, Ordering::Relaxed) != byte {
                sink.show(&UpdatePhase::Downloading {
                    version: phase_version.clone(),
                    percent,
                });
            }
        });
        let bytes = match provider.download(on_progress).await {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!("Signed updater download/verification failed: {error}");
                self.sink.show(&UpdatePhase::Failed {
                    failure: UpdateFailure::Download,
                });
                return InstallOutcome::Stopped {
                    failure: UpdateFailure::Download,
                };
            }
        };

        let wait_started = Instant::now();
        let permit = loop {
            match recovery.try_begin() {
                ActivityAttempt::Ready(permit) => break permit,
                ActivityAttempt::WorkActive => {
                    if wait.expired(wait_started.elapsed()) {
                        warn!(
                            "Update wait exceeded {:?}; current work remains untouched",
                            wait.timeout
                        );
                        self.sink.show(&UpdatePhase::Failed {
                            failure: UpdateFailure::WaitTimedOut,
                        });
                        return InstallOutcome::Stopped {
                            failure: UpdateFailure::WaitTimedOut,
                        };
                    }
                    self.sink.show(&UpdatePhase::WaitingForWork);
                    tokio::time::sleep(wait.poll_interval).await;
                }
                ActivityAttempt::Blocked => {
                    warn!("Update activity unexpectedly rejected the recovery path");
                    self.sink.show(&UpdatePhase::Failed {
                        failure: UpdateFailure::PreparationBlocked,
                    });
                    return InstallOutcome::Stopped {
                        failure: UpdateFailure::PreparationBlocked,
                    };
                }
                ActivityAttempt::Failed(message) => {
                    warn!("Could not safely apply update: {message}");
                    self.sink.show(&UpdatePhase::Failed {
                        failure: UpdateFailure::PreparationFailed,
                    });
                    return InstallOutcome::Stopped {
                        failure: UpdateFailure::PreparationFailed,
                    };
                }
            }
        };

        self.sink.show(&UpdatePhase::PreparingDrafts);
        if let Err(error) = recovery.prepare(&permit).await {
            warn!("Could not safely prepare update: {error}");
            self.sink.show(&UpdatePhase::Failed {
                failure: UpdateFailure::RecoveryConflict,
            });
            return InstallOutcome::Stopped {
                failure: UpdateFailure::RecoveryConflict,
            };
        }

        self.sink.show(&UpdatePhase::Installing {
            version: version.to_owned(),
        });
        if let Err(error) = provider.install(&bytes) {
            warn!("Signed updater installation failed: {error}");
            self.sink.show(&UpdatePhase::Failed {
                failure: UpdateFailure::Install,
            });
            return InstallOutcome::Stopped {
                failure: UpdateFailure::Install,
            };
        }

        self.sink.show(&UpdatePhase::Installed {
            version: version.to_owned(),
        });
        InstallOutcome::RestartReady { permit }
    }
}

#[cfg(test)]
#[path = "orchestrator_tests.rs"]
mod tests;
