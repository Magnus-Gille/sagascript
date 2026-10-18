//! This machine's answers to the questions the settings code asks, and the
//! entry points that use them (#372).
//!
//! `models` and `settings` never probe the machine: what is downloaded and
//! whether Pianissimo can run here are injected, as a `MachineProbe` into the
//! store (`store::load_with`, `store::try_update_with`) and as plain values into
//! the `*_gated` methods of [`Settings`]. This module sits above both settings
//! and transcription and supplies the real answers from the transcription code:
//!
//! - `ThisMachine`, the `MachineProbe` of this machine;
//! - [`load`], [`update`] and [`try_update`], the store entry points with it;
//! - the [`Settings`] methods without a `_gated` suffix (`dictation_model_for_profile`,
//!   `set_profile_model`, `effective_file_model_for`, ...): each is its `_gated`
//!   sibling with this machine's probe, so callers that do not inject keep one
//!   short call.
//!
//! The old paths `settings::store::{load, update, try_update}` re-export the
//! entry points for one release.

use crate::models::{FileModel, Language, WhisperModel};
use crate::settings::{
    store, FileModelPreference, HotkeyProfile, LineupEnv, MachineProbe, ModelResetNotice, Settings,
    SettingsError,
};
use crate::transcription::{model, pianissimo_backend, pianissimo_model};

/// Whether Pianissimo can run on this machine: macOS 14+ on Apple Silicon, or
/// Windows on ARM with the bundled host
/// ([`pianissimo_backend::runtime_supported_on_this_os`]).
pub(crate) fn pianissimo_supported() -> bool {
    pianissimo_backend::runtime_supported_on_this_os()
}

/// Whether the files of a file-transcription model are on disk here.
pub(crate) fn file_model_ready(choice: FileModel) -> bool {
    match choice {
        FileModel::Whisper(whisper) => model::is_model_downloaded(whisper),
        FileModel::PianissimoOriginal => pianissimo_model::is_downloaded(),
    }
}

/// The machine this process runs on. Probed lazily: the settings store asks
/// for [`MachineProbe::lineup_env`] only while the Swedish lineup migration is
/// pending.
pub(crate) struct ThisMachine;

impl MachineProbe for ThisMachine {
    fn pianissimo_supported(&self) -> bool {
        pianissimo_supported()
    }

    fn lineup_env(&self) -> LineupEnv<'_> {
        LineupEnv {
            pianissimo_supported: self.pianissimo_supported(),
            pianissimo_downloaded: pianissimo_model::is_downloaded(),
            whisper_downloaded: &model::is_model_downloaded,
        }
    }
}

/// Load settings from disk, migrating an older settings file (see
/// `settings::migrations`). Returns defaults if the file is missing or
/// unreadable. Partial JSON files are handled by `#[serde(default)]` on
/// `Settings`. `store::load_with` with the probes of this machine.
pub fn load() -> Settings {
    store::load_with(&ThisMachine)
}

/// Apply a field-level settings mutation to the latest on-disk snapshot and
/// return the persisted result.
///
/// GUI commands use this instead of saving their in-memory `Settings` clone,
/// which may be stale when the CLI changed another field moments earlier.
/// `store::update_with` with the probes of this machine.
pub fn update<F>(mutate: F) -> Result<Settings, SettingsError>
where
    F: FnOnce(&mut Settings),
{
    store::update_with(&ThisMachine, mutate)
}

/// Like [`update`], but lets validation fail while the cross-process lock is
/// held so read/validate/write profile mutations cannot race another writer.
/// `store::try_update_with` with the probes of this machine.
///
/// The closure's own error type `E` comes back unchanged; a failure of the store
/// itself is converted into it (`E: From<SettingsError>`), so a caller that
/// validates with its own error type does not have to flatten either one.
pub fn try_update<E, F>(mutate: F) -> Result<Settings, E>
where
    E: From<SettingsError>,
    F: FnOnce(&mut Settings) -> Result<(), E>,
{
    store::try_update_with(&ThisMachine, mutate)
}

/// The `_gated` methods of `settings::manager`, asked of this machine.
impl Settings {
    /// Resolve the live engine for one profile. Unmigrated settings retain
    /// their previous global selection until the profile migration is saved.
    pub fn dictation_model_for_profile(&self, profile_id: &str) -> Result<FileModel, SettingsError> {
        self.dictation_model_for_profile_gated(profile_id, pianissimo_supported())
    }

    /// [`Self::dictation_model_for_profile`] as the Whisper model and whether
    /// the profile uses Pianissimo instead.
    pub fn live_model_for_profile(&self, profile_id: &str) -> Result<(WhisperModel, bool), SettingsError> {
        self.live_model_for_profile_gated(profile_id, pianissimo_supported())
    }

    /// True when `profile_id`'s live dictation model is Pianissimo.
    pub fn profile_uses_pianissimo(&self, profile_id: &str) -> bool {
        self.profile_uses_pianissimo_gated(profile_id, pianissimo_supported())
    }

    /// True when any dictation profile's live model is Pianissimo.
    pub fn any_profile_uses_pianissimo(&self) -> bool {
        self.any_profile_uses_pianissimo_gated(pianissimo_supported())
    }

    pub fn set_profile_model(
        &mut self,
        profile_id: &str,
        preference: FileModelPreference,
    ) -> Result<(), SettingsError> {
        self.set_profile_model_gated(profile_id, preference, pianissimo_supported())
    }

    /// Change the default profile's language. A model the new language cannot
    /// use is reset to Auto; the returned notices describe every such reset.
    pub fn set_default_profile_language(
        &mut self,
        language: Language,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        self.set_default_profile_language_gated(language, pianissimo_supported())
    }

    /// The settings operations of `settings::ops`, asked of this machine.
    pub fn apply_language_selection(
        &mut self,
        language: Language,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        self.apply_language_selection_gated(language, pianissimo_supported())
    }

    /// See [`Self::set_whisper_model_gated`].
    pub fn set_whisper_model(&mut self, model: WhisperModel) -> Result<(), SettingsError> {
        self.set_whisper_model_gated(model, pianissimo_supported())
    }

    /// See [`Self::set_auto_select_model_gated`].
    pub fn set_auto_select_model(&mut self, enabled: bool) -> Result<(), SettingsError> {
        self.set_auto_select_model_gated(enabled, pianissimo_supported())
    }

    /// See [`Self::set_pianissimo_dictation_gated`].
    pub fn set_pianissimo_dictation(&mut self, enabled: bool) -> Result<(), SettingsError> {
        self.set_pianissimo_dictation_gated(enabled, pianissimo_supported())
    }

    /// See [`Self::apply_profiles_change_gated`].
    pub fn apply_profiles_change(
        &mut self,
        profiles: Vec<HotkeyProfile>,
        model: Option<(&str, FileModelPreference)>,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        self.apply_profiles_change_gated(profiles, model, pianissimo_supported())
    }

    /// Replace the profile list. When a profile's language changes and its explicit
    /// model no longer fits, that profile's model is reset to Auto and a notice
    /// is returned instead of an error.
    pub fn replace_hotkey_profiles(
        &mut self,
        profiles: Vec<HotkeyProfile>,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        self.replace_hotkey_profiles_gated(profiles, pianissimo_supported())
    }

    /// Resolve the model for file transcription without changing the live
    /// dictation model preference. An `Auto` preference prefers the
    /// recommended model when it is downloaded, else the best downloaded
    /// compatible model (see [`Self::effective_file_model_for_gated`]).
    pub fn effective_file_model_for(&self, language: Language) -> Result<FileModel, SettingsError> {
        self.effective_file_model_for_gated(language, pianissimo_supported(), &file_model_ready)
    }

    /// Like [`Self::effective_file_model_for`], but for work that needs a
    /// Whisper model (meetings, diarization, decoder hints, beam search): an
    /// `Auto` preference never resolves to Pianissimo. An explicit Pianissimo
    /// choice is returned as-is so callers can report it clearly.
    pub fn effective_whisper_file_model_for(
        &self,
        language: Language,
    ) -> Result<FileModel, SettingsError> {
        self.effective_whisper_file_model_for_gated(language, &file_model_ready)
    }

    /// Build the ordered set of profile models worth loading during GUI
    /// startup. Pianissimo profiles are excluded. The primary remaining Whisper
    /// profile is always first and included. Additional distinct models must fit both
    /// the resident-entry limit and advertised model-size budget.
    pub fn warm_model_plan(
        &self,
        max_models: usize,
        max_total_mb: u32,
    ) -> Vec<(WhisperModel, Language)> {
        self.warm_model_plan_gated(max_models, max_total_mb, pianissimo_supported())
    }
}

#[cfg(test)]
#[path = "machine_tests.rs"]
mod tests;
