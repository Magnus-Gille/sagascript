use std::collections::VecDeque;

use super::{apply, rollback, Registrar, RollbackOutcome, TransitionFailureCause};
use crate::hotkey::OperationalHotkey;

#[derive(Default)]
struct FakeRegistrar {
    calls: Vec<String>,
    register_results: VecDeque<Result<(), String>>,
    unregister_results: VecDeque<Result<(), String>>,
}

impl FakeRegistrar {
    fn with_results(
        register_results: impl IntoIterator<Item = Result<(), &'static str>>,
        unregister_results: impl IntoIterator<Item = Result<(), &'static str>>,
    ) -> Self {
        Self {
            calls: Vec::new(),
            register_results: register_results
                .into_iter()
                .map(|result| result.map_err(str::to_string))
                .collect(),
            unregister_results: unregister_results
                .into_iter()
                .map(|result| result.map_err(str::to_string))
                .collect(),
        }
    }

    fn result(queue: &mut VecDeque<Result<(), String>>) -> Result<(), String> {
        queue.pop_front().unwrap_or(Ok(()))
    }
}

impl Registrar for FakeRegistrar {
    fn register(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.calls.push(format!("register:{}", shortcuts.join(",")));
        Self::result(&mut self.register_results)
    }

    fn unregister(&mut self, shortcuts: &[String]) -> Result<(), String> {
        self.calls
            .push(format!("unregister:{}", shortcuts.join(",")));
        Self::result(&mut self.unregister_results)
    }
}

fn shortcuts(value: &str) -> Vec<String> {
    vec![value.to_string()]
}

fn registered(value: &str) -> OperationalHotkey {
    OperationalHotkey::Registered(shortcuts(value))
}

#[test]
fn registered_transition_unregisters_old_then_registers_new() {
    let mut registrar = FakeRegistrar::default();
    let result = apply(&registered("old"), &shortcuts("new"), &mut registrar);

    assert_eq!(result, Ok(registered("new")));
    assert_eq!(registrar.calls, ["unregister:old", "register:new"]);
}

#[test]
fn inactive_transition_registers_new_without_unregistering() {
    let mut registrar = FakeRegistrar::default();
    let result = apply(
        &OperationalHotkey::Inactive,
        &shortcuts("new"),
        &mut registrar,
    );

    assert_eq!(result, Ok(registered("new")));
    assert_eq!(registrar.calls, ["register:new"]);
}

#[test]
fn unknown_previous_state_refuses_before_any_os_call() {
    let mut registrar = FakeRegistrar::default();
    let error = apply(
        &OperationalHotkey::Unknown,
        &shortcuts("new"),
        &mut registrar,
    )
    .expect_err("unknown state must be refused");

    assert_eq!(error.cause, TransitionFailureCause::PreviousUnknown);
    assert_eq!(error.operational, OperationalHotkey::Unknown);
    assert!(registrar.calls.is_empty());
}

#[test]
fn old_unregister_failure_stops_before_new_registration_and_is_unknown() {
    let mut registrar = FakeRegistrar::with_results([], [Err("old unregister")]);
    let error = apply(&registered("old"), &shortcuts("new"), &mut registrar)
        .expect_err("old unregister failure must fail the transition");

    assert_eq!(
        error.cause,
        TransitionFailureCause::UnregisterOld {
            error: "old unregister".into()
        }
    );
    assert_eq!(error.operational, OperationalHotkey::Unknown);
    assert_eq!(registrar.calls, ["unregister:old"]);
}

#[test]
fn new_registration_failure_cleans_up_then_restores_old() {
    let mut registrar =
        FakeRegistrar::with_results([Err("new register"), Ok(())], [Ok(()), Ok(())]);
    let error = apply(&registered("old"), &shortcuts("new"), &mut registrar)
        .expect_err("new registration failure must be returned");

    assert_eq!(error.operational, registered("old"));
    assert_eq!(
        error.cause,
        TransitionFailureCause::RegisterNew {
            error: "new register".into(),
            rollback: RollbackOutcome::Restored(registered("old")),
        }
    );
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
}

#[test]
fn cleanup_failure_leaves_unknown_and_never_restores_old() {
    let mut registrar =
        FakeRegistrar::with_results([Err("new register")], [Ok(()), Err("partial cleanup")]);
    let error = apply(&registered("old"), &shortcuts("new"), &mut registrar)
        .expect_err("cleanup failure must fail the transition");

    assert_eq!(error.operational, OperationalHotkey::Unknown);
    assert_eq!(
        error.cause,
        TransitionFailureCause::RegisterNew {
            error: "new register".into(),
            rollback: RollbackOutcome::CleanupFailed("partial cleanup".into()),
        }
    );
    assert_eq!(
        registrar.calls,
        ["unregister:old", "register:new", "unregister:new"]
    );
}

#[test]
fn restore_failure_after_cleanup_leaves_inactive() {
    let mut registrar =
        FakeRegistrar::with_results([Err("new register"), Err("restore old")], [Ok(()), Ok(())]);
    let error = apply(&registered("old"), &shortcuts("new"), &mut registrar)
        .expect_err("restore failure must fail the transition");

    assert_eq!(error.operational, OperationalHotkey::Inactive);
    assert_eq!(
        error.cause,
        TransitionFailureCause::RegisterNew {
            error: "new register".into(),
            rollback: RollbackOutcome::RestoreFailed("restore old".into()),
        }
    );
    assert_eq!(
        registrar.calls,
        [
            "unregister:old",
            "register:new",
            "unregister:new",
            "register:old"
        ]
    );
}

#[test]
fn inactive_registration_failure_cleans_up_without_restore_attempt() {
    let mut registrar = FakeRegistrar::with_results([Err("new register")], [Ok(())]);
    let error = apply(
        &OperationalHotkey::Inactive,
        &shortcuts("new"),
        &mut registrar,
    )
    .expect_err("new registration failure must be returned");

    assert_eq!(error.operational, OperationalHotkey::Inactive);
    assert_eq!(
        error.cause,
        TransitionFailureCause::RegisterNew {
            error: "new register".into(),
            rollback: RollbackOutcome::Restored(OperationalHotkey::Inactive),
        }
    );
    assert_eq!(registrar.calls, ["register:new", "unregister:new"]);
}

#[test]
fn inactive_cleanup_failure_is_unknown() {
    let mut registrar =
        FakeRegistrar::with_results([Err("new register")], [Err("partial cleanup")]);
    let error = apply(
        &OperationalHotkey::Inactive,
        &shortcuts("new"),
        &mut registrar,
    )
    .expect_err("cleanup failure must fail the transition");

    assert_eq!(error.operational, OperationalHotkey::Unknown);
    assert_eq!(
        error.cause,
        TransitionFailureCause::RegisterNew {
            error: "new register".into(),
            rollback: RollbackOutcome::CleanupFailed("partial cleanup".into()),
        }
    );
    assert_eq!(registrar.calls, ["register:new", "unregister:new"]);
}

#[test]
fn persistence_rollback_cleans_new_then_restores_old() {
    let mut registrar = FakeRegistrar::default();
    let result = rollback(&registered("old"), &shortcuts("new"), &mut registrar);

    assert_eq!(result, RollbackOutcome::Restored(registered("old")));
    assert_eq!(registrar.calls, ["unregister:new", "register:old"]);
}

#[test]
fn persistence_rollback_cleanup_failure_does_not_restore_old() {
    let mut registrar = FakeRegistrar::with_results([], [Err("partial cleanup")]);
    let result = rollback(&registered("old"), &shortcuts("new"), &mut registrar);

    assert_eq!(
        result,
        RollbackOutcome::CleanupFailed("partial cleanup".into())
    );
    assert_eq!(result.operational(), OperationalHotkey::Unknown);
    assert_eq!(registrar.calls, ["unregister:new"]);
}

#[test]
fn persistence_rollback_restore_failure_is_inactive() {
    let mut registrar = FakeRegistrar::with_results([Err("restore old")], [Ok(())]);
    let result = rollback(&registered("old"), &shortcuts("new"), &mut registrar);

    assert_eq!(result, RollbackOutcome::RestoreFailed("restore old".into()));
    assert_eq!(result.operational(), OperationalHotkey::Inactive);
    assert_eq!(registrar.calls, ["unregister:new", "register:old"]);
}

#[test]
fn defensive_unknown_rollback_refuses_before_any_os_call() {
    let mut registrar = FakeRegistrar::default();
    let result = rollback(
        &OperationalHotkey::Unknown,
        &shortcuts("new"),
        &mut registrar,
    );

    assert_eq!(
        result,
        RollbackOutcome::Restored(OperationalHotkey::Unknown)
    );
    assert!(registrar.calls.is_empty());
}
