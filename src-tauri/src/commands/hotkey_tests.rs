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
