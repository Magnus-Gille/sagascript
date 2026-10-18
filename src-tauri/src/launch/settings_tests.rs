use super::*;
use sagascript_testsupport::TempDir;

#[test]
fn auto_paste_requires_runtime_accessibility_approval() {
    assert!(auto_paste_permitted(false, false));
    assert!(auto_paste_permitted(false, true));
    assert!(!auto_paste_permitted(true, false));
    assert!(auto_paste_permitted(true, true));
}

#[test]
fn migrate_legacy_settings_renames_when_new_absent() {
    let dir = TempDir::new();
    let legacy = dir.join("flowdictate-settings.json");
    let new_path = dir.join("sagascript-settings.json");
    std::fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();

    migrate_legacy_settings(&legacy, &new_path);

    assert!(!legacy.exists(), "legacy file should be renamed away");
    assert!(
        new_path.exists(),
        "new path should now hold the migrated settings"
    );
    assert_eq!(
        std::fs::read_to_string(&new_path).unwrap(),
        r#"{"language":"sv"}"#
    );
}

#[test]
fn overridden_settings_path_disables_flowdictate_migration() {
    let dir = TempDir::new();
    let legacy = dir.join("flowdictate-settings.json");
    let new_path = dir.join("sagascript-settings.json");
    std::fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();

    migrate_legacy_settings_unless_overridden(&legacy, &new_path, true);

    assert!(
        legacy.exists(),
        "override mode must not inspect or move legacy settings"
    );
    assert!(!new_path.exists());
}

#[test]
fn migrate_legacy_settings_noop_when_new_already_exists() {
    let dir = TempDir::new();
    let legacy = dir.join("flowdictate-settings.json");
    let new_path = dir.join("sagascript-settings.json");
    std::fs::write(&legacy, "legacy-content").unwrap();
    std::fs::write(&new_path, "already-migrated-content").unwrap();

    migrate_legacy_settings(&legacy, &new_path);

    // Neither file should be touched — a Sagascript settings file already exists.
    assert!(legacy.exists());
    assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "legacy-content");
    assert_eq!(
        std::fs::read_to_string(&new_path).unwrap(),
        "already-migrated-content"
    );
}

#[test]
fn migrate_legacy_settings_noop_when_legacy_absent() {
    let dir = TempDir::new();
    let legacy = dir.join("flowdictate-settings.json");
    let new_path = dir.join("sagascript-settings.json");

    // Should not panic when there's nothing to migrate.
    migrate_legacy_settings(&legacy, &new_path);

    assert!(!new_path.exists());
}

/// Regression test for the swallowed-rename bug: a failed rename (here,
/// forced by a destination whose parent directory doesn't exist) must not
/// panic, and the legacy file must be left in place rather than lost —
/// the old `let _ = std::fs::rename(...)` code silently discarded the
/// error either way, but this at least confirms the failure path doesn't
/// destroy data.
#[test]
fn migrate_legacy_settings_does_not_panic_on_rename_failure() {
    let dir = TempDir::new();
    let legacy = dir.join("flowdictate-settings.json");
    std::fs::write(&legacy, "legacy-content").unwrap();
    // Destination's parent directory doesn't exist -> rename fails.
    let new_path = dir
        .join("nonexistent-subdir")
        .join("sagascript-settings.json");

    migrate_legacy_settings(&legacy, &new_path);

    assert!(legacy.exists(), "legacy file must survive a failed rename");
    assert!(!new_path.exists());
}
