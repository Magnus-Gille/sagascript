use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

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
    pub fn begin(&self) -> (String, oneshot::Receiver<Result<(), String>>) {
        let nonce = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        *self.pending.lock().unwrap() = Some(PendingPreparation {
            nonce: nonce.clone(),
            sender,
        });
        (nonce, receiver)
    }

    pub fn complete(&self, nonce: &str, result: Result<(), String>) -> Result<(), String> {
        let mut pending = self.pending.lock().unwrap();
        if pending
            .as_ref()
            .is_none_or(|current| current.nonce != nonce)
        {
            return Err("Unknown or expired update preparation request.".into());
        }
        if let Some(current) = pending.take() {
            let _ = current.sender.send(result);
        }
        Ok(())
    }

    pub fn cancel(&self, nonce: &str) {
        let mut pending = self.pending.lock().unwrap();
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

pub struct ExclusiveLease(Arc<UpdateActivity>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveError {
    WorkActive,
    ResultsPending,
    AlreadyExclusive,
}

impl UpdateActivity {
    pub fn begin_work(self: &Arc<Self>) -> Result<WorkLease, String> {
        let mut state = self.state.lock().unwrap();
        if state.exclusive {
            return Err("An update is being applied. Wait for Sagascript to restart.".into());
        }
        state.active_work = state.active_work.saturating_add(1);
        Ok(WorkLease(self.clone()))
    }

    pub fn set_result_pending(&self, result_id: &str, pending: bool) -> Result<(), String> {
        if result_id.is_empty() || result_id.len() > 128 {
            return Err("Invalid transcription result ID.".into());
        }
        let mut state = self.state.lock().unwrap();
        if pending && state.exclusive {
            return Err("An update is being applied. Wait for Sagascript to restart.".into());
        }
        if pending {
            state.unsaved_results.insert(result_id.to_owned());
        } else {
            state.unsaved_results.remove(result_id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn active_work_count(&self) -> usize {
        self.state.lock().unwrap().active_work
    }

    #[cfg(test)]
    pub fn pending_result_count(&self) -> usize {
        self.state.lock().unwrap().unsaved_results.len()
    }

    pub fn try_exclusive(
        self: &Arc<Self>,
        require_no_pending_results: bool,
    ) -> Result<ExclusiveLease, ExclusiveError> {
        let mut state = self.state.lock().unwrap();
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
        let mut state = self.0.state.lock().unwrap();
        state.active_work = state.active_work.saturating_sub(1);
    }
}

impl Drop for ExclusiveLease {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().exclusive = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_accepts_only_the_current_nonce() {
        let preparation = UpdatePreparation::default();
        let (nonce, receiver) = preparation.begin();
        assert!(preparation.complete("stale", Ok(())).is_err());
        preparation.complete(&nonce, Ok(())).unwrap();
        assert!(receiver.blocking_recv().unwrap().is_ok());
        assert!(preparation.complete(&nonce, Ok(())).is_err());
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

    #[test]
    fn exclusive_update_does_not_accept_new_pending_results() {
        let activity = Arc::new(UpdateActivity::default());
        let _exclusive = activity.try_exclusive(true).unwrap();

        assert!(activity.set_result_pending("late-result", true).is_err());
        assert_eq!(activity.pending_result_count(), 0);
    }
}
