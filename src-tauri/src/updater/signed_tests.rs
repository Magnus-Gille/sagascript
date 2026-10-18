use super::*;

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
