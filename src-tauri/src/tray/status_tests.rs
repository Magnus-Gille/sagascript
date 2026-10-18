use super::*;

#[test]
fn tray_label_idle_not_failed() {
    assert_eq!(tray_label("idle", false), ("Sagascript", "S", "Idle"));
}

#[test]
fn tray_label_recording_not_failed() {
    assert_eq!(
        tray_label("recording", false),
        ("Sagascript - Recording...", "●", "Recording...")
    );
}

#[test]
fn tray_status_uses_compact_native_state_markers() {
    assert_eq!(tray_label("idle", false).1, "S");
    assert_eq!(tray_label("recording", false).1, "●");
    assert_eq!(tray_label("loading_model", false).1, "…");
    assert_eq!(tray_label("transcribing", false).1, "…");
    assert_eq!(tray_label("idle", true).1, "!");
}

#[test]
fn tray_label_hotkey_failed_is_distinct_from_idle() {
    let failed = tray_label("idle", true);
    assert_ne!(failed, tray_label("idle", false));
    assert_eq!(failed.2, "Hotkey unavailable");
}

#[test]
fn tray_label_hotkey_failed_wins_over_recording() {
    // The sticky warning must win over a normal state transition into
    // "recording" — a hotkey that isn't registered can't actually be
    // driving a recording state the user trusts.
    assert_eq!(tray_label("recording", true), tray_label("idle", true));
}

#[test]
fn tray_label_hotkey_failed_wins_over_transcribing() {
    assert_eq!(tray_label("transcribing", true), tray_label("idle", true));
}

#[test]
fn tray_label_hotkey_failed_wins_over_loading_model() {
    assert_eq!(tray_label("loading_model", true), tray_label("idle", true));
}

#[test]
fn truncate_for_tray_empty_string_unchanged() {
    assert_eq!(truncate_for_tray(""), "");
}

#[test]
fn truncate_for_tray_ascii_at_threshold_unchanged() {
    let text = "a".repeat(60);
    assert_eq!(truncate_for_tray(&text), text);
}

#[test]
fn truncate_for_tray_ascii_over_threshold_truncated() {
    let text = "a".repeat(61);
    let display = truncate_for_tray(&text);
    assert_eq!(display, format!("{}...", "a".repeat(57)));
}

#[test]
fn truncate_for_tray_multibyte_straddle_does_not_panic() {
    // "å" is 2 bytes in UTF-8; placed so its second byte falls exactly at
    // byte offset 57 — the fixed byte-slice cutoff used before the fix.
    let text = format!("{}å{}", "a".repeat(56), "b".repeat(10));
    let display = truncate_for_tray(&text);
    assert!(std::str::from_utf8(display.as_bytes()).is_ok());
}

#[test]
fn truncate_for_tray_all_multibyte_does_not_panic() {
    // "🎉" is 4 bytes in UTF-8; 16 repeats = 64 bytes, so byte offset 57
    // never lands on a char boundary.
    let text = "🎉".repeat(16);
    let display = truncate_for_tray(&text);
    assert!(std::str::from_utf8(display.as_bytes()).is_ok());
}
