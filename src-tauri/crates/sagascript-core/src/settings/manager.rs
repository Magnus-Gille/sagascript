use std::collections::{BTreeMap, HashSet};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use super::{canonical_hotkey, validate_hotkey, LineupEnv, SettingsError};

use crate::glossary::Glossary;

// The model catalog moved to `crate::models` (#372). Re-exported so the old
// paths (`sagascript_core::settings::{FileModel, Language, WhisperModel}`)
// keep compiling for the CLI and the app. Shim: remove one release after the
// callers import from `sagascript_core::models`.
pub use crate::models::{FileModel, Language, WhisperModel};

/// Resolve an `Auto` file-model preference against what is downloaded.
/// `allow_pianissimo` false yields Whisper models only.
fn auto_file_model(
    language: Language,
    pianissimo_supported: bool,
    allow_pianissimo: bool,
    is_ready: &dyn Fn(FileModel) -> bool,
) -> FileModel {
    let supported = pianissimo_supported && allow_pianissimo;
    let recommended = FileModel::recommended_for(language, supported);
    if is_ready(recommended) {
        return recommended;
    }
    let mut candidates = FileModel::app_lineup(language, supported, None);
    let mut rest: Vec<WhisperModel> = WhisperModel::models_for_language(language).to_vec();
    rest.sort_by_key(|m| std::cmp::Reverse(m.size_mb()));
    candidates.extend(rest.into_iter().map(FileModel::Whisper));
    candidates
        .into_iter()
        .filter(|m| allow_pianissimo || *m != FileModel::PianissimoOriginal)
        .find(|m| is_ready(*m))
        .unwrap_or(recommended)
}

/// Persisted preference for file transcription.
///
/// `Auto` chooses the requested language's recommended Whisper model,
/// independently of every live-dictation profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FileModelPreference {
    #[default]
    Auto,
    Whisper(WhisperModel),
    PianissimoOriginal,
}

impl FileModelPreference {
    /// Stable identifier of this preference: `auto`, a Whisper model's
    /// [`WhisperModel::id`] or [`FileModel::PIANISSIMO_ID`]. What the settings
    /// file stores and `config get` prints; [`Self::parse_id`] is its inverse.
    pub fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Whisper(model) => model.id(),
            Self::PianissimoOriginal => FileModel::PIANISSIMO_ID,
        }
    }

    /// Parse one of the stable persisted model identifiers.
    pub fn parse_id(id: &str) -> Result<Self, SettingsError> {
        match id {
            "auto" => Ok(Self::Auto),
            FileModel::PIANISSIMO_ID => Ok(Self::PianissimoOriginal),
            _ => WhisperModel::from_id(id)
                .map(Self::Whisper)
                .ok_or_else(|| SettingsError::UnknownModelPreference { id: id.to_string() }),
        }
    }
}

impl Serialize for FileModelPreference {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for FileModelPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FileModelPreferenceVisitor;

        impl<'de> de::Visitor<'de> for FileModelPreferenceVisitor {
            type Value = FileModelPreference;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a file model preference identifier")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                FileModelPreference::parse_id(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(FileModelPreferenceVisitor)
    }
}

/// Hotkey activation mode
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HotkeyMode {
    #[serde(rename = "push", alias = "presenter")]
    #[default]
    PushToTalk,
    #[serde(rename = "toggle")]
    Toggle,
}

impl HotkeyMode {
    /// Both modes, in the order the app lists them.
    pub const ALL: [HotkeyMode; 2] = [HotkeyMode::PushToTalk, HotkeyMode::Toggle];

    /// Stable identifier of this mode: what the settings file stores and
    /// `sagascript config` prints.
    pub fn id(self) -> &'static str {
        match self {
            HotkeyMode::PushToTalk => "push",
            HotkeyMode::Toggle => "toggle",
        }
    }

    /// The mode with this stable identifier, if any. The retired `presenter`
    /// is not one: only a settings file is still read with it (as `push`).
    pub fn from_id(id: &str) -> Option<HotkeyMode> {
        HotkeyMode::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

/// When the Pianissimo engine host loads its model ahead of the first utterance.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnginePrewarm {
    /// Load lazily when the first transcription needs the model.
    Off,
    /// Load in the background when the app starts, after system wake and when
    /// a profile switches to Pianissimo (Pianissimo active only). Default.
    #[default]
    OnAppStart,
    /// Load in the background when the push-to-talk key goes down.
    OnKeyDown,
}

impl EnginePrewarm {
    pub const ALL: [EnginePrewarm; 3] = [Self::Off, Self::OnAppStart, Self::OnKeyDown];

    /// Stable identifier used in the settings file and by `sagascript config`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::OnAppStart => "on_app_start",
            Self::OnKeyDown => "on_key_down",
        }
    }

    pub fn parse_id(value: &str) -> Result<Self, SettingsError> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.id() == value)
            .ok_or_else(|| SettingsError::InvalidEnginePrewarm {
                value: value.to_string(),
            })
    }
}

/// Default minutes of inactivity before the engine host unloads its model.
pub const DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES: u32 = 60;

/// Defaults that shipped before [`ENGINE_DEFAULTS_VERSION`] 1, used to tell
/// users still on the old defaults from users who chose something else.
pub const LEGACY_ENGINE_IDLE_UNLOAD_MINUTES: u32 = 10;

/// Current version of the engine warm-up defaults migration.
pub const ENGINE_DEFAULTS_VERSION: u32 = 1;

/// One global shortcut and the transcription language selected when it fires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotkeyProfile {
    pub id: String,
    pub name: String,
    pub shortcut: String,
    pub language: Language,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_to_talk_shortcut: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_shortcut: Option<String>,
}

impl HotkeyProfile {
    pub fn legacy_default(shortcut: String, language: Language) -> Self {
        Self {
            id: "default".to_string(),
            name: "Default".to_string(),
            shortcut,
            language,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }
    }

    /// Index of the default profile in `profiles`: the one called `default`,
    /// else the first. `profiles` must not be empty. The profile that `hotkey`
    /// and `language` mirror, that `default_profile()` returns and that the
    /// legacy-named operations (`config set hotkey`, `set_language`) change.
    pub fn default_index(profiles: &[HotkeyProfile]) -> usize {
        profiles
            .iter()
            .position(|profile| profile.id == "default")
            .unwrap_or(0)
    }

    /// Update the compatibility shortcut and whichever explicit binding is
    /// primary for this profile.
    pub fn set_primary_shortcut(&mut self, shortcut: String) {
        if self.push_to_talk_shortcut.is_some() {
            self.push_to_talk_shortcut = Some(shortcut.clone());
        } else if self.toggle_shortcut.is_some() {
            self.toggle_shortcut = Some(shortcut.clone());
        }
        self.shortcut = shortcut;
    }
}

/// All user-configurable settings, persisted as JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: Language,
    pub whisper_model: WhisperModel,
    pub file_transcription_model: FileModelPreference,
    /// Use the experimental native Pianissimo runtime for Swedish live dictation.
    /// File transcription continues to use `file_transcription_model`.
    pub pianissimo_dictation: bool,
    /// When the engine host loads the Pianissimo model ahead of use.
    pub engine_prewarm: EnginePrewarm,
    /// Minutes of inactivity before the engine host unloads its model; the host
    /// process itself exits after twice this. 0 disables idle unloading.
    pub engine_idle_unload_minutes: u32,
    pub hotkey_mode: HotkeyMode,
    pub show_overlay: bool,
    pub auto_paste: bool,
    pub auto_select_model: bool,
    /// Hotkey shortcut string (e.g. "Control+Shift+Space")
    pub hotkey: String,
    /// Explicit dictation profiles. Empty means a legacy settings file; callers
    /// use `resolved_hotkey_profiles()` to synthesize its default profile.
    pub hotkey_profiles: Vec<HotkeyProfile>,
    /// Live model owned by each profile id. `Auto` selects the recommendation
    /// for that profile's language; file transcription has its own preference.
    pub profile_models: BTreeMap<String, FileModelPreference>,
    /// Optional initial prompt that primes the decoder with domain vocabulary
    /// (names, jargon, spellings) for more accurate transcription. Empty = none.
    pub initial_prompt: String,
    /// Additional personal dictionary entries scoped to a dictation profile.
    /// The legacy `initial_prompt` remains on disk only for rollback.
    pub profile_glossaries: BTreeMap<String, String>,
    /// Legacy global glossary has been copied into the profile dictionaries.
    /// The original file remains on disk for rollback, but no longer affects
    /// dictation once this marker is set.
    pub profile_glossary_migrated: bool,
    /// Beam search width. 0 = greedy decoding (fastest); >=2 enables beam search
    /// (more accurate on hard audio, several times slower).
    pub beam_size: u32,
    /// Allow whisper's temperature fallback (re-decode hard segments at higher
    /// temperature). true preserves robustness; false caps worst-case latency.
    pub temperature_fallback: bool,
    /// Skip non-speech regions with Silero VAD (reduces silence hallucination
    /// and speeds up clips with leading/trailing silence). Needs the VAD model.
    pub vad_enabled: bool,
    /// Whether the user has completed the first-launch onboarding
    #[serde(alias = "hasCompletedOnboarding")]
    pub has_completed_onboarding: bool,
    /// Version of the Swedish model-lineup migration that has run on this file
    /// (see [`Settings::migrate_swedish_lineup`]). A missing key means 0: files
    /// written before the lineup change are migrated once. New installs start
    /// at the current version.
    #[serde(default)]
    pub model_lineup_version: u32,
    /// Version of the engine warm-up defaults migration that has run on this
    /// file (see [`Settings::migrate_engine_defaults`]). A missing key means 0.
    /// New installs start at the current version.
    #[serde(default)]
    pub engine_defaults_version: u32,
    /// Schema version of the settings file (see `settings::migrations`). A
    /// missing key means 0: the file predates it, and the migrations derive
    /// its version from the markers above, which they keep writing so older
    /// releases can still read the file. New installs start at
    /// [`SCHEMA_VERSION`](super::SCHEMA_VERSION).
    #[serde(default)]
    pub schema_version: u32,
}

/// Current version of the Swedish model-lineup migration.
pub const MODEL_LINEUP_VERSION: u32 = 1;

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            whisper_model: WhisperModel::default(),
            file_transcription_model: FileModelPreference::default(),
            pianissimo_dictation: false,
            engine_prewarm: EnginePrewarm::default(),
            engine_idle_unload_minutes: DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES,
            hotkey_mode: HotkeyMode::default(),
            show_overlay: true,
            auto_paste: true,
            auto_select_model: true,
            hotkey: "Control+Shift+Space".to_string(),
            hotkey_profiles: Vec::new(),
            profile_models: BTreeMap::new(),
            initial_prompt: String::new(),
            profile_glossaries: BTreeMap::new(),
            profile_glossary_migrated: false,
            beam_size: 0,
            temperature_fallback: true,
            vad_enabled: false,
            has_completed_onboarding: false,
            model_lineup_version: MODEL_LINEUP_VERSION,
            engine_defaults_version: ENGINE_DEFAULTS_VERSION,
            schema_version: super::SCHEMA_VERSION,
        }
    }
}

/// A profile's model was reset to Auto because a language change made it
/// incompatible. Structured so callers can filter by profile id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelResetNotice {
    pub profile_id: String,
    pub profile_name: String,
    pub message: String,
}

impl ModelResetNotice {
    /// User-facing strings, for wire formats that carry plain messages.
    pub fn messages(notices: &[ModelResetNotice]) -> Vec<String> {
        notices
            .iter()
            .map(|notice| notice.message.clone())
            .collect()
    }
}

impl Settings {
    /// One-time move of the engine warm-up settings to the cold-start defaults
    /// (load at app start, unload after 60 idle minutes), recorded by
    /// `engine_defaults_version`. Only values still equal to the previous
    /// defaults move: an explicit `off`, `on_app_start` or any other idle
    /// timeout is kept. (A user who explicitly picked the old default values
    /// is indistinguishable from one who never touched them and moves too.)
    /// Returns true when it ran. Applied by settings migration step 6.
    pub fn migrate_engine_defaults(&mut self) -> bool {
        if self.engine_defaults_version >= ENGINE_DEFAULTS_VERSION {
            return false;
        }
        self.engine_defaults_version = ENGINE_DEFAULTS_VERSION;
        if self.engine_prewarm == EnginePrewarm::OnKeyDown {
            self.engine_prewarm = EnginePrewarm::OnAppStart;
        }
        if self.engine_idle_unload_minutes == LEGACY_ENGINE_IDLE_UNLOAD_MINUTES {
            self.engine_idle_unload_minutes = DEFAULT_ENGINE_IDLE_UNLOAD_MINUTES;
        }
        true
    }

    /// One-time move of Swedish dictation off the retired KB-Whisper
    /// Tiny/Base/Small choices, recorded by `model_lineup_version` so a second
    /// run is a no-op. Returns true when it ran. Applied by settings
    /// migration step 5.
    ///
    /// Safety rule: dictation must keep working. When a profile's model is not
    /// downloaded, the hotkey flow fails the utterance ("model not
    /// downloaded") and Settings shows a "<model> required" download prompt,
    /// so switching engines under a user who lacks the new one would break
    /// dictation until they download ~608 MB. Therefore a profile only moves
    /// when the recommended engine is already downloaded, or when its current
    /// model is not usable either (then the move costs nothing and Settings
    /// prompts for the recommended engine). Otherwise the old model is kept
    /// and Settings offers a one-click switch that downloads first.
    /// `Auto` profiles used to resolve to KB-Whisper Base; if Base is
    /// downloaded but the new recommendation is not, they are pinned to Base
    /// for the same reason. Explicit Medium/Large/Pianissimo are untouched.
    pub fn migrate_swedish_lineup(&mut self, env: &LineupEnv<'_>) -> bool {
        if self.model_lineup_version >= MODEL_LINEUP_VERSION {
            return false;
        }
        self.model_lineup_version = MODEL_LINEUP_VERSION;
        let target = FileModel::recommended_for(Language::Swedish, env.pianissimo_supported);
        let (target_pref, target_ready) = match target {
            FileModel::PianissimoOriginal => (
                FileModelPreference::PianissimoOriginal,
                env.pianissimo_downloaded,
            ),
            FileModel::Whisper(model) => (
                FileModelPreference::Whisper(model),
                (env.whisper_downloaded)(model),
            ),
        };
        let previous_auto = WhisperModel::KbWhisperBase;
        let swedish: Vec<String> = self
            .resolved_hotkey_profiles()
            .into_iter()
            .filter(|profile| profile.language == Language::Swedish)
            .map(|profile| profile.id)
            .collect();
        for id in swedish {
            let Some(current) = self.profile_models.get(&id).copied() else {
                continue;
            };
            match current {
                FileModelPreference::Whisper(old) if old.is_hidden_in_app() => {
                    if target_ready || !(env.whisper_downloaded)(old) {
                        tracing::info!(profile = %id, from = ?old, to = ?target_pref, "Swedish model lineup: moved profile to the recommended model");
                        self.profile_models.insert(id, target_pref);
                    } else {
                        tracing::info!(profile = %id, model = ?old, "Swedish model lineup: kept profile model because the recommended model is not downloaded");
                    }
                }
                FileModelPreference::Auto
                    if !target_ready && (env.whisper_downloaded)(previous_auto) =>
                {
                    tracing::info!(profile = %id, model = ?previous_auto, "Swedish model lineup: pinned Auto profile to its previous model because the recommended model is not downloaded");
                    self.profile_models
                        .insert(id, FileModelPreference::Whisper(previous_auto));
                }
                _ => {}
            }
        }
        // Legacy global choice (rollback compatibility): a Whisper field
        // cannot hold Pianissimo, so it only ever moves to KB-Whisper Medium.
        if !self.auto_select_model && self.whisper_model.is_hidden_in_app() {
            let medium = WhisperModel::KbWhisperMedium;
            if (env.whisper_downloaded)(medium) || !(env.whisper_downloaded)(self.whisper_model) {
                tracing::info!(from = ?self.whisper_model, to = ?medium, "Swedish model lineup: moved legacy global model");
                self.whisper_model = medium;
            }
        }
        true
    }

    pub fn default_profile(&self) -> HotkeyProfile {
        let profiles = self.resolved_hotkey_profiles();
        profiles[HotkeyProfile::default_index(&profiles)].clone()
    }

    /// Preserve the effective live model of every existing profile before
    /// global model preferences stop being consulted for live dictation.
    /// Applied by settings migration step 3; never replaces a profile's model.
    pub fn materialize_profile_models(&mut self) {
        if self.hotkey_profiles.is_empty() {
            self.hotkey_profiles.push(HotkeyProfile::legacy_default(
                self.hotkey.clone(),
                self.language,
            ));
        }
        for profile in &self.hotkey_profiles {
            let preference = if self.pianissimo_dictation && profile.language == Language::Swedish {
                FileModelPreference::PianissimoOriginal
            } else if self.auto_select_model
                || !self.whisper_model.is_compatible_with(profile.language)
            {
                FileModelPreference::Auto
            } else {
                FileModelPreference::Whisper(self.whisper_model)
            };
            self.profile_models
                .entry(profile.id.clone())
                .or_insert(preference);
        }
    }

    /// Copy the global dictionary's entries into every profile dictionary as
    /// hints, once (`profile_glossary_migrated`). Applied by settings
    /// migration step 4.
    pub fn migrate_global_glossary_to_profiles(&mut self) {
        if self.profile_glossary_migrated {
            return;
        }
        let hints: Vec<String> = Glossary::parse(&self.initial_prompt)
            .entries()
            .iter()
            .map(|entry| entry.canonical.clone())
            .collect();
        for profile in self.resolved_hotkey_profiles() {
            let source = self.profile_glossaries.entry(profile.id).or_default();
            let mut known: HashSet<String> = Glossary::parse(source)
                .entries()
                .iter()
                .map(|entry| entry.canonical.to_lowercase())
                .collect();
            for hint in &hints {
                if known.insert(hint.to_lowercase()) {
                    if !source.trim().is_empty() {
                        source.push('\n');
                    }
                    source.push_str(hint);
                }
            }
        }
        self.profile_glossary_migrated = true;
    }

    /// Resolve the live engine for one profile. Unmigrated settings retain
    /// their previous global selection until the profile migration is saved.
    /// `pianissimo_supported` is the Pianissimo platform gate;
    /// `Settings::dictation_model_for_profile` asks this machine.
    pub fn dictation_model_for_profile_gated(
        &self,
        profile_id: &str,
        pianissimo_supported: bool,
    ) -> Result<FileModel, SettingsError> {
        let profile = self
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| SettingsError::UnknownProfile {
                id: profile_id.to_string(),
            })?;
        let preference = self
            .profile_models
            .get(profile_id)
            .copied()
            .unwrap_or_else(|| {
                if self.uses_pianissimo_for_dictation(profile.language) {
                    FileModelPreference::PianissimoOriginal
                } else if self.auto_select_model
                    || !self.whisper_model.is_compatible_with(profile.language)
                {
                    FileModelPreference::Auto
                } else {
                    FileModelPreference::Whisper(self.whisper_model)
                }
            });
        match preference {
            FileModelPreference::Auto => Ok(FileModel::recommended_for(
                profile.language,
                pianissimo_supported,
            )),
            FileModelPreference::Whisper(model) if model.is_compatible_with(profile.language) => {
                Ok(FileModel::Whisper(model))
            }
            FileModelPreference::Whisper(model) => {
                Err(SettingsError::ModelIncompatibleWithProfile {
                    model: model.display_name().to_string(),
                    profile_id: profile_id.to_string(),
                })
            }
            FileModelPreference::PianissimoOriginal if profile.language == Language::Swedish => {
                Ok(FileModel::PianissimoOriginal)
            }
            FileModelPreference::PianissimoOriginal => {
                Err(SettingsError::PianissimoSwedishProfileOnly {
                    profile_id: profile_id.to_string(),
                })
            }
        }
    }

    /// [`Self::dictation_model_for_profile_gated`] as the Whisper model and
    /// whether the profile uses Pianissimo instead.
    /// `Settings::live_model_for_profile` asks this machine for the gate.
    pub fn live_model_for_profile_gated(
        &self,
        profile_id: &str,
        pianissimo_supported: bool,
    ) -> Result<(WhisperModel, bool), SettingsError> {
        match self.dictation_model_for_profile_gated(profile_id, pianissimo_supported)? {
            FileModel::Whisper(model) => Ok((model, false)),
            // The Whisper slot is unused when the flag is set; keep the Whisper
            // recommendation as a harmless placeholder.
            FileModel::PianissimoOriginal => {
                Ok((WhisperModel::recommended(Language::Swedish), true))
            }
        }
    }

    /// True when `profile_id`'s live dictation model is Pianissimo, given the
    /// platform gate. `Settings::profile_uses_pianissimo` asks this machine.
    pub fn profile_uses_pianissimo_gated(
        &self,
        profile_id: &str,
        pianissimo_supported: bool,
    ) -> bool {
        self.live_model_for_profile_gated(profile_id, pianissimo_supported)
            .is_ok_and(|(_, pianissimo)| pianissimo)
    }

    /// True when any dictation profile's live model is Pianissimo, given the
    /// platform gate. `Settings::any_profile_uses_pianissimo` asks this machine.
    pub fn any_profile_uses_pianissimo_gated(&self, pianissimo_supported: bool) -> bool {
        self.resolved_hotkey_profiles()
            .iter()
            .any(|profile| self.profile_uses_pianissimo_gated(&profile.id, pianissimo_supported))
    }

    /// Choose a profile's live model; selecting Pianissimo is refused unless
    /// `pianissimo_supported`. `Settings::set_profile_model` asks this machine.
    pub fn set_profile_model_gated(
        &mut self,
        profile_id: &str,
        preference: FileModelPreference,
        pianissimo_supported: bool,
    ) -> Result<(), SettingsError> {
        if preference == FileModelPreference::PianissimoOriginal && !pianissimo_supported {
            return Err(SettingsError::PianissimoUnsupported);
        }
        let profile = self
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| SettingsError::UnknownProfile {
                id: profile_id.to_string(),
            })?;
        match preference {
            FileModelPreference::Whisper(model) if !model.is_compatible_with(profile.language) => {
                return Err(SettingsError::ModelIncompatibleWithProfile {
                    model: model.display_name().to_string(),
                    profile_id: profile_id.to_string(),
                });
            }
            FileModelPreference::PianissimoOriginal if profile.language != Language::Swedish => {
                return Err(SettingsError::PianissimoRequiresSwedishProfile);
            }
            _ => {}
        }
        self.profile_models
            .insert(profile_id.to_string(), preference);
        Ok(())
    }

    /// Settings migration for systems that cannot run Pianissimo (anything but
    /// macOS 14+ on Apple Silicon or Windows on ARM): every selection of it falls back to the
    /// recommended Whisper model (`Auto`). Returns true when anything changed.
    /// Warns once for each selection it demotes: the startup load's notice.
    pub fn demote_unsupported_pianissimo(&mut self, pianissimo_supported: bool) -> bool {
        self.demote_unsupported_pianissimo_logging(pianissimo_supported, true)
    }

    /// [`Self::demote_unsupported_pianissimo`] without the warnings, for a copy
    /// that is demoted again after each write (the app's controller): the
    /// startup load has said that this host cannot run the engine, and saying
    /// it again with every setter would only repeat it.
    pub fn demote_unsupported_pianissimo_quietly(&mut self, pianissimo_supported: bool) -> bool {
        self.demote_unsupported_pianissimo_logging(pianissimo_supported, false)
    }

    fn demote_unsupported_pianissimo_logging(
        &mut self,
        pianissimo_supported: bool,
        warn: bool,
    ) -> bool {
        if pianissimo_supported {
            return false;
        }
        let mut changed = false;
        for (profile, preference) in self.profile_models.iter_mut() {
            if *preference == FileModelPreference::PianissimoOriginal {
                if warn {
                    tracing::warn!(
                        "Pianissimo is not supported on this system; profile '{profile}' now uses the recommended Whisper model"
                    );
                }
                *preference = FileModelPreference::Auto;
                changed = true;
            }
        }
        if self.file_transcription_model == FileModelPreference::PianissimoOriginal {
            if warn {
                tracing::warn!(
                    "Pianissimo is not supported on this system; file transcription now uses the recommended Whisper model"
                );
            }
            self.file_transcription_model = FileModelPreference::Auto;
            changed = true;
        }
        if self.pianissimo_dictation {
            if warn {
                tracing::warn!("Pianissimo is not supported on this system; disabling the legacy Pianissimo dictation flag");
            }
            self.pianissimo_dictation = false;
            changed = true;
        }
        changed
    }

    /// Change the default profile's language. A model the new language cannot
    /// use is reset to Auto; the returned notices describe every such reset.
    /// `pianissimo_supported` is the platform gate;
    /// `Settings::set_default_profile_language` asks this machine.
    pub fn set_default_profile_language_gated(
        &mut self,
        language: Language,
        pianissimo_supported: bool,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        let mut candidate = self.clone();
        let mut profiles = candidate.resolved_hotkey_profiles();
        let index = HotkeyProfile::default_index(&profiles);
        profiles[index].language = language;
        let notices = candidate.replace_hotkey_profiles_gated(profiles, pianissimo_supported)?;
        *self = candidate;
        Ok(notices)
    }

    /// Reset (to Auto) the model of every profile whose language differs from
    /// `previous` and whose explicit model cannot serve the new language.
    /// Other profiles are never touched. Returns one notice per reset.
    fn reset_models_incompatible_after_language_change(
        &mut self,
        previous: &[HotkeyProfile],
        supported: bool,
    ) -> Vec<ModelResetNotice> {
        let mut notices = Vec::new();
        for profile in self.resolved_hotkey_profiles() {
            let changed = previous
                .iter()
                .any(|old| old.id == profile.id && old.language != profile.language);
            if !changed {
                continue;
            }
            let reason = match self.profile_models.get(&profile.id) {
                Some(FileModelPreference::Whisper(model))
                    if !model.is_compatible_with(profile.language) =>
                {
                    format!(
                        "{} does not support {}",
                        model.display_name(),
                        profile.language.display_name()
                    )
                }
                Some(FileModelPreference::PianissimoOriginal)
                    if profile.language != Language::Swedish =>
                {
                    "Pianissimo only supports Swedish".to_string()
                }
                _ => continue,
            };
            let recommended = match FileModel::recommended_for(profile.language, supported) {
                FileModel::Whisper(model) => model.display_name(),
                FileModel::PianissimoOriginal => "Pianissimo",
            };
            self.profile_models
                .insert(profile.id.clone(), FileModelPreference::Auto);
            notices.push(ModelResetNotice {
                profile_id: profile.id.clone(),
                profile_name: profile.name.clone(),
                message: format!(
                    "Model for profile '{}' changed to Recommended ({recommended}) because {reason}.",
                    profile.name
                ),
            });
        }
        notices
    }

    /// Resolve the selected profile's dictionary. Unmigrated in-memory legacy
    /// settings keep their old hint-only global behavior until first load.
    pub fn effective_glossary_source(&self, profile_id: Option<&str>) -> String {
        self.effective_glossary_source_with_prompt(profile_id, None)
    }

    /// Compose the glossary source for one transcription without mutating
    /// stored settings. A non-empty one-run prompt replaces the saved global
    /// hint source; the selected known, explicit-language profile remains the
    /// only source of deterministic alias replacements.
    pub fn effective_glossary_source_with_prompt(
        &self,
        profile_id: Option<&str>,
        prompt: Option<&str>,
    ) -> String {
        if self.profile_glossary_migrated {
            let id = profile_id
                .map(str::to_string)
                .or_else(|| Some(self.default_profile().id));
            let scoped = id
                .as_deref()
                .and_then(|id| self.profile_glossaries.get(id))
                .map(String::as_str)
                .unwrap_or("");
            let auto_language = id.as_deref().is_some_and(|id| {
                self.resolved_hotkey_profiles()
                    .iter()
                    .any(|profile| profile.id == id && profile.language == Language::Auto)
            });
            // Auto-detect cannot safely apply deterministic replacements in an
            // unknown language, but its dictionary hints remain available.
            let auto_hints =
                auto_language.then(|| Glossary::parse(scoped).decoder_prompt().unwrap_or_default());
            let scoped = auto_hints.as_deref().unwrap_or(scoped);
            let prompt_hints = prompt
                .filter(|source| !source.trim().is_empty())
                .and_then(|source| Glossary::parse(source).decoder_prompt());
            return match prompt_hints.as_deref() {
                Some(source) if scoped.trim().is_empty() => source.to_string(),
                Some(source) => format!("{source}\n{scoped}"),
                None => scoped.to_string(),
            };
        }
        let base_source = prompt
            .filter(|candidate| !candidate.trim().is_empty())
            .unwrap_or(self.initial_prompt.as_str());
        let global = Glossary::parse(base_source)
            .decoder_prompt()
            .unwrap_or_default();
        let scoped_profile_id = profile_id.filter(|requested_id| {
            self.resolved_hotkey_profiles()
                .iter()
                .any(|profile| profile.id == **requested_id && profile.language != Language::Auto)
        });
        let scoped = scoped_profile_id
            .and_then(|id| self.profile_glossaries.get(id))
            .map(String::as_str)
            .map(str::trim)
            .unwrap_or_default();

        match (global.trim().is_empty(), scoped.is_empty()) {
            (true, true) => String::new(),
            (false, true) => global,
            (true, false) => scoped.to_string(),
            (false, false) => format!("{}\n{scoped}", global.trim()),
        }
    }

    /// Legacy model helper for settings predating profile-owned model choices.
    pub fn effective_model(&self) -> WhisperModel {
        self.effective_model_for(self.language)
    }

    pub fn effective_model_for(&self, language: Language) -> WhisperModel {
        if self.auto_select_model || !self.whisper_model.is_compatible_with(language) {
            WhisperModel::recommended(language)
        } else {
            self.whisper_model
        }
    }

    /// Legacy pre-migration Pianissimo switch, consulted only when a profile
    /// has not yet received its own model preference.
    pub fn uses_pianissimo_for_dictation(&self, language: Language) -> bool {
        self.pianissimo_dictation && language == Language::Swedish
    }

    /// Resolve the model for file transcription without changing the live
    /// dictation model preference, given the Pianissimo platform gate and a
    /// model-readiness probe (`Settings::effective_file_model_for` asks this
    /// machine for both). An `Auto` preference prefers the recommended model
    /// when it is downloaded, else the best downloaded compatible model.
    ///
    /// `Auto`: the recommended model if `is_ready`; otherwise the first ready
    /// model in the language's app lineup order, then other compatible Whisper
    /// models from largest to smallest; if none is ready, the recommended model
    /// (so "not downloaded" errors name the recommended download).
    pub fn effective_file_model_for_gated(
        &self,
        language: Language,
        pianissimo_supported: bool,
        is_ready: &dyn Fn(FileModel) -> bool,
    ) -> Result<FileModel, SettingsError> {
        match self.file_transcription_model {
            FileModelPreference::Auto => Ok(auto_file_model(
                language,
                pianissimo_supported,
                true,
                is_ready,
            )),
            _ => self.explicit_file_model_for(language),
        }
    }

    fn explicit_file_model_for(&self, language: Language) -> Result<FileModel, SettingsError> {
        match self.file_transcription_model {
            FileModelPreference::Auto => unreachable!("Auto is resolved by the caller"),
            FileModelPreference::Whisper(model) if model.is_compatible_with(language) => {
                Ok(FileModel::Whisper(model))
            }
            FileModelPreference::Whisper(model) => {
                Err(SettingsError::ModelIncompatibleWithLanguage {
                    model: model.display_name().to_string(),
                    language: language.display_name().to_string(),
                })
            }
            FileModelPreference::PianissimoOriginal if language == Language::Swedish => {
                Ok(FileModel::PianissimoOriginal)
            }
            FileModelPreference::PianissimoOriginal => {
                Err(SettingsError::PianissimoSwedishFilesOnly)
            }
        }
    }

    /// Like [`Self::effective_file_model_for_gated`], but for work that needs a
    /// Whisper model (meetings, diarization, decoder hints, beam search): an
    /// `Auto` preference never resolves to Pianissimo. An explicit Pianissimo
    /// choice is returned as-is so callers can report it clearly.
    /// `Settings::effective_whisper_file_model_for` asks this machine for the
    /// readiness probe.
    pub fn effective_whisper_file_model_for_gated(
        &self,
        language: Language,
        is_ready: &dyn Fn(FileModel) -> bool,
    ) -> Result<FileModel, SettingsError> {
        match self.file_transcription_model {
            FileModelPreference::Auto => Ok(auto_file_model(language, false, false, is_ready)),
            _ => self.explicit_file_model_for(language),
        }
    }

    pub fn resolved_hotkey_profiles(&self) -> Vec<HotkeyProfile> {
        if self.hotkey_profiles.is_empty() {
            vec![HotkeyProfile::legacy_default(
                self.hotkey.clone(),
                self.language,
            )]
        } else {
            self.hotkey_profiles.clone()
        }
    }

    /// Return the profile bindings that should be registered at runtime.
    /// Explicit push-to-talk/toggle bindings replace the legacy binding for a
    /// profile.
    pub fn resolved_hotkey_bindings(&self) -> Vec<(HotkeyProfile, String, HotkeyMode)> {
        self.resolved_hotkey_profiles()
            .into_iter()
            .flat_map(|profile| {
                let mut bindings = Vec::with_capacity(2);
                if let Some(shortcut) = &profile.push_to_talk_shortcut {
                    bindings.push((profile.clone(), shortcut.clone(), HotkeyMode::PushToTalk));
                }
                if let Some(shortcut) = &profile.toggle_shortcut {
                    bindings.push((profile.clone(), shortcut.clone(), HotkeyMode::Toggle));
                }
                if bindings.is_empty() {
                    bindings.push((profile.clone(), profile.shortcut.clone(), self.hotkey_mode));
                }
                bindings
            })
            .collect()
    }

    /// Return all shortcuts that the active hotkey mode may register.
    pub fn resolved_shortcuts(&self) -> Vec<String> {
        self.resolved_hotkey_bindings()
            .into_iter()
            .map(|(_, shortcut, _)| shortcut)
            .collect()
    }

    pub fn validate_hotkey_profiles(profiles: &[HotkeyProfile]) -> Result<(), SettingsError> {
        if profiles.is_empty() {
            return Err(SettingsError::NoProfiles);
        }
        let mut ids = HashSet::new();
        let mut shortcuts = HashSet::new();
        for profile in profiles {
            if profile.id.is_empty()
                || profile.id.len() > 32
                || !profile.id.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !profile
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
            {
                return Err(SettingsError::InvalidProfileId {
                    id: profile.id.clone(),
                });
            }
            if !ids.insert(profile.id.clone()) {
                return Err(SettingsError::DuplicateProfileId {
                    id: profile.id.clone(),
                });
            }
            if profile.name.trim().is_empty() {
                return Err(SettingsError::ProfileNameMissing {
                    profile_id: profile.id.clone(),
                });
            }
            if profile.name.chars().count() > 40 {
                return Err(SettingsError::ProfileNameTooLong {
                    profile_id: profile.id.clone(),
                });
            }
            validate_hotkey(&profile.shortcut)?;
            let has_explicit_bindings =
                profile.push_to_talk_shortcut.is_some() || profile.toggle_shortcut.is_some();
            let active_shortcuts = if has_explicit_bindings {
                profile
                    .push_to_talk_shortcut
                    .iter()
                    .chain(profile.toggle_shortcut.iter())
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            } else {
                vec![profile.shortcut.as_str()]
            };
            for shortcut in active_shortcuts {
                if shortcut.trim().is_empty() {
                    return Err(SettingsError::EmptyProfileHotkey {
                        profile_id: profile.id.clone(),
                    });
                }
                validate_hotkey(shortcut)?;
                let canonical = canonical_hotkey(shortcut)?;
                if !shortcuts.insert(canonical) {
                    return Err(SettingsError::DuplicateHotkey {
                        shortcut: shortcut.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Validate the complete shortcut configuration.
    pub fn validate_shortcut_configuration(&self) -> Result<(), SettingsError> {
        let profiles = self.resolved_hotkey_profiles();
        Self::validate_hotkey_profiles(&profiles)?;
        let shortcuts = self
            .resolved_hotkey_bindings()
            .into_iter()
            .map(|(_, shortcut, _)| canonical_hotkey(&shortcut))
            .collect::<Result<HashSet<_>, _>>()?;
        if shortcuts.len() != self.resolved_hotkey_bindings().len() {
            return Err(SettingsError::DuplicateHotkeyBindings);
        }
        Ok(())
    }

    pub fn replace_hotkey_mode(&mut self, hotkey_mode: HotkeyMode) -> Result<(), SettingsError> {
        let mut candidate = self.clone();
        candidate.hotkey_mode = hotkey_mode;
        candidate.validate_shortcut_configuration()?;
        self.hotkey_mode = hotkey_mode;
        Ok(())
    }

    /// Replace the profile list. When a profile's language changes and its explicit
    /// model no longer fits, that profile's model is reset to Auto and a notice
    /// is returned instead of an error. `pianissimo_supported` is the platform
    /// gate; `Settings::replace_hotkey_profiles` asks this machine.
    pub fn replace_hotkey_profiles_gated(
        &mut self,
        profiles: Vec<HotkeyProfile>,
        pianissimo_supported: bool,
    ) -> Result<Vec<ModelResetNotice>, SettingsError> {
        let mut profiles = profiles;
        for profile in &mut profiles {
            if let Some(shortcut) = profile
                .push_to_talk_shortcut
                .as_ref()
                .or(profile.toggle_shortcut.as_ref())
            {
                profile.shortcut = shortcut.clone();
            }
        }
        Self::validate_hotkey_profiles(&profiles)?;
        let current_profiles = self.resolved_hotkey_profiles();
        if let Some(profile) = profiles.iter().find(|profile| {
            current_profiles.iter().any(|current| {
                current.id == profile.id
                    && current.language != profile.language
                    && self
                        .profile_glossaries
                        .get(&profile.id)
                        .is_some_and(|source| {
                            Glossary::parse(source)
                                .entries()
                                .iter()
                                .any(|entry| !entry.aliases.is_empty())
                        })
            })
        }) {
            return Err(SettingsError::DictionaryBlocksLanguageChange {
                profile_id: profile.id.clone(),
            });
        }
        let current_ids: HashSet<&str> = current_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        if let Some(profile) = profiles.iter().find(|profile| {
            !current_ids.contains(profile.id.as_str())
                && self
                    .profile_glossaries
                    .get(&profile.id)
                    .is_some_and(|source| !source.trim().is_empty())
        }) {
            return Err(SettingsError::InactiveProfileDictionary {
                profile_id: profile.id.clone(),
            });
        }
        let legacy = &profiles[HotkeyProfile::default_index(&profiles)];
        let mut candidate = self.clone();
        candidate.hotkey = legacy.shortcut.clone();
        candidate.language = legacy.language;
        candidate.hotkey_profiles = profiles;
        let active_ids: HashSet<&str> = candidate
            .hotkey_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        candidate
            .profile_models
            .retain(|id, _| active_ids.contains(id.as_str()));
        for profile in &candidate.hotkey_profiles {
            candidate
                .profile_models
                .entry(profile.id.clone())
                .or_insert(FileModelPreference::Auto);
        }
        let notices = candidate.reset_models_incompatible_after_language_change(
            &current_profiles,
            pianissimo_supported,
        );
        for profile in &candidate.hotkey_profiles {
            candidate.dictation_model_for_profile_gated(&profile.id, pianissimo_supported)?;
        }
        candidate.validate_shortcut_configuration()?;
        self.hotkey = candidate.hotkey;
        self.language = candidate.language;
        self.hotkey_profiles = candidate.hotkey_profiles;
        self.profile_models = candidate.profile_models;
        Ok(notices)
    }

    pub fn hotkey_profile_for_shortcut(&self, shortcut: &str) -> Option<HotkeyProfile> {
        let target = canonical_hotkey(shortcut).ok()?;
        self.resolved_hotkey_bindings()
            .into_iter()
            .find(|(_, binding, _)| {
                canonical_hotkey(binding).ok().as_deref() == Some(target.as_str())
            })
            .map(|(profile, _, _)| profile)
    }

    pub fn set_legacy_language(&mut self, language: Language) -> Result<(), SettingsError> {
        let current_legacy_language = if self.hotkey_profiles.is_empty() {
            Some(self.language)
        } else {
            self.hotkey_profiles
                .iter()
                .find(|profile| profile.id == "default")
                .map(|profile| profile.language)
        };
        if current_legacy_language.is_some_and(|current| current != language)
            && self
                .profile_glossaries
                .get("default")
                .is_some_and(|source| {
                    Glossary::parse(source)
                        .entries()
                        .iter()
                        .any(|entry| !entry.aliases.is_empty())
                })
        {
            return Err(SettingsError::DictionaryBlocksLanguageChange {
                profile_id: "default".to_string(),
            });
        }
        self.language = language;
        if let Some(profile) = self
            .hotkey_profiles
            .iter_mut()
            .find(|profile| profile.id == "default")
        {
            profile.language = language;
        }
        Ok(())
    }

    /// Try to update the legacy/default profile shortcut atomically. The default
    /// profile is the one called `default`, else the first
    /// ([`HotkeyProfile::default_index`]): with no profile called `default`
    /// this used to store the shortcut in `hotkey` alone, which no binding
    /// reads, so `config set hotkey` changed nothing that fires (#373).
    pub fn try_set_legacy_hotkey(&mut self, shortcut: String) -> Result<(), SettingsError> {
        let mut candidate = self.clone();
        candidate.hotkey = shortcut.clone();
        if !candidate.hotkey_profiles.is_empty() {
            let index = HotkeyProfile::default_index(&candidate.hotkey_profiles);
            candidate.hotkey_profiles[index].set_primary_shortcut(shortcut);
        }
        candidate.validate_shortcut_configuration()?;
        self.hotkey = candidate.hotkey;
        self.hotkey_profiles = candidate.hotkey_profiles;
        Ok(())
    }

    /// Checked legacy/default profile shortcut update. Invalid or colliding
    /// updates leave the complete settings value unchanged.
    pub fn set_legacy_hotkey(&mut self, shortcut: String) -> Result<(), SettingsError> {
        self.try_set_legacy_hotkey(shortcut)
    }

    /// Build the ordered set of profile models worth loading during GUI
    /// startup. Pianissimo profiles are excluded. The primary remaining Whisper
    /// profile is always first and included. Additional distinct models must fit both
    /// the resident-entry limit and advertised model-size budget.
    /// `pianissimo_supported` is the platform gate; `Settings::warm_model_plan`
    /// asks this machine.
    pub fn warm_model_plan_gated(
        &self,
        max_models: usize,
        max_total_mb: u32,
        pianissimo_supported: bool,
    ) -> Vec<(WhisperModel, Language)> {
        let profiles: Vec<_> = self
            .resolved_hotkey_profiles()
            .into_iter()
            .filter(|profile| {
                matches!(
                    self.dictation_model_for_profile_gated(&profile.id, pianissimo_supported),
                    Ok(FileModel::Whisper(_))
                )
            })
            .collect();
        let Some(primary_index) = profiles
            .iter()
            .position(|profile| profile.id == "default")
            .or_else(|| (!profiles.is_empty()).then_some(0))
        else {
            return Vec::new();
        };

        let capacity = max_models.max(1);
        let ordered_indices = std::iter::once(primary_index)
            .chain((0..profiles.len()).filter(|index| *index != primary_index));
        let mut plan = Vec::with_capacity(capacity.min(profiles.len()));
        let mut total_mb = 0u32;

        for index in ordered_indices {
            let profile = &profiles[index];
            let Ok(FileModel::Whisper(model)) =
                self.dictation_model_for_profile_gated(&profile.id, pianissimo_supported)
            else {
                continue;
            };
            if plan.iter().any(|(resident, _)| *resident == model) {
                continue;
            }

            let is_primary = plan.is_empty();
            let next_total = total_mb.saturating_add(model.size_mb());
            if !is_primary && (plan.len() >= capacity || next_total > max_total_mb) {
                continue;
            }

            plan.push((model, profile.language));
            total_mb = next_total;
        }

        plan
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
