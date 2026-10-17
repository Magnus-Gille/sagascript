use super::*;

#[test]
fn a_failed_change_restores_what_was_registered() {
    let registered = OperationalHotkey::Registered(vec!["Control+Shift+Space".into()]);
    assert_eq!(
        rollback_for(&registered),
        Rollback::Restore(&["Control+Shift+Space".to_string()])
    );
    assert_eq!(
        rollback_for(&OperationalHotkey::Inactive),
        Rollback::NothingActive
    );
}

/// The unknown state is refused before a change touches the OS, so a rollback
/// does not start from it. If it ever did, the rollback restores nothing
/// rather than guessing, and the health record keeps saying unknown.
#[test]
fn an_unknown_previous_state_restores_nothing() {
    assert_eq!(
        rollback_for(&OperationalHotkey::Unknown),
        Rollback::StateUnknown
    );
}

#[test]
fn the_monitor_reinstall_answers_decide_whether_registration_goes_on() {
    use crate::commands::main_thread::MainThreadReply::{Disconnected, Done, TimedOut};

    assert!(after_monitor_install(Done(Ok(()))).is_ok());
    // A failed install is logged; registration then reports the monitor.
    assert!(after_monitor_install(Done(Err("no event loop".into()))).is_ok());
    // The main thread is gone (shutdown, or the closure panicked): nothing to
    // wait for.
    assert!(after_monitor_install(Disconnected).is_ok());

    // The install is still queued. Registering now would say "restart
    // Sagascript" about a monitor that is about to exist: ask for another try.
    let error = after_monitor_install(TimedOut).expect_err("a timeout ends the call");
    assert_eq!(error.code(), "busy");
    assert!(error.retryable());
    assert!(error.to_string().contains("try again"), "{error}");
    assert!(!error.to_string().contains("restart"), "{error}");
}
