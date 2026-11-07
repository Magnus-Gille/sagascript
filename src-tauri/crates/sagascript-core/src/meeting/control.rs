//! Progress and cancellation of a meeting job (#343, #374): the control a caller who owns the job
//! hands to the pipeline and to reprocessing, and the phases they report.
//!
//! Cancellation is cooperative: the caller raises the flag and every long step looks at it at
//! its checkpoints, so a cancelled job stops at the next one and releases what it holds.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::error::DictationError;
use crate::sync::lock_or_recover;

/// A step of a meeting job, in the order a run reaches them. The wire name (`snake_case`) is what
/// the app puts in a job snapshot and its frontend labels.
#[derive(Clone, Copy, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPhase {
    Preparing,
    Decoding,
    LoadingModel,
    Analyzing,
    Clustering,
    Finalizing,
}

/// What a caller who owns a meeting job hands the run: the flag that cancels it and where it
/// reports its phase. The caller owns the flag and must not clear it during a job.
pub struct MeetingControl<'a> {
    pub cancellation: &'a AtomicBool,
    pub progress: &'a (dyn Fn(MeetingPhase) + Sync),
}

impl MeetingControl<'_> {
    /// `Err(Cancelled)` once the caller raised the flag.
    pub fn check(&self) -> Result<(), DictationError> {
        if self.cancellation.load(Ordering::Acquire) {
            return Err(DictationError::Cancelled);
        }
        Ok(())
    }
}

/// Check for cancellation, report `phase`, check again. Nothing happens without a control.
pub fn meeting_checkpoint(
    control: Option<&MeetingControl<'_>>,
    phase: MeetingPhase,
) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
        (control.progress)(phase);
        control.check()?;
    }
    Ok(())
}

/// Check for cancellation; nothing happens without a control.
pub fn meeting_check(control: Option<&MeetingControl<'_>>) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
}

// Poisoned locks here are recovered (ADR 0005): each critical section is one
// counter bump or `Vec` push/retain/iteration.
const ABORTERS_LOCK: &str = "cancel.aborters";
const NEXT_ID_LOCK: &str = "cancel.next_id";

type Aborter = Box<dyn Fn() + Send>;

/// A cancellation flag plus the aborters to fire when it is set. The process
/// uses one global instance; tests build their own.
pub struct Canceller {
    flag: AtomicBool,
    aborters: Mutex<Vec<(u64, Aborter)>>,
    next_id: Mutex<u64>,
}

impl Canceller {
    pub const fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            aborters: Mutex::new(Vec::new()),
            next_id: Mutex::new(0),
        }
    }

    pub fn flag(&self) -> &AtomicBool {
        &self.flag
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Err when cancellation was requested.
    pub fn check(&self) -> Result<(), DictationError> {
        if self.is_cancelled() {
            Err(DictationError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Set the flag and fire every registered aborter. Returns whether this
    /// was the first request.
    pub fn request(&self) -> bool {
        let first = !self.flag.swap(true, Ordering::SeqCst);
        self.fire();
        first
    }

    fn fire(&self) {
        // Recover a poisoned list (ADR 0005): skipping it would silently stop
        // Ctrl-C from aborting inference.
        for (_, abort) in lock_or_recover(&self.aborters, ABORTERS_LOCK).iter() {
            abort();
        }
    }

    /// Register `abort` to run on cancellation. If cancellation already
    /// happened it fires immediately, so there is no registration race.
    pub fn register<'a>(&'a self, abort: impl Fn() + Send + 'static) -> AborterGuard<'a> {
        let id = {
            let mut next = lock_or_recover(&self.next_id, NEXT_ID_LOCK);
            *next += 1;
            *next
        };
        lock_or_recover(&self.aborters, ABORTERS_LOCK).push((id, Box::new(abort)));
        if self.is_cancelled() {
            self.fire();
        }
        AborterGuard { owner: self, id }
    }
}

impl Default for Canceller {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps an aborter registered until dropped.
pub struct AborterGuard<'a> {
    owner: &'a Canceller,
    id: u64,
}

impl Drop for AborterGuard<'_> {
    fn drop(&mut self) {
        lock_or_recover(&self.owner.aborters, ABORTERS_LOCK).retain(|(id, _)| *id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn request_fires_registered_aborters_and_sets_the_flag() {
        let canceller = Canceller::new();
        let hits = Arc::new(AtomicBool::new(false));
        let seen = hits.clone();
        let _guard = canceller.register(move || seen.store(true, Ordering::SeqCst));
        assert!(canceller.check().is_ok());
        assert!(canceller.request());
        assert!(!canceller.request(), "second request is reported as repeat");
        assert!(hits.load(Ordering::SeqCst));
        assert!(matches!(
            canceller.check().unwrap_err(),
            DictationError::Cancelled
        ));
    }

    #[test]
    fn dropped_guard_is_not_fired_and_late_registration_fires_at_once() {
        let canceller = Canceller::new();
        let hits = Arc::new(AtomicBool::new(false));
        let seen = hits.clone();
        drop(canceller.register(move || seen.store(true, Ordering::SeqCst)));
        canceller.request();
        assert!(!hits.load(Ordering::SeqCst));
        let late = Arc::new(AtomicBool::new(false));
        let seen = late.clone();
        let _guard = canceller.register(move || seen.store(true, Ordering::SeqCst));
        assert!(
            late.load(Ordering::SeqCst),
            "cancel before registration must still abort"
        );
    }

    #[test]
    fn meeting_control_emits_ordered_phases_and_never_clears_cancellation() {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};

        let cancellation = AtomicBool::new(false);
        let phases = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&phases);
        let progress = move |phase| observed.lock().expect("phase lock").push(phase);
        let control = MeetingControl {
            cancellation: &cancellation,
            progress: &progress,
        };
        let expected = [
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ];
        for phase in expected {
            meeting_checkpoint(Some(&control), phase).expect("checkpoint is live");
        }
        assert_eq!(*phases.lock().expect("phase lock"), expected);
        assert_eq!(
            serde_json::to_string(&MeetingPhase::LoadingModel).expect("phase serializes"),
            "\"loading_model\""
        );

        cancellation.store(true, Ordering::Release);
        assert!(meeting_checkpoint(Some(&control), MeetingPhase::Finalizing).is_err());
        assert!(cancellation.load(Ordering::Acquire));
    }
}
