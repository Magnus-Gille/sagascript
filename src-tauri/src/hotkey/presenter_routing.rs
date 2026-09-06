use sagascript_core::settings::{canonical_hotkey, HotkeyMode, HotkeyProfile, Settings};

/// Semantic result of one global-shortcut press. Recording lifecycle and
/// release handling stay with the controller; this helper only resolves the
/// configured binding without touching application state.
#[derive(Debug, PartialEq, Eq)]
pub enum PressAction {
    Start(HotkeyProfile),
    Finish,
    Cancel,
    NoOp,
}

/// Resolve a pressed shortcut against one complete, validated settings value.
/// Profile shortcuts remain starts in every mode. Presenter finish/cancel
/// bindings are active only in Presenter mode and never become implicit starts.
pub fn pressed_action(settings: &Settings, shortcut: &str) -> PressAction {
    if settings.validate_shortcut_configuration().is_err() {
        return PressAction::NoOp;
    }
    let Ok(target) = canonical_hotkey(shortcut) else {
        return PressAction::NoOp;
    };

    if let Some(profile) = settings
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| {
            canonical_hotkey(&profile.shortcut).ok().as_deref() == Some(target.as_str())
        })
    {
        return PressAction::Start(profile);
    }

    if settings.hotkey_mode != HotkeyMode::Presenter {
        return PressAction::NoOp;
    }
    if canonical_hotkey(&settings.presenter.finish_shortcut)
        .ok()
        .as_deref()
        == Some(target.as_str())
    {
        return PressAction::Finish;
    }
    if settings
        .presenter
        .cancel_shortcut
        .as_deref()
        .and_then(|cancel| canonical_hotkey(cancel).ok())
        .as_deref()
        == Some(target.as_str())
    {
        return PressAction::Cancel;
    }
    PressAction::NoOp
}

#[cfg(test)]
mod tests {
    use super::{pressed_action, PressAction};
    use sagascript_core::settings::{HotkeyMode, HotkeyProfile, Language, Settings};

    fn profile(id: &str, shortcut: &str, language: Language) -> HotkeyProfile {
        HotkeyProfile {
            id: id.to_string(),
            name: id.to_string(),
            shortcut: shortcut.to_string(),
            language,
        }
    }

    fn presenter_settings() -> Settings {
        let mut settings = Settings::default();
        settings
            .replace_hotkey_profiles(vec![
                profile("default", "Control+Shift+Space", Language::English),
                profile("swedish", "Control+Option+S", Language::Swedish),
            ])
            .unwrap();
        settings.presenter.cancel_shortcut = Some("Option+Escape".to_string());
        settings.replace_hotkey_mode(HotkeyMode::Presenter).unwrap();
        settings
    }

    #[test]
    fn push_and_toggle_keep_profile_shortcuts_as_starts() {
        for mode in [HotkeyMode::PushToTalk, HotkeyMode::Toggle] {
            let mut settings = Settings::default();
            settings
                .replace_hotkey_profiles(vec![profile(
                    "default",
                    "Control+Shift+Space",
                    Language::English,
                )])
                .unwrap();
            settings.replace_hotkey_mode(mode).unwrap();
            assert!(matches!(
                pressed_action(&settings, "Ctrl+Shift+Space"),
                PressAction::Start(ref profile) if profile.id == "default"
            ));
        }
    }

    #[test]
    fn presenter_routes_start_finish_and_cancel_aliases() {
        let settings = presenter_settings();
        assert!(matches!(
            pressed_action(&settings, "Ctrl+Shift+Space"),
            PressAction::Start(ref profile) if profile.id == "default"
        ));
        assert!(matches!(
            pressed_action(&settings, "Ctrl+Option+S"),
            PressAction::Start(ref profile) if profile.id == "swedish"
        ));
        assert_eq!(
            pressed_action(&settings, "Ctrl+Shift+Enter"),
            PressAction::Finish
        );
        assert_eq!(pressed_action(&settings, "Alt+Esc"), PressAction::Cancel);
    }

    #[test]
    fn inactive_presenter_bindings_are_noops_unless_profile_owns_binding() {
        let mut settings = Settings::default();
        settings.presenter.cancel_shortcut = Some("Option+Escape".to_string());
        assert_eq!(
            pressed_action(&settings, "Ctrl+Shift+Enter"),
            PressAction::NoOp
        );
        assert_eq!(pressed_action(&settings, "Alt+Escape"), PressAction::NoOp);

        settings
            .replace_hotkey_profiles(vec![profile(
                "default",
                "Control+Shift+Enter",
                Language::English,
            )])
            .unwrap();
        assert!(matches!(
            pressed_action(&settings, "Ctrl+Shift+Enter"),
            PressAction::Start(ref profile) if profile.id == "default"
        ));
    }

    #[test]
    fn invalid_or_unknown_shortcuts_are_noops_without_implicit_start() {
        let settings = presenter_settings();
        assert_eq!(
            pressed_action(&settings, "Control+NotAKey"),
            PressAction::NoOp
        );
        assert_eq!(
            pressed_action(&settings, "Control+Shift+Q"),
            PressAction::NoOp
        );
    }

    #[test]
    fn invalid_whole_configuration_disables_every_route() {
        let mut settings = presenter_settings();
        settings.presenter.finish_shortcut = settings.hotkey.clone();
        assert_eq!(
            pressed_action(&settings, "Ctrl+Shift+Space"),
            PressAction::NoOp
        );
        assert_eq!(
            pressed_action(&settings, "Ctrl+Shift+Enter"),
            PressAction::NoOp
        );
    }
}
