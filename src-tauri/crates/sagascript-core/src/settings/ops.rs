//! One implementation per settings operation (#373).
//!
//! `sagascript config set` and the app's setters used to apply the same user
//! intention separately, and persisted different state: a language change reset
//! the legacy model fields in the app and not in the CLI, `config set hotkey`
//! edited no profile when none was called `default`, and the legacy model flags
//! moved with a model change in the CLI only. The operations below are what
//! both surfaces call, so the state they persist is the same by construction;
//! `core::test_support::settings_ops_table` and the tests that use it in the CLI
//! and in the app check that each surface really does call them.
//!
//! What an operation does **not** do is anything outside the [`Settings`]
//! value. Side effects stay with the surface that has the means for them:
//! registering a shortcut with the OS, downloading the VAD model, asking the OS
//! whether the Accessibility permission was given, rebuilding the tray menu.
//! The operations take what those effects found out ([`Accessibility`],
//! `pianissimo_supported`) as an argument.
//!
//! Each operation is atomic: on `Err` the settings are as they were. They are
//! methods of [`Settings`] like the rest of its operations; the `_gated` ones
//! take the Pianissimo platform gate and have a sibling without the suffix in
//! `crate::machine` that asks this machine (`settings` never probes it).
//!
//! The legacy model fields (`whisper_model`, `auto_select_model`,
//! `pianissimo_dictation`) no longer decide anything once a profile has a model
//! of its own (`profile_models`), but releases before the profile models read
//! them, so the store keeps them in step for a downgrade (ADR 0007, rule 3).
//! Every operation that changes the default profile's model through one of
//! these names therefore updates the matching legacy field, in the app as well
//! as in the CLI. Operations on a named profile (`set_profile_model`,
//! `apply_profiles_change`) do not: a profile other than the default has no
//! legacy field.

use super::{FileModelPreference, HotkeyProfile, ModelResetNotice, Settings};
use crate::models::{Language, WhisperModel};

/// What a surface knows about the Accessibility permission when it is asked to
/// turn auto-paste on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accessibility {
    /// The surface asked the OS and the permission is given (or the OS has no
    /// such permission).
    Granted,
    /// The surface asked the OS and the permission is missing.
    Denied,
    /// The surface cannot ask. The CLI is not the installed app: the permission
    /// belongs to the app's bundle identity, so a CLI process would test the
    /// wrong program. It stores the request, and the app, which can ask, applies
    /// the permission when it loads the settings
    /// (`load_settings_with_permission_gate`).
    Unknown,
}

impl Settings {
    /// Make `language` the default profile's language, as the app's language
    /// picker and `config set language` both mean it: the default profile takes
    /// the language (a model of its own that the language cannot use is reset to
    /// Auto, with a notice), and the legacy model fields go back to "the
    /// recommended model for this language" (`whisper_model` is that model,
    /// `auto_select_model` is on), the way the selection of a language has
    /// always started over in the app.
    ///
    /// Refused, with nothing changed, when the profile has a personal dictionary
    /// with aliases (`set_default_profile_language_gated`).
    /// `pianissimo_supported` is the platform gate.
    pub fn apply_language_selection_gated(
        &mut self,
        language: Language,
        pianissimo_supported: bool,
    ) -> Result<Vec<ModelResetNotice>, String> {
        let notices = self.set_default_profile_language_gated(language, pianissimo_supported)?;
        self.whisper_model = WhisperModel::recommended(language);
        self.auto_select_model = true;
        Ok(notices)
    }

    /// Choose `model` for the default profile's live dictation and turn
    /// automatic selection off. Refused, with nothing changed, when the model
    /// cannot serve the profile's language.
    pub fn set_whisper_model_gated(
        &mut self,
        model: WhisperModel,
        pianissimo_supported: bool,
    ) -> Result<(), String> {
        let profile_id = self.default_profile().id;
        self.set_profile_model_gated(
            &profile_id,
            FileModelPreference::Whisper(model),
            pianissimo_supported,
        )?;
        self.whisper_model = model;
        self.auto_select_model = false;
        Ok(())
    }

    /// Turn automatic model selection on or off for the default profile: on is
    /// the recommended model, off keeps the model that `whisper_model` names.
    /// Refused, with nothing changed, when that model cannot serve the profile's
    /// language.
    pub fn set_auto_select_model_gated(
        &mut self,
        enabled: bool,
        pianissimo_supported: bool,
    ) -> Result<(), String> {
        let profile_id = self.default_profile().id;
        let preference = if enabled {
            FileModelPreference::Auto
        } else {
            FileModelPreference::Whisper(self.whisper_model)
        };
        self.set_profile_model_gated(&profile_id, preference, pianissimo_supported)?;
        self.auto_select_model = enabled;
        Ok(())
    }

    /// Use Pianissimo for the default profile's live dictation, or go back to
    /// the recommended model. Refused, with nothing changed, when the platform
    /// cannot run Pianissimo or the profile is not Swedish.
    pub fn set_pianissimo_dictation_gated(
        &mut self,
        enabled: bool,
        pianissimo_supported: bool,
    ) -> Result<(), String> {
        let profile_id = self.default_profile().id;
        let preference = if enabled {
            FileModelPreference::PianissimoOriginal
        } else {
            FileModelPreference::Auto
        };
        self.set_profile_model_gated(&profile_id, preference, pianissimo_supported)?;
        self.pianissimo_dictation = enabled;
        Ok(())
    }

    /// Replace the profile list and, in the same step, optionally choose a model
    /// for one profile of the new list. A profile whose language changed and
    /// whose model no longer fits goes back to Auto with a notice, except the one
    /// given an explicit `model`, which is checked against the new language
    /// instead: an incompatible choice is an error. Every profile of the result
    /// must resolve to a model. Refused, with nothing changed, when any of that
    /// fails. `pianissimo_supported` is the platform gate.
    ///
    /// The one place a profile list is applied: `sagascript config profiles
    /// create|update|remove` and the app's shortcut profiles both end here.
    pub fn apply_profiles_change_gated(
        &mut self,
        profiles: Vec<HotkeyProfile>,
        model: Option<(&str, FileModelPreference)>,
        pianissimo_supported: bool,
    ) -> Result<Vec<ModelResetNotice>, String> {
        let mut candidate = self.clone();
        let mut notices =
            candidate.replace_hotkey_profiles_gated(profiles, pianissimo_supported)?;
        if let Some((id, preference)) = model {
            candidate.set_profile_model_gated(id, preference, pianissimo_supported)?;
            notices.retain(|notice| notice.profile_id != id);
        }
        for profile in candidate.resolved_hotkey_profiles() {
            candidate.dictation_model_for_profile_gated(&profile.id, pianissimo_supported)?;
        }
        *self = candidate;
        Ok(notices)
    }

    /// Turn auto-paste on or off. Turning it on is stored only where the
    /// permission is not known to be missing: with [`Accessibility::Denied`] the
    /// request is refused and `false` is stored, so the stored value never claims
    /// a paste the OS would block. Turning it off is always stored.
    ///
    /// Returns the stored value, so the surface can tell the user when a request
    /// was refused. What a surface that cannot ask does is described on
    /// [`Accessibility::Unknown`].
    pub fn set_auto_paste(&mut self, requested: bool, accessibility: Accessibility) -> bool {
        self.auto_paste = requested && accessibility != Accessibility::Denied;
        self.auto_paste
    }

    /// Turn voice-activity detection on or off. Only the stored value: the
    /// Silero model it needs is fetched by the surface that turns it on (the app
    /// when the setting is saved, `transcribe --vad` on first use).
    pub fn set_vad_enabled(&mut self, enabled: bool) {
        self.vad_enabled = enabled;
    }
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
