use sagascript_core::settings::{HotkeyMode, HotkeyProfile, Settings};

/// One explicit user change. Apply it to fresh settings without replacing
/// unrelated preferences, then bind persistence to the registered shortcuts.
pub enum HotkeyChange {
    Profiles(Vec<HotkeyProfile>),
    Mode(HotkeyMode),
}

impl HotkeyChange {
    pub fn prepare(&self, settings: &Settings) -> Result<Settings, String> {
        let mut candidate = settings.clone();
        match self {
            Self::Profiles(profiles) => candidate.replace_hotkey_profiles(profiles.clone())?,
            Self::Mode(mode) => candidate.replace_hotkey_mode(*mode)?,
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
    use sagascript_core::settings::{HotkeyMode, Settings};

    #[test]
    fn invalid_profiles_never_produce_registration_candidate() {
        assert!(HotkeyChange::Profiles(Vec::new()).prepare(&Settings::default()).is_err());
    }

    #[test]
    fn concurrent_binding_change_rejects_persistence_without_mutation() {
        let settings = Settings::default();
        let change = HotkeyChange::Mode(HotkeyMode::Toggle);
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
        let change = HotkeyChange::Mode(HotkeyMode::Toggle);
        let registered = change.prepare(&settings).unwrap().resolved_shortcuts();
        let mut fresh = settings;
        fresh.show_overlay = false;
        change.apply_registered(&mut fresh, &registered).unwrap();
        assert!(!fresh.show_overlay);
        assert_eq!(fresh.hotkey_mode, HotkeyMode::Toggle);
    }

    #[test]
    fn dual_binding_profile_change_registers_and_persists_both_shortcuts() {
        let settings = Settings::default();
        let mut profiles = settings.resolved_hotkey_profiles();
        profiles[0].push_to_talk_shortcut = Some("Super+S".into());
        profiles[0].toggle_shortcut = Some("Super+Shift+S".into());
        let change = HotkeyChange::Profiles(profiles);
        let registered = change.prepare(&settings).unwrap().resolved_shortcuts();
        assert_eq!(registered, vec!["Super+S", "Super+Shift+S"]);
        let mut fresh = settings;
        fresh.show_overlay = false;
        change.apply_registered(&mut fresh, &registered).unwrap();
        assert!(!fresh.show_overlay);
        assert_eq!(fresh.hotkey, "Super+S");
        assert_eq!(fresh.resolved_hotkey_bindings().len(), 2);
    }

    #[test]
    fn incomplete_dual_binding_registration_cannot_be_persisted() {
        let settings = Settings::default();
        let mut profiles = settings.resolved_hotkey_profiles();
        profiles[0].push_to_talk_shortcut = Some("Super+S".into());
        profiles[0].toggle_shortcut = Some("Super+Shift+S".into());
        let change = HotkeyChange::Profiles(profiles);
        let mut fresh = settings.clone();
        assert!(change.apply_registered(&mut fresh, &["Super+S".into()]).is_err());
        assert_eq!(fresh.resolved_hotkey_profiles(), settings.resolved_hotkey_profiles());
    }
}
