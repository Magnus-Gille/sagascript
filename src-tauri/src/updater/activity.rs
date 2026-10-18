use sagascript_core::sync::lock_or_recover;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

// Recovered as-is after a panic (ADR 0005): each critical section is one
// counter/flag/set update or `Option` swap, so the data stays valid. The lease
// `Drop`s run while unwinding, where a second panic would abort the app.
const PREPARATION_LOCK: &str = "update.preparation";
const ACTIVITY_LOCK: &str = "update.activity";

/// Why the update coordinator refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ActivityError {
    /// An update is being installed: no new work, no new pending result.
    #[error("An update is being applied. Wait for Sagascript to restart.")]
    UpdateApplying,
    /// A result id that is empty or longer than 128 bytes.
    #[error("Invalid transcription result ID.")]
    InvalidResultId,
    /// An acknowledgement whose nonce is not the pending preparation's.
    #[error("Unknown or expired update preparation request.")]
    UnknownPreparation,
}

struct PendingPreparation {
    nonce: String,
    sender: oneshot::Sender<Result<(), String>>,
}

/// A one-shot acknowledgement from the Settings webview after its results have
/// been saved to the local recovery file. An update never restarts on a missing
/// or failed acknowledgement.
#[derive(Default)]
pub struct UpdatePreparation {
    pending: Mutex<Option<PendingPreparation>>,
}

impl UpdatePreparation {
    #[cfg(any(target_os = "macos", test))]
    pub fn begin(&self) -> (String, oneshot::Receiver<Result<(), String>>) {
        let nonce = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        *lock_or_recover(&self.pending, PREPARATION_LOCK) = Some(PendingPreparation {
            nonce: nonce.clone(),
            sender,
        });
        (nonce, receiver)
    }

    pub fn complete(&self, nonce: &str, result: Result<(), String>) -> Result<(), ActivityError> {
        let mut pending = lock_or_recover(&self.pending, PREPARATION_LOCK);
        if pending
            .as_ref()
            .is_none_or(|current| current.nonce != nonce)
        {
            return Err(ActivityError::UnknownPreparation);
        }
        if let Some(current) = pending.take() {
            let _ = current.sender.send(result);
        }
        Ok(())
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn cancel(&self, nonce: &str) {
        let mut pending = lock_or_recover(&self.pending, PREPARATION_LOCK);
        if pending
            .as_ref()
            .is_some_and(|current| current.nonce == nonce)
        {
            pending.take();
        }
    }
}

#[derive(Default)]
struct ActivityState {
    active_work: usize,
    exclusive: bool,
    unsaved_results: HashSet<String>,
}

/// Coordinates update installation with work that must finish before the app
/// can safely be replaced or restarted.
#[derive(Default)]
pub struct UpdateActivity {
    state: Mutex<ActivityState>,
}

pub struct WorkLease(Arc<UpdateActivity>);

#[cfg(any(target_os = "macos", test))]
pub struct ExclusiveLease(Arc<UpdateActivity>);

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveError {
    WorkActive,
    ResultsPending,
    AlreadyExclusive,
}

impl UpdateActivity {
    pub fn begin_work(self: &Arc<Self>) -> Result<WorkLease, ActivityError> {
        let mut state = lock_or_recover(&self.state, ACTIVITY_LOCK);
        if state.exclusive {
            return Err(ActivityError::UpdateApplying);
        }
        state.active_work = state.active_work.saturating_add(1);
        Ok(WorkLease(self.clone()))
    }

    pub fn set_result_pending(&self, result_id: &str, pending: bool) -> Result<(), ActivityError> {
        if result_id.is_empty() || result_id.len() > 128 {
            return Err(ActivityError::InvalidResultId);
        }
        let mut state = lock_or_recover(&self.state, ACTIVITY_LOCK);
        if pending && state.exclusive {
            return Err(ActivityError::UpdateApplying);
        }
        if pending {
            state.unsaved_results.insert(result_id.to_owned());
        } else {
            state.unsaved_results.remove(result_id);
        }
        Ok(())
    }

    pub fn is_result_pending(&self, result_id: &str) -> bool {
        lock_or_recover(&self.state, ACTIVITY_LOCK)
            .unsaved_results
            .contains(result_id)
    }

    #[cfg(test)]
    pub fn active_work_count(&self) -> usize {
        lock_or_recover(&self.state, ACTIVITY_LOCK).active_work
    }

    #[cfg(test)]
    pub fn pending_result_count(&self) -> usize {
        lock_or_recover(&self.state, ACTIVITY_LOCK)
            .unsaved_results
            .len()
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn try_exclusive(
        self: &Arc<Self>,
        require_no_pending_results: bool,
    ) -> Result<ExclusiveLease, ExclusiveError> {
        let mut state = lock_or_recover(&self.state, ACTIVITY_LOCK);
        if state.exclusive {
            return Err(ExclusiveError::AlreadyExclusive);
        }
        if state.active_work > 0 {
            return Err(ExclusiveError::WorkActive);
        }
        if require_no_pending_results && !state.unsaved_results.is_empty() {
            return Err(ExclusiveError::ResultsPending);
        }
        state.exclusive = true;
        Ok(ExclusiveLease(self.clone()))
    }
}

impl Drop for WorkLease {
    fn drop(&mut self) {
        let mut state = lock_or_recover(&self.0.state, ACTIVITY_LOCK);
        state.active_work = state.active_work.saturating_sub(1);
    }
}

#[cfg(any(target_os = "macos", test))]
impl Drop for ExclusiveLease {
    fn drop(&mut self) {
        lock_or_recover(&self.0.state, ACTIVITY_LOCK).exclusive = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successfully_delivered_result_can_leave_memory_without_remaining_a_recovery_draft() {
        let activity = UpdateActivity::default();
        assert!(!activity.is_result_pending("live-dictation"));
        activity.set_result_pending("live-dictation", true).unwrap();
        assert!(activity.is_result_pending("live-dictation"));
        activity
            .set_result_pending("live-dictation", false)
            .unwrap();
        assert!(!activity.is_result_pending("live-dictation"));
    }

    #[test]
    fn preparation_accepts_only_the_current_nonce() {
        let preparation = UpdatePreparation::default();
        let (nonce, receiver) = preparation.begin();
        assert!(preparation.complete("stale", Ok(())).is_err());
        preparation.complete(&nonce, Ok(())).unwrap();
        assert!(receiver.blocking_recv().unwrap().is_ok());
        assert!(preparation.complete(&nonce, Ok(())).is_err());
    }

    /// A refusal says why, and the command layer maps it to a code of its own:
    /// an update being applied is `busy` (repeat the action once Sagascript has
    /// restarted), a bad id is `invalid_input`, an unknown nonce `not_found`.
    #[test]
    fn refusals_are_typed_and_become_command_errors_with_codes() {
        use crate::command_error::CommandError;

        let activity = Arc::new(UpdateActivity::default());
        assert_eq!(
            activity.set_result_pending("", true),
            Err(ActivityError::InvalidResultId)
        );
        assert_eq!(
            activity.set_result_pending(&"x".repeat(129), true),
            Err(ActivityError::InvalidResultId)
        );
        let exclusive = activity.try_exclusive(false).unwrap();
        assert_eq!(
            activity.begin_work().err(),
            Some(ActivityError::UpdateApplying)
        );
        assert_eq!(
            activity.set_result_pending("result", true),
            Err(ActivityError::UpdateApplying)
        );
        drop(exclusive);
        let preparation = UpdatePreparation::default();
        assert_eq!(
            preparation.complete("nonce", Ok(())),
            Err(ActivityError::UnknownPreparation)
        );

        for (error, code, retryable, text) in [
            (
                ActivityError::UpdateApplying,
                "busy",
                true,
                "An update is being applied. Wait for Sagascript to restart.",
            ),
            (
                ActivityError::InvalidResultId,
                "invalid_input",
                false,
                "Invalid transcription result ID.",
            ),
            (
                ActivityError::UnknownPreparation,
                "not_found",
                false,
                "Unknown or expired update preparation request.",
            ),
        ] {
            let error = CommandError::from(error);
            assert_eq!(error.code(), code);
            assert_eq!(error.retryable(), retryable, "{code}");
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn cancelled_preparation_cannot_acknowledge_a_later_update() {
        let preparation = UpdatePreparation::default();
        let (old_nonce, old_receiver) = preparation.begin();
        preparation.cancel(&old_nonce);
        assert!(old_receiver.blocking_recv().is_err());
        let (new_nonce, receiver) = preparation.begin();
        assert!(preparation.complete(&old_nonce, Ok(())).is_err());
        preparation
            .complete(&new_nonce, Err("draft save failed".into()))
            .unwrap();
        assert_eq!(
            receiver.blocking_recv().unwrap().unwrap_err(),
            "draft save failed"
        );
    }

    #[test]
    fn exclusive_update_waits_for_active_work_and_blocks_new_work_atomically() {
        let activity = Arc::new(UpdateActivity::default());
        let work = activity.begin_work().unwrap();

        assert!(matches!(
            activity.try_exclusive(false),
            Err(ExclusiveError::WorkActive)
        ));
        drop(work);

        let exclusive = activity.try_exclusive(false).unwrap();
        assert!(activity.begin_work().is_err());
        drop(exclusive);
        assert!(activity.begin_work().is_ok());
    }

    #[test]
    fn restart_waits_until_each_unsaved_result_is_resolved() {
        let activity = Arc::new(UpdateActivity::default());
        activity.set_result_pending("result-a", true).unwrap();
        activity.set_result_pending("result-b", true).unwrap();

        assert!(matches!(
            activity.try_exclusive(true),
            Err(ExclusiveError::ResultsPending)
        ));
        activity.set_result_pending("result-a", false).unwrap();
        assert_eq!(activity.pending_result_count(), 1);
        activity.set_result_pending("result-b", false).unwrap();
        assert_eq!(activity.pending_result_count(), 0);
        assert!(activity.try_exclusive(true).is_ok());
    }

    #[test]
    fn result_ids_are_bounded_and_work_lease_releases_after_drop() {
        let activity = Arc::new(UpdateActivity::default());
        assert!(activity.set_result_pending("", true).is_err());
        assert!(activity.set_result_pending(&"x".repeat(129), true).is_err());
        let work = activity.begin_work().unwrap();
        assert_eq!(activity.active_work_count(), 1);
        drop(work);
        assert_eq!(activity.active_work_count(), 0);
    }

    fn poison<T: Send>(mutex: &Mutex<T>) {
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("injected panic while holding the lock (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(mutex.is_poisoned());
    }

    /// A lease is dropped while unwinding from a panic in update-guarded work.
    /// Panicking again on the poisoned state would be a double panic (abort).
    #[test]
    fn lease_drops_tolerate_a_poisoned_activity_state() {
        let activity = Arc::new(UpdateActivity::default());
        let work = activity.begin_work().unwrap();
        poison(&activity.state);

        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(work)));
        assert!(dropped.is_ok(), "WorkLease::drop must not panic");
        assert_eq!(activity.active_work_count(), 0);

        let exclusive = activity.try_exclusive(false).unwrap();
        poison(&activity.state);
        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(exclusive)));
        assert!(dropped.is_ok(), "ExclusiveLease::drop must not panic");
        assert!(activity.begin_work().is_ok(), "exclusivity was released");
    }

    #[test]
    fn exclusive_update_does_not_accept_new_pending_results() {
        let activity = Arc::new(UpdateActivity::default());
        let _exclusive = activity.try_exclusive(true).unwrap();

        assert!(activity.set_result_pending("late-result", true).is_err());
        assert_eq!(activity.pending_result_count(), 0);
    }
}
