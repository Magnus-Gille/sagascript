use super::*;

#[test]
fn tray_label_idle_not_failed() {
    assert_eq!(
        tray_label("idle", false, false),
        ("Sagascript", "S", "Idle")
    );
}

#[test]
fn tray_label_recording_not_failed() {
    assert_eq!(
        tray_label("recording", false, false),
        ("Sagascript - Recording...", "●", "Recording...")
    );
}

#[test]
fn tray_status_uses_compact_native_state_markers() {
    assert_eq!(tray_label("idle", false, false).1, "S");
    assert_eq!(tray_label("recording", false, false).1, "●");
    assert_eq!(tray_label("loading_model", false, false).1, "…");
    assert_eq!(tray_label("transcribing", false, false).1, "…");
    assert_eq!(tray_label("idle", true, false).1, "!");
}

#[test]
fn tray_label_hotkey_failed_is_distinct_from_idle() {
    let failed = tray_label("idle", true, false);
    assert_ne!(failed, tray_label("idle", false, false));
    assert_eq!(failed.2, "Hotkey unavailable");
}

#[test]
fn tray_label_hotkey_failed_wins_over_recording() {
    // The sticky warning must win over a normal state transition into
    // "recording" — a hotkey that isn't registered can't actually be
    // driving a recording state the user trusts.
    assert_eq!(
        tray_label("recording", true, false),
        tray_label("idle", true, false)
    );
}

#[test]
fn tray_label_hotkey_failed_wins_over_transcribing() {
    assert_eq!(
        tray_label("transcribing", true, false),
        tray_label("idle", true, false)
    );
}

#[test]
fn tray_label_hotkey_failed_wins_over_loading_model() {
    assert_eq!(
        tray_label("loading_model", true, false),
        tray_label("idle", true, false)
    );
}

#[test]
fn tray_label_recorder_active_shows_audio_recording() {
    assert_eq!(
        tray_label("idle", false, true),
        ("Sagascript - Recording audio...", "●", "Recording audio...")
    );
}

#[test]
fn tray_label_recorder_active_wins_over_every_dictation_state() {
    // A dictation's `update_tray_status(_, "idle")` after it finished must not
    // erase the recorder's indicator: whatever the state, a running recorder
    // shows.
    let shown = tray_label("idle", false, true);
    for state in [
        "idle",
        "recording",
        "loading_model",
        "transcribing",
        "other",
    ] {
        assert_eq!(tray_label(state, false, true), shown, "{state}");
    }
    assert_ne!(shown, tray_label("idle", false, false));
    assert_ne!(shown, tray_label("recording", false, false));
}

#[test]
fn tray_label_recorder_active_keeps_the_hotkey_warning_in_the_text() {
    // The recording indicator wins, but the broken hotkey stays visible.
    let (tooltip, title, menu_text) = tray_label("idle", true, true);
    assert_eq!(title, "●");
    assert!(menu_text.starts_with("Recording audio..."), "{menu_text}");
    assert!(menu_text.contains("hotkey unavailable"), "{menu_text}");
    assert!(tooltip.contains("Recording audio..."), "{tooltip}");
    assert!(tooltip.contains("hotkey unavailable"), "{tooltip}");
    assert_ne!((tooltip, title, menu_text), tray_label("idle", false, true));
}

#[test]
fn dictation_state_names_map_to_tray_states() {
    assert_eq!(dictation_state_name(AppState::Idle), "idle");
    assert_eq!(dictation_state_name(AppState::Error), "idle");
    assert_eq!(dictation_state_name(AppState::Recording), "recording");
    assert_eq!(dictation_state_name(AppState::Transcribing), "transcribing");
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
