use super::*;

#[test]
fn update_status_describes_available_and_current_releases() {
    assert_eq!(
        update_status_text(&updates::UpdateCheck::Available {
            version: semver::Version::new(1, 2, 3)
        }),
        "Update available — v1.2.3"
    );
    assert_eq!(
        update_status_text(&updates::UpdateCheck::UpToDate),
        "Sagascript is up to date"
    );
    assert_eq!(
        update_action_text(&updates::UpdateCheck::Available {
            version: semver::Version::new(1, 2, 3)
        }),
        "Open Release v1.2.3…"
    );
}

#[test]
fn update_menu_makes_available_release_actionable() {
    let result = updates::UpdateCheck::Available {
        version: semver::Version::new(1, 2, 0),
    };

    assert_eq!(update_status_text(&result), "Update available — v1.2.0");
    assert_eq!(update_action_text(&result), "Open Release v1.2.0…");
}

#[test]
fn update_menu_keeps_a_clear_recheck_action_when_current() {
    assert_eq!(
        update_action_text(&updates::UpdateCheck::UpToDate),
        "Check Again…"
    );
}

#[test]
fn update_action_targets_the_exact_stable_release() {
    assert_eq!(
        stable_release_url(&semver::Version::new(1, 2, 0)),
        "https://github.com/Magnus-Gille/sagascript/releases/tag/v1.2.0"
    );
}
