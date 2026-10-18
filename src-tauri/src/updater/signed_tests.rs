use super::*;

#[test]
fn update_wait_times_out_without_stopping_active_work() {
    let start = Instant::now();
    assert!(!update_work_wait_expired(
        start,
        start + Duration::from_secs(599)
    ));
    assert!(update_work_wait_expired(
        start,
        start + Duration::from_secs(600)
    ));
}

#[test]
fn pending_native_dictation_cannot_replace_a_different_recovered_draft() {
    let old = serde_json::json!({ "dictation": { "text": "Older unsaved draft" } });
    assert!(native_recovery_conflicts(&old, "New unsaved dictation"));
    assert!(!native_recovery_conflicts(&old, "Older unsaved draft"));
    assert!(!native_recovery_conflicts(
        &serde_json::json!({ "dictation": null }),
        "New unsaved dictation"
    ));
}
