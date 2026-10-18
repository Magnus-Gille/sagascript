use super::*;

#[cfg(target_os = "macos")]
#[test]
fn tray_autosave_name_is_stable_and_bundle_qualified() {
    assert_eq!(TRAY_AUTOSAVE_NAME, "ai.gille.sagascript.main");
    assert_eq!(
        tray_preferred_position_key(TRAY_AUTOSAVE_NAME),
        "NSStatusItem Preferred Position ai.gille.sagascript.main"
    );
    assert_eq!(DEFAULT_TRAY_PREFERRED_POSITION, 340.0);
    assert_eq!(initial_tray_preferred_position(false), Some(340.0));
    assert_eq!(initial_tray_preferred_position(true), None);
}
