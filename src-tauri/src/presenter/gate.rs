//! Pure sequencing policy. Native target/field observations must be supplied
//! by the platform adapter, never by IPC or frontend claims. This gate alone
//! does not prove insertion or prevent a queued OS action after dispatch.

// Non-macOS builds deliberately retain drafts until they have a verified
// native field adapter. Keep the shared policy testable on every platform.
#![cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    #[default]
    InsertOnly,
    Return,
    CommandReturn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Listening,
    Transcribing,
    AwaitingInsertion,
    Inserted,
    Submitting,
    Sent,
    Cancelled,
    Draft,
    Failed,
    NoSpeech,
    SubmitUncertain,
}

pub struct Gate {
    action: Action,
    target_valid: bool,
    phase: Phase,
    insertion_dispatched: bool,
}

impl Gate {
    pub fn new(action: Action, target_known: bool) -> Self {
        Self {
            action,
            target_valid: target_known,
            phase: Phase::Listening,
            insertion_dispatched: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn finish(&mut self) -> bool {
        if self.phase != Phase::Listening {
            return false;
        }
        self.phase = Phase::Transcribing;
        true
    }

    pub fn target_changed(&mut self) {
        self.target_valid = false;
    }

    pub fn transcribed(&mut self, success: bool, nonempty: bool) {
        if self.phase != Phase::Transcribing {
            return;
        }
        self.phase = if !success {
            Phase::Failed
        } else if !nonempty {
            Phase::NoSpeech
        } else if !self.target_valid {
            Phase::Draft
        } else {
            Phase::AwaitingInsertion
        };
    }

    pub fn may_insert(&mut self, current_target_matches: bool) -> bool {
        if self.phase != Phase::AwaitingInsertion || self.insertion_dispatched {
            return false;
        }
        if !self.target_valid || !current_target_matches {
            self.target_valid = false;
            self.phase = Phase::Draft;
            return false;
        }
        self.insertion_dispatched = true;
        true
    }

    /// Consume the one possible submit opportunity after native field-state
    /// verification. The caller must keep this decision and key dispatch in
    /// the same serialized native action and recheck target immediately there.
    pub fn authorize_submit(
        &mut self,
        insertion_proven: bool,
        current_target_matches: bool,
        app_rule_still_allows_action: bool,
    ) -> Option<Action> {
        if self.phase != Phase::AwaitingInsertion || !self.insertion_dispatched {
            return None;
        }
        if !self.target_valid || !current_target_matches || !insertion_proven {
            self.phase = Phase::Draft;
            return None;
        }
        if self.action == Action::InsertOnly || !app_rule_still_allows_action {
            self.phase = Phase::Inserted;
            return None;
        }
        self.phase = Phase::Submitting;
        Some(self.action)
    }

    pub fn submit_completed(&mut self, success: bool) {
        if self.phase == Phase::Submitting {
            // A failed native call may already have injected part of a key
            // sequence. Do not retry or claim the prompt was definitely unsent.
            self.phase = if success {
                Phase::Sent
            } else {
                Phase::SubmitUncertain
            };
        }
    }

    pub fn insertion_timed_out(&mut self) {
        if self.phase == Phase::AwaitingInsertion {
            self.phase = Phase::Draft;
        }
    }

    pub fn cancel(&mut self) {
        if matches!(
            self.phase,
            Phase::Listening | Phase::Transcribing | Phase::AwaitingInsertion
        ) {
            self.phase = Phase::Cancelled;
            self.target_valid = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Gate, Phase};

    #[test]
    fn insertion_dispatch_is_consumed_once_and_required_before_submit() {
        let mut gate = Gate::new(Action::Return, true);
        gate.finish();
        gate.transcribed(true, true);
        assert_eq!(gate.authorize_submit(true, true, true), None);
        assert!(gate.may_insert(true));
        assert!(!gate.may_insert(true));
        assert_eq!(gate.authorize_submit(true, true, true), Some(Action::Return));
    }

    #[test]
    fn submit_requires_each_observation_in_order_and_is_consumed_once() {
        let mut gate = Gate::new(Action::Return, true);
        assert_eq!(gate.authorize_submit(true, true, true), None);
        assert!(gate.finish());
        assert!(!gate.finish());
        assert_eq!(gate.authorize_submit(true, true, true), None);
        gate.transcribed(true, true);
        assert!(gate.may_insert(true));
        assert_eq!(
            gate.authorize_submit(true, true, true),
            Some(Action::Return)
        );
        assert_eq!(gate.phase(), Phase::Submitting);
        assert_eq!(gate.authorize_submit(true, true, true), None);
        gate.submit_completed(true);
        assert_eq!(gate.phase(), Phase::Sent);
        gate.submit_completed(false);
        assert_eq!(gate.phase(), Phase::Sent);
    }

    #[test]
    fn default_inserts_without_submit() {
        let mut gate = Gate::new(Action::InsertOnly, true);
        gate.finish();
        gate.transcribed(true, true);
        assert!(gate.may_insert(true));
        assert_eq!(gate.authorize_submit(true, true, true), None);
        assert_eq!(gate.phase(), Phase::Inserted);
    }

    #[test]
    fn missing_or_changed_target_never_inserts_or_submits() {
        for initially_known in [false, true] {
            let mut gate = Gate::new(Action::CommandReturn, initially_known);
            gate.target_changed(); // Changing away and back must stay invalid.
            gate.finish();
            gate.transcribed(true, true);
            assert!(!gate.may_insert(true));
            assert_eq!(gate.authorize_submit(true, true, true), None);
            assert_eq!(gate.phase(), Phase::Draft);
        }
    }

    #[test]
    fn empty_failed_or_unproven_insertion_cannot_submit() {
        for (success, nonempty) in [(false, false), (false, true), (true, false)] {
            let mut gate = Gate::new(Action::Return, true);
            gate.finish();
            gate.transcribed(success, nonempty);
            assert!(!gate.may_insert(true));
            assert_eq!(gate.authorize_submit(true, true, true), None);
        }
        for (proven, same, still_allowed) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let mut gate = Gate::new(Action::Return, true);
            gate.finish();
            gate.transcribed(true, true);
            assert!(gate.may_insert(true));
            assert_eq!(gate.authorize_submit(proven, same, still_allowed), None);
            assert_ne!(gate.phase(), Phase::Sent);
            assert_eq!(gate.authorize_submit(true, true, true), None);
        }
    }

    #[test]
    fn cancellation_and_timeout_are_terminal_for_late_callbacks() {
        for steps in 0..3 {
            let mut gate = Gate::new(Action::Return, true);
            if steps >= 1 {
                gate.finish();
            }
            if steps >= 2 {
                gate.transcribed(true, true);
            }
            gate.cancel();
            gate.transcribed(true, true);
            assert!(!gate.finish());
            assert!(!gate.may_insert(true));
            assert_eq!(gate.authorize_submit(true, true, true), None);
            gate.submit_completed(true);
            assert_eq!(gate.phase(), Phase::Cancelled);
        }
        let mut gate = Gate::new(Action::Return, true);
        gate.finish();
        gate.transcribed(true, true);
        gate.insertion_timed_out();
        assert_eq!(gate.authorize_submit(true, true, true), None);
        assert_eq!(gate.phase(), Phase::Draft);
    }

    #[test]
    fn failed_submit_is_not_retried_and_cancellation_cannot_claim_recall() {
        for succeeded in [false, true] {
            let mut gate = Gate::new(Action::Return, true);
            gate.finish();
            gate.transcribed(true, true);
            assert!(gate.may_insert(true));
            assert_eq!(
                gate.authorize_submit(true, true, true),
                Some(Action::Return)
            );
            gate.cancel(); // Native key dispatch may already be in flight.
            assert_eq!(gate.phase(), Phase::Submitting);
            gate.submit_completed(succeeded);
            assert_eq!(
                gate.phase(),
                if succeeded {
                    Phase::Sent
                } else {
                    Phase::SubmitUncertain
                }
            );
            assert_eq!(gate.authorize_submit(true, true, true), None);
        }
    }
}
