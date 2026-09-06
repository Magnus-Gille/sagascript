use sagascript_core::settings::{HotkeyMode, HotkeyProfile, PresenterConfig, Settings};

/// One explicit user change. Apply it to fresh settings without replacing
/// unrelated preferences, then bind persistence to the registered shortcuts.
pub enum HotkeyChange {
    Profiles(Vec<HotkeyProfile>),
    Mode(HotkeyMode),
    Presenter(PresenterConfig),
}

impl HotkeyChange {
    pub fn prepare(&self, settings: &Settings) -> Result<Settings, String> {
        let mut candidate = settings.clone();
        match self {
            Self::Profiles(profiles) => candidate.replace_hotkey_profiles(profiles.clone())?,
            Self::Mode(mode) => candidate.replace_hotkey_mode(*mode)?,
            Self::Presenter(config) => candidate.replace_presenter_config(config.clone())?,
        }
        candidate.validate_shortcut_configuration()?;
        Ok(candidate)
    }

    pub fn apply_registered(
        &self,
        fresh: &mut Settings,
        registered: &[String],
    ) -> Result<(), String> {
        let candidate = self.prepare(fresh)?;
        if candidate.resolved_shortcuts() != registered {
            return Err("Shortcut configuration changed concurrently; retry after settings refresh".into());
        }
        *fresh = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HotkeyChange;
    use sagascript_core::settings::{HotkeyMode, PresenterConfig, Settings};

    #[test]
    fn presenter_change_registers_complete_active_binding_set() {
        let settings = Settings::default();
        let changed = HotkeyChange::Mode(HotkeyMode::Presenter).prepare(&settings).unwrap();
        assert_eq!(changed.resolved_shortcuts(), vec!["Control+Shift+Space", "Control+Shift+Enter"]);
        assert_eq!(settings.hotkey_mode, HotkeyMode::PushToTalk);
    }

    #[test]
    fn concurrent_binding_change_rejects_persistence_without_mutation() {
        let settings = Settings::default();
        let change = HotkeyChange::Mode(HotkeyMode::Presenter);
        let registered = change.prepare(&settings).unwrap().resolved_shortcuts();
        let mut fresh = settings;
        fresh.set_legacy_hotkey("Control+Shift+E".into()).unwrap();
        let original = serde_json::to_value(&fresh).unwrap();
        assert!(change.apply_registered(&mut fresh, &registered).is_err());
        assert_eq!(serde_json::to_value(&fresh).unwrap(), original);
    }

    #[test]
    fn unrelated_fresh_settings_are_preserved() {
        let settings = Settings::default();
        let change = HotkeyChange::Mode(HotkeyMode::Presenter);
        let registered = change.prepare(&settings).unwrap().resolved_shortcuts();
        let mut fresh = settings;
        fresh.show_overlay = false;
        change.apply_registered(&mut fresh, &registered).unwrap();
        assert!(!fresh.show_overlay);
        assert_eq!(fresh.hotkey_mode, HotkeyMode::Presenter);
    }

    #[test]
    fn invalid_configuration_never_produces_registration_candidate() {
        let settings = Settings { hotkey_mode: HotkeyMode::Presenter, ..Settings::default() };
        let config = PresenterConfig { finish_shortcut: settings.hotkey.clone(), ..PresenterConfig::default() };
        assert!(HotkeyChange::Presenter(config).prepare(&settings).is_err());
        assert!(HotkeyChange::Profiles(Vec::new()).prepare(&settings).is_err());
    }
}
