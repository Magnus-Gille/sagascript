//! The CLI/app parity table (#373): every settings operation, from a starting
//! `Settings`, with the state it must leave behind.
//!
//! `sagascript config set ...` and the app's setters are two entry points to
//! the same user intentions. Each of them has to persist what the shared
//! operation in `settings::ops` (or the plain field assignment, for the
//! settings that are one) says, and nothing else. This table says it once:
//! [`cases`] lists the operations and the settings they start from,
//! [`expected`] computes what a surface must have persisted with core alone, and
//! [`assert_persisted`] compares.
//!
//! The CLI (`tests/settings_parity.rs`, which runs the real `sagascript config`)
//! and the app (`commands/persist_parity_tests.rs`, which runs the real setter
//! functions on Tauri's mock runtime) each translate a [`Case`] into their own
//! call and check themselves against this table, so the two are equal to each
//! other through core without either depending on the other (the way
//! `run_context_table` compares the run resolvers, #346).
//!
//! The surfaces differ on purpose in two places: `auto_paste` with a missing
//! Accessibility permission (the app asks the OS and stores `false`, the CLI
//! cannot ask and stores the request) and `initial_prompt` over a dictionary
//! that has entries (the CLI merges, the app replaces). `Case::differs_in` names
//! the fields, and the test below pins the list.
//!
//! Some rows start from a settings *file* of an older release
//! ([`Case::legacy_file`]): both surfaces must migrate it when they write, and end
//! where the shared operation on the migrated settings ends. `Reset` rows are
//! `sagascript config reset KEY`; the app has no reset, so its side is the setter
//! with the key's default value, which is what resetting means.

use serde_json::Value;

use crate::glossary::Glossary;
use crate::settings::{
    Accessibility, EnginePrewarm, FileModelPreference, HotkeyMode, HotkeyProfile, Language,
    Settings, WhisperModel,
};

/// The two entry points to a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Cli,
    App,
}

/// A change to the profile list, as `sagascript config profiles` takes it. The
/// app sends the whole resulting list, so [`ProfileEdit::resulting`] is the list
/// both end up applying.
///
/// A model given in the same edit is chosen in the same step by the CLI, which
/// refuses the whole edit if the model does not fit; the app chooses it with a
/// command of its own after the list. The table therefore holds only edits whose
/// model fits, where the two end in the same state; the refusal is tested in
/// the CLI (`profiles_update_language_with_incompatible_explicit_model_errors_without_change`).
#[derive(Debug, Clone)]
pub enum ProfileEdit {
    Create {
        id: &'static str,
        name: &'static str,
        shortcut: &'static str,
        language: Language,
        model: Option<FileModelPreference>,
    },
    Update {
        id: &'static str,
        shortcut: Option<&'static str>,
        language: Option<Language>,
        model: Option<FileModelPreference>,
    },
    Remove {
        id: &'static str,
    },
}

/// The list the edit leaves and the model it chooses in the same step.
pub(crate) type ProfileChange = (Vec<HotkeyProfile>, Option<(String, FileModelPreference)>);

impl ProfileEdit {
    /// The profile list after the edit, and the model it chooses, from the
    /// settings it starts from. An unknown id is an error, as in the CLI.
    pub fn resulting(&self, start: &Settings) -> Result<ProfileChange, String> {
        let mut profiles = start.resolved_hotkey_profiles();
        match self {
            ProfileEdit::Create {
                id,
                name,
                shortcut,
                language,
                model,
            } => {
                if profiles.iter().any(|profile| profile.id == *id) {
                    return Err(format!("Profile '{id}' already exists"));
                }
                profiles.push(profile(id, name, shortcut, *language));
                Ok((profiles, model.map(|model| (id.to_string(), model))))
            }
            ProfileEdit::Update {
                id,
                shortcut,
                language,
                model,
            } => {
                let profile = profiles
                    .iter_mut()
                    .find(|profile| profile.id == *id)
                    .ok_or_else(|| format!("Unknown profile '{id}'"))?;
                if let Some(shortcut) = shortcut {
                    profile.set_primary_shortcut(shortcut.to_string());
                }
                if let Some(language) = language {
                    profile.language = *language;
                }
                Ok((profiles, model.map(|model| (id.to_string(), model))))
            }
            ProfileEdit::Remove { id } => {
                let before = profiles.len();
                profiles.retain(|profile| profile.id != *id);
                if profiles.len() == before {
                    return Err(format!("Unknown profile '{id}'"));
                }
                Ok((profiles, None))
            }
        }
    }
}

/// One user intention.
#[derive(Debug, Clone)]
pub enum Op {
    Language(Language),
    WhisperModel(WhisperModel),
    AutoSelectModel(bool),
    PianissimoDictation(bool),
    /// `permission` is what the app's probe finds; the CLI never asks.
    AutoPaste {
        enabled: bool,
        permission: Accessibility,
    },
    VadEnabled(bool),
    ShowOverlay(bool),
    BeamSize(u32),
    TemperatureFallback(bool),
    EnginePrewarm(EnginePrewarm),
    EngineIdleUnloadMinutes(u32),
    FileTranscriptionModel(FileModelPreference),
    HotkeyMode(HotkeyMode),
    /// The default profile's shortcut (`config set hotkey`, the app's `set_hotkey`).
    Hotkey(&'static str),
    Profiles(ProfileEdit),
    /// One profile's model (`config profiles update ID --model`, `set_profile_model`).
    ProfileModel {
        id: &'static str,
        model: FileModelPreference,
    },
    /// Add a term, with aliases, to a profile's dictionary (`glossary add TERM
    /// --alias A --profile ID`). The app's dictionary editor saves the whole text,
    /// so its side is `set_profile_glossary` with the entry added to the text it
    /// read.
    DictionaryAdd {
        profile: &'static str,
        term: &'static str,
        aliases: &'static [&'static str],
    },
    /// `config set initial_prompt TEXT` and the app's `set_initial_prompt`: the
    /// CLI merges the entries into the default profile's dictionary, the app
    /// replaces the dictionary.
    InitialPrompt(&'static str),
    /// `config reset KEY`: the shared operation of `KEY` with its default value
    /// ([`reset_as_op`]). Only for the keys where that is what resetting does.
    Reset(&'static str),
}

/// The default shortcut, which `config reset hotkey` goes back to.
pub const DEFAULT_HOTKEY: &str = "Control+Shift+Space";

/// The shared operation that `config reset KEY` means: the setter of `KEY` with
/// the default value of `Settings::default()`. The legacy model keys
/// (`whisper_model`, `auto_select_model`, `pianissimo_dictation`) are not here:
/// their reset puts the profile's model back to Auto and moves one legacy field,
/// which no setter does (ARCHITECTURE.md, left for part 2).
pub fn reset_as_op(key: &str) -> Op {
    let defaults = Settings::default();
    match key {
        "language" => Op::Language(defaults.language),
        "hotkey_mode" => Op::HotkeyMode(defaults.hotkey_mode),
        "show_overlay" => Op::ShowOverlay(defaults.show_overlay),
        "auto_paste" => Op::AutoPaste {
            enabled: defaults.auto_paste,
            permission: Accessibility::Granted,
        },
        "vad_enabled" => Op::VadEnabled(defaults.vad_enabled),
        "beam_size" => Op::BeamSize(defaults.beam_size),
        "temperature_fallback" => Op::TemperatureFallback(defaults.temperature_fallback),
        "engine_prewarm" => Op::EnginePrewarm(defaults.engine_prewarm),
        "engine_idle_unload_minutes" => {
            Op::EngineIdleUnloadMinutes(defaults.engine_idle_unload_minutes)
        }
        "file_transcription_model" => Op::FileTranscriptionModel(defaults.file_transcription_model),
        "hotkey" => Op::Hotkey(DEFAULT_HOTKEY),
        other => panic!("`config reset {other}` is not the setter with the default value"),
    }
}

/// An operation, the settings it starts from, and where the surfaces differ.
#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    /// The settings the case starts from; ignored when there is a
    /// [`legacy_file`](Self::legacy_file).
    pub start: Settings,
    /// A settings file of an older release, verbatim, instead of `start`: it has
    /// steps pending that the surface must run when it writes.
    pub legacy_file: Option<String>,
    pub op: Op,
    /// The settings fields in which the two surfaces store different values for
    /// this case, by design. Empty for every case but one kind.
    pub differs_in: &'static [&'static str],
}

impl Case {
    /// The text of the settings file the case starts from.
    pub fn file_text(&self) -> String {
        self.legacy_file.clone().unwrap_or_else(|| {
            serde_json::to_string_pretty(&self.start).expect("settings serialize")
        })
    }

    /// The settings as a surface loads that file, before it runs (`store::load_from`:
    /// the steps that need no machine state are applied).
    pub fn loaded_start(&self) -> Settings {
        let Some(text) = &self.legacy_file else {
            return self.start.clone();
        };
        let dir = tempfile::tempdir().expect("a temporary directory");
        let path = dir.path().join("sagascript-settings.json");
        std::fs::write(&path, text).expect("write the legacy file");
        crate::settings::store::load_from(&path)
    }
}

fn profile(id: &str, name: &str, shortcut: &str, language: Language) -> HotkeyProfile {
    HotkeyProfile {
        id: id.into(),
        name: name.into(),
        shortcut: shortcut.into(),
        language,
        push_to_talk_shortcut: None,
        toggle_shortcut: None,
    }
}

// -- The settings the cases start from. Each is what a saved file looks like
// -- after its migrations: every profile has a model of its own.

/// A fresh install after its first save: an English default profile on Auto.
pub fn english() -> Settings {
    Settings {
        hotkey_profiles: vec![profile(
            "default",
            "Default",
            "Control+Shift+Space",
            Language::English,
        )],
        profile_models: [("default".to_string(), FileModelPreference::Auto)].into(),
        profile_glossary_migrated: true,
        ..Settings::default()
    }
}

/// English, with a legacy model that English cannot use: turning automatic
/// selection off would pin it, and is refused.
pub(crate) fn english_with_an_unusable_legacy_model() -> Settings {
    Settings {
        whisper_model: WhisperModel::KbWhisperLarge,
        auto_select_model: false,
        ..english()
    }
}

/// A Norwegian default profile on a chosen model, the legacy fields in step.
pub(crate) fn norwegian_pinned() -> Settings {
    let mut settings = english();
    settings.language = Language::Norwegian;
    settings.hotkey_profiles[0].language = Language::Norwegian;
    settings.whisper_model = WhisperModel::NbWhisperBase;
    settings.auto_select_model = false;
    settings.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::NbWhisperSmall),
    );
    settings
}

/// A Swedish default profile on a chosen Whisper model and an English second
/// profile. Never Auto on Swedish in a row that changes a model: that is
/// Pianissimo where it runs, and the app's setter would then warm its engine.
/// `engine_prewarm` is off for the same reason, so that even the row that ends on
/// Auto (`pianissimo_dictation` false) wants no warm-up on any machine.
pub(crate) fn swedish_pinned() -> Settings {
    let mut settings = english();
    settings.engine_prewarm = EnginePrewarm::Off;
    settings.language = Language::Swedish;
    settings.hotkey_profiles[0].language = Language::Swedish;
    settings
        .hotkey_profiles
        .push(profile("work", "Work", "Super+W", Language::English));
    settings.whisper_model = WhisperModel::KbWhisperLarge;
    settings.auto_select_model = false;
    settings.profile_models.insert(
        "default".into(),
        FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
    );
    settings.profile_models.insert(
        "work".into(),
        FileModelPreference::Whisper(WhisperModel::BaseEn),
    );
    settings
}

/// A Swedish default profile whose dictionary has an alias: its language cannot
/// change.
pub(crate) fn swedish_with_a_dictionary() -> Settings {
    let mut settings = swedish_pinned();
    settings
        .profile_glossaries
        .insert("default".into(), "merge = merch".into());
    settings
}

/// Explicit profiles, none called `default`.
pub(crate) fn without_a_default_profile() -> Settings {
    Settings {
        language: Language::English,
        hotkey: "Super+E".into(),
        hotkey_profiles: vec![
            profile("english", "English", "Super+E", Language::English),
            profile("swedish", "Swedish", "Super+S", Language::Swedish),
        ],
        profile_models: [
            ("english".to_string(), FileModelPreference::Auto),
            (
                "swedish".to_string(),
                FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
            ),
        ]
        .into(),
        profile_glossary_migrated: true,
        ..Settings::default()
    }
}

/// The default profile has an explicit push-to-talk and toggle shortcut.
pub(crate) fn with_explicit_bindings() -> Settings {
    let mut settings = english();
    let default = &mut settings.hotkey_profiles[0];
    default.shortcut = "Super+D".into();
    default.push_to_talk_shortcut = Some("Super+D".into());
    default.toggle_shortcut = Some("Super+T".into());
    settings.hotkey = "Super+D".into();
    settings
}

/// English, with a second profile `auto` that detects its language.
pub(crate) fn with_an_auto_profile() -> Settings {
    let mut settings = english();
    settings
        .hotkey_profiles
        .push(profile("auto", "Auto", "Super+A", Language::Auto));
    settings
        .profile_models
        .insert("auto".into(), FileModelPreference::Auto);
    settings
}

/// English, with a dictionary on the default profile.
pub(crate) fn with_a_dictionary(source: &str) -> Settings {
    let mut settings = english();
    settings
        .profile_glossaries
        .insert("default".into(), source.into());
    settings
}

/// English, with the shortcut of the default profile (and `hotkey`) changed.
pub(crate) fn with_hotkey(shortcut: &str) -> Settings {
    let mut settings = english();
    settings.hotkey = shortcut.into();
    settings.hotkey_profiles[0].shortcut = shortcut.into();
    settings
}

/// The file an older release would have left: `english()` as a JSON object,
/// changed by `edit` to the shape of that release. The steps that need machine
/// state (`model_lineup_version`, `engine_defaults_version`) are recorded, so
/// what is pending is the same on every machine.
fn legacy_file(edit: impl FnOnce(&mut serde_json::Map<String, Value>)) -> String {
    let Value::Object(mut object) = serde_json::to_value(english()).expect("settings serialize")
    else {
        panic!("settings serialize to an object");
    };
    edit(&mut object);
    serde_json::to_string_pretty(&Value::Object(object)).expect("the file serializes")
}

/// A file from 1.3, before the profile models and the profile dictionaries: no
/// `schema_version`, a profile without a model, the hints not yet copied.
fn legacy_before_profile_models() -> String {
    legacy_file(|object| {
        object.remove("schema_version");
        object.insert("profile_models".into(), serde_json::json!({}));
        object.insert("profile_glossary_migrated".into(), Value::Bool(false));
        object.insert("pianissimo_dictation".into(), Value::Bool(false));
    })
}

/// A file at `schema_version` 3: only the profile dictionaries are pending.
fn legacy_at_version_3() -> String {
    legacy_file(|object| {
        object.insert("schema_version".into(), Value::from(3));
        object.insert("profile_glossary_migrated".into(), Value::Bool(false));
    })
}

/// A file a newer release wrote: this build writes it back with its own version.
fn from_a_newer_release() -> String {
    legacy_file(|object| {
        object.insert("schema_version".into(), Value::from(99));
        object.insert("a_setting_of_a_newer_release".into(), Value::Bool(true));
    })
}

/// English, every plain setting off its default.
pub fn customised() -> Settings {
    Settings {
        show_overlay: false,
        auto_paste: false,
        beam_size: 3,
        temperature_fallback: false,
        vad_enabled: true,
        engine_prewarm: EnginePrewarm::Off,
        engine_idle_unload_minutes: 7,
        ..english()
    }
}

/// The cases, in the order they are added.
struct Table(Vec<Case>);

impl Table {
    fn add(&mut self, name: &str, start: Settings, op: Op) -> &mut Case {
        self.0.push(Case {
            name: name.into(),
            start,
            legacy_file: None,
            op,
            differs_in: &[],
        });
        self.0.last_mut().expect("a case was just added")
    }

    fn add_legacy(&mut self, name: &str, file: String, op: Op) {
        self.add(name, english(), op).legacy_file = Some(file);
    }
}

/// Every operation the two surfaces share.
pub fn cases() -> Vec<Case> {
    use FileModelPreference::{Auto, Whisper};

    let mut table = Table(Vec::new());
    let mut add = |name: &str, start: Settings, op: Op| {
        table.add(name, start, op);
    };

    // The language: the legacy model fields start over, profile models the
    // language cannot use go back to Auto, and a dictionary with aliases refuses.
    for language in [Language::Swedish, Language::Norwegian, Language::Auto] {
        add(
            &format!("english default profile -> {language:?}"),
            english(),
            Op::Language(language),
        );
    }
    for language in [Language::Norwegian, Language::Swedish, Language::English] {
        add(
            &format!("norwegian profile on a chosen model -> {language:?}"),
            norwegian_pinned(),
            Op::Language(language),
        );
    }
    for language in [Language::English, Language::Norwegian, Language::Auto] {
        add(
            &format!("swedish profile on a chosen model -> {language:?}"),
            swedish_pinned(),
            Op::Language(language),
        );
    }
    add(
        "swedish profile with a dictionary -> English (refused)",
        swedish_with_a_dictionary(),
        Op::Language(Language::English),
    );
    add(
        "swedish profile with a dictionary -> Swedish",
        swedish_with_a_dictionary(),
        Op::Language(Language::Swedish),
    );
    add(
        "no profile called default -> Swedish",
        without_a_default_profile(),
        Op::Language(Language::Swedish),
    );

    // The default profile's model under its legacy names.
    for model in [
        WhisperModel::BaseEn,
        WhisperModel::Medium,
        WhisperModel::KbWhisperLarge,
    ] {
        add(
            &format!("english default profile, whisper_model {model:?}"),
            english(),
            Op::WhisperModel(model),
        );
    }
    for model in [WhisperModel::NbWhisperBase, WhisperModel::KbWhisperMedium] {
        add(
            &format!("norwegian profile on a chosen model, whisper_model {model:?}"),
            norwegian_pinned(),
            Op::WhisperModel(model),
        );
    }
    add(
        "swedish profile on a chosen model, whisper_model KbWhisperMedium",
        swedish_pinned(),
        Op::WhisperModel(WhisperModel::KbWhisperMedium),
    );
    for enabled in [true, false] {
        add(
            &format!("english default profile, auto_select_model {enabled}"),
            english(),
            Op::AutoSelectModel(enabled),
        );
        add(
            &format!("norwegian profile on a chosen model, auto_select_model {enabled}"),
            norwegian_pinned(),
            Op::AutoSelectModel(enabled),
        );
    }
    add(
        "english with an unusable legacy model, auto_select_model false (refused)",
        english_with_an_unusable_legacy_model(),
        Op::AutoSelectModel(false),
    );
    add(
        "swedish profile on a chosen model, auto_select_model false",
        swedish_pinned(),
        Op::AutoSelectModel(false),
    );
    add(
        "english default profile, pianissimo_dictation false",
        english(),
        Op::PianissimoDictation(false),
    );
    add(
        "english default profile, pianissimo_dictation true (refused: not Swedish)",
        english(),
        Op::PianissimoDictation(true),
    );
    add(
        "swedish profile on a chosen model, pianissimo_dictation false",
        swedish_pinned(),
        Op::PianissimoDictation(false),
    );

    // Auto-paste: where the surfaces differ on purpose.
    let off = Settings {
        auto_paste: false,
        ..english()
    };
    let on = Settings {
        auto_paste: true,
        ..english()
    };
    add(
        "auto_paste on, permission granted",
        off.clone(),
        Op::AutoPaste {
            enabled: true,
            permission: Accessibility::Granted,
        },
    );
    // Only this case differs: the app stores `false`, the CLI the request.
    table
        .add(
            "auto_paste on, permission missing",
            off,
            Op::AutoPaste {
                enabled: true,
                permission: Accessibility::Denied,
            },
        )
        .differs_in = &["auto_paste"];
    let mut add = |name: &str, start: Settings, op: Op| {
        table.add(name, start, op);
    };
    add(
        "auto_paste off, permission granted",
        on.clone(),
        Op::AutoPaste {
            enabled: false,
            permission: Accessibility::Granted,
        },
    );
    add(
        "auto_paste off, permission missing",
        on,
        Op::AutoPaste {
            enabled: false,
            permission: Accessibility::Denied,
        },
    );

    // Settings that are one field.
    for enabled in [true, false] {
        add(
            &format!("vad_enabled {enabled}"),
            customised(),
            Op::VadEnabled(enabled),
        );
    }
    add("show_overlay true", customised(), Op::ShowOverlay(true));
    add("beam_size 5", customised(), Op::BeamSize(5));
    add("beam_size 0", customised(), Op::BeamSize(0));
    add(
        "temperature_fallback true",
        customised(),
        Op::TemperatureFallback(true),
    );
    add(
        "engine_prewarm on_key_down",
        customised(),
        Op::EnginePrewarm(EnginePrewarm::OnKeyDown),
    );
    add(
        "engine_idle_unload_minutes 0",
        customised(),
        Op::EngineIdleUnloadMinutes(0),
    );
    add(
        "file_transcription_model auto",
        english(),
        Op::FileTranscriptionModel(Auto),
    );
    add(
        "file_transcription_model small",
        english(),
        Op::FileTranscriptionModel(Whisper(WhisperModel::Small)),
    );

    // Shortcuts.
    add(
        "hotkey_mode toggle",
        english(),
        Op::HotkeyMode(HotkeyMode::Toggle),
    );
    add(
        "hotkey_mode toggle, explicit bindings",
        with_explicit_bindings(),
        Op::HotkeyMode(HotkeyMode::Toggle),
    );
    add(
        "hotkey, default profile",
        english(),
        Op::Hotkey("Control+Alt+D"),
    );
    add(
        "hotkey, no profile called default",
        without_a_default_profile(),
        Op::Hotkey("Control+Alt+D"),
    );
    add(
        "hotkey, no profile called default, collides (refused)",
        without_a_default_profile(),
        Op::Hotkey("Super+S"),
    );
    add(
        "hotkey, explicit bindings",
        with_explicit_bindings(),
        Op::Hotkey("Control+Alt+Q"),
    );
    add(
        "hotkey, collides with another profile (refused)",
        swedish_pinned(),
        Op::Hotkey("Super+W"),
    );

    // Profiles.
    let created = |language, model| {
        Op::Profiles(ProfileEdit::Create {
            id: "p1",
            name: "P1",
            shortcut: "Super+1",
            language,
            model,
        })
    };
    add(
        "create a profile",
        english(),
        created(Language::English, None),
    );
    add(
        "create a Swedish profile on a chosen model",
        english(),
        created(
            Language::Swedish,
            Some(Whisper(WhisperModel::KbWhisperMedium)),
        ),
    );
    add(
        "create a profile with an id in use (refused)",
        english(),
        Op::Profiles(ProfileEdit::Create {
            id: "default",
            name: "Again",
            shortcut: "Super+2",
            language: Language::English,
            model: None,
        }),
    );
    let updated = |id, language, model| {
        Op::Profiles(ProfileEdit::Update {
            id,
            shortcut: None,
            language,
            model,
        })
    };
    add(
        "change another profile's language, its model goes back to Auto",
        swedish_pinned(),
        updated("work", Some(Language::Swedish), None),
    );
    add(
        "change the default profile's language, its model goes back to Auto",
        swedish_pinned(),
        updated("default", Some(Language::English), None),
    );
    add(
        "change the default profile's language and choose a model",
        swedish_pinned(),
        updated(
            "default",
            Some(Language::English),
            Some(Whisper(WhisperModel::BaseEn)),
        ),
    );
    add(
        "change a language when the dictionary has aliases (refused)",
        swedish_with_a_dictionary(),
        updated("default", Some(Language::English), None),
    );
    add(
        "change a profile's shortcut",
        swedish_pinned(),
        Op::Profiles(ProfileEdit::Update {
            id: "work",
            shortcut: Some("Super+Y"),
            language: None,
            model: None,
        }),
    );
    add(
        "change a profile's shortcut to one in use (refused)",
        swedish_pinned(),
        Op::Profiles(ProfileEdit::Update {
            id: "work",
            shortcut: Some("Super+S"),
            language: None,
            model: None,
        }),
    );
    add(
        "remove a profile",
        swedish_pinned(),
        Op::Profiles(ProfileEdit::Remove { id: "work" }),
    );
    add(
        "remove the default profile",
        swedish_pinned(),
        Op::Profiles(ProfileEdit::Remove { id: "default" }),
    );
    add(
        "remove the only profile (refused)",
        english(),
        Op::Profiles(ProfileEdit::Remove { id: "default" }),
    );
    add(
        "choose a profile's model",
        swedish_pinned(),
        Op::ProfileModel {
            id: "work",
            model: Whisper(WhisperModel::Small),
        },
    );
    add(
        "choose the default profile's model back to Auto",
        norwegian_pinned(),
        Op::ProfileModel {
            id: "default",
            model: Auto,
        },
    );
    add(
        "choose a model the profile cannot use (refused)",
        swedish_pinned(),
        Op::ProfileModel {
            id: "default",
            model: Whisper(WhisperModel::BaseEn),
        },
    );

    // The personal dictionary (glossary_ops): an entry added from the CLI, or
    // typed into the dictionary editor of the app, ends the same; a profile that
    // does not exist, or that detects its language and is given an alias, refuses.
    let mut add = |name: &str, start: Settings, op: Op| {
        table.add(name, start, op);
    };
    let entry = |profile, term, aliases| Op::DictionaryAdd {
        profile,
        term,
        aliases,
    };
    add(
        "dictionary: add a hint",
        english(),
        entry("default", "Grimnir", &[]),
    );
    add(
        "dictionary: add a term with an alias",
        with_a_dictionary("Alpha"),
        entry("default", "Grimnir", &["grimmer"]),
    );
    add(
        "dictionary: merge an alias into a term",
        with_a_dictionary("Grimnir = grimmer"),
        entry("default", "Grimnir", &["grimnar"]),
    );
    add(
        "dictionary: add to another profile",
        swedish_pinned(),
        entry("work", "Grimnir", &["grimmer"]),
    );
    add(
        "dictionary: a hint for a profile that detects its language",
        with_an_auto_profile(),
        entry("auto", "Grimnir", &[]),
    );
    add(
        "dictionary: an alias for a profile that detects its language (refused)",
        with_an_auto_profile(),
        entry("auto", "Grimnir", &["grimmer"]),
    );
    add(
        "dictionary: an unknown profile (refused)",
        english(),
        entry("ghost", "Grimnir", &[]),
    );
    add(
        "dictionary: an unknown profile with an alias (refused)",
        english(),
        entry("ghost", "Grimnir", &["grimmer"]),
    );
    // `config set initial_prompt` merges, the app replaces: where the dictionary
    // is empty they end alike, otherwise they differ in the dictionary (part 2).
    add(
        "initial_prompt on an empty dictionary",
        english(),
        Op::InitialPrompt("Grimnir = grimmer"),
    );
    table
        .add(
            "initial_prompt over a dictionary with entries",
            with_a_dictionary("Alpha = a"),
            Op::InitialPrompt("Beta"),
        )
        .differs_in = &["profile_glossaries"];
    let mut add = |name: &str, start: Settings, op: Op| {
        table.add(name, start, op);
    };

    // `config reset KEY` is the setter with the default value.
    for key in [
        "show_overlay",
        "auto_paste",
        "vad_enabled",
        "beam_size",
        "temperature_fallback",
        "engine_prewarm",
        "engine_idle_unload_minutes",
    ] {
        add(&format!("reset {key}"), customised(), Op::Reset(key));
    }
    add(
        "reset language, swedish profile on a chosen model",
        swedish_pinned(),
        Op::Reset("language"),
    );
    add(
        "reset language, the dictionary has aliases (refused)",
        swedish_with_a_dictionary(),
        Op::Reset("language"),
    );
    add(
        "reset hotkey_mode",
        Settings {
            hotkey_mode: HotkeyMode::Toggle,
            ..english()
        },
        Op::Reset("hotkey_mode"),
    );
    add(
        "reset file_transcription_model",
        Settings {
            file_transcription_model: Whisper(WhisperModel::Small),
            ..english()
        },
        Op::Reset("file_transcription_model"),
    );
    add("reset hotkey", with_hotkey("Super+X"), Op::Reset("hotkey"));
    add(
        "reset hotkey, no profile called default",
        without_a_default_profile(),
        Op::Reset("hotkey"),
    );
    // Not decided: the CLI leaves the explicit bindings, and so the shortcut,
    // where it was (`apply`, `Op::Reset("hotkey")`).
    table
        .add(
            "reset hotkey, explicit bindings",
            with_explicit_bindings(),
            Op::Reset("hotkey"),
        )
        .differs_in = &["hotkey", "hotkey_profiles"];
    let mut add = |name: &str, start: Settings, op: Op| {
        table.add(name, start, op);
    };
    let mut taken = swedish_pinned();
    taken.hotkey_profiles[1].shortcut = DEFAULT_HOTKEY.into();
    taken.hotkey = "Super+S".into();
    taken.hotkey_profiles[0].shortcut = "Super+S".into();
    add(
        "reset hotkey, the default shortcut is another profile's (refused)",
        taken,
        Op::Reset("hotkey"),
    );

    // A settings file of an older release, with steps pending: both surfaces
    // migrate it when they write, and then end where the operation on the
    // migrated settings ends.
    let mut legacy = |name: &str, file: String, op: Op| {
        table.add_legacy(name, file, op);
    };
    legacy(
        "file before profile models, beam_size",
        legacy_before_profile_models(),
        Op::BeamSize(5),
    );
    legacy(
        "file before profile models, language",
        legacy_before_profile_models(),
        Op::Language(Language::Norwegian),
    );
    legacy(
        "file before profile models, profile model",
        legacy_before_profile_models(),
        Op::ProfileModel {
            id: "default",
            model: Whisper(WhisperModel::BaseEn),
        },
    );
    legacy(
        "file before profile models, a new profile",
        legacy_before_profile_models(),
        created(Language::English, None),
    );
    legacy(
        "file at version 3, show_overlay",
        legacy_at_version_3(),
        Op::ShowOverlay(false),
    );
    legacy(
        "file at version 3, a dictionary entry",
        legacy_at_version_3(),
        entry("default", "Grimnir", &["grimmer"]),
    );
    legacy(
        "file at version 3, reset beam_size",
        legacy_at_version_3(),
        Op::Reset("beam_size"),
    );
    legacy(
        "file from a newer release, vad_enabled",
        from_a_newer_release(),
        Op::VadEnabled(true),
    );
    table.0
}

/// What `surface` must have persisted for `case`, computed with core alone from
/// `start` (the settings as the surface loads them). `Err` is a refusal: then
/// nothing may have been written.
pub fn expected(case: &Case, surface: Surface, start: &Settings) -> Result<Settings, String> {
    let mut settings = start.clone();
    apply(&case.op, surface, start, &mut settings)?;
    Ok(settings)
}

/// Apply `op` to `settings` (a copy of `start`) with the shared operations.
fn apply(
    op: &Op,
    surface: Surface,
    start: &Settings,
    settings: &mut Settings,
) -> Result<(), String> {
    match op {
        Op::Language(language) => {
            settings.apply_language_selection(*language)?;
        }
        Op::WhisperModel(model) => settings.set_whisper_model(*model)?,
        Op::AutoSelectModel(enabled) => settings.set_auto_select_model(*enabled)?,
        Op::PianissimoDictation(enabled) => settings.set_pianissimo_dictation(*enabled)?,
        Op::AutoPaste {
            enabled,
            permission,
        } => {
            let accessibility = match surface {
                Surface::Cli => Accessibility::Unknown,
                Surface::App => *permission,
            };
            settings.set_auto_paste(*enabled, accessibility);
        }
        Op::VadEnabled(enabled) => settings.set_vad_enabled(*enabled),
        Op::ShowOverlay(enabled) => settings.show_overlay = *enabled,
        Op::BeamSize(beam) => settings.beam_size = *beam,
        Op::TemperatureFallback(enabled) => settings.temperature_fallback = *enabled,
        Op::EnginePrewarm(mode) => settings.engine_prewarm = *mode,
        Op::EngineIdleUnloadMinutes(minutes) => settings.engine_idle_unload_minutes = *minutes,
        Op::FileTranscriptionModel(choice) => settings.file_transcription_model = *choice,
        Op::HotkeyMode(mode) => settings.replace_hotkey_mode(*mode)?,
        Op::Hotkey(shortcut) => {
            crate::settings::validate_hotkey(shortcut)?;
            settings.try_set_legacy_hotkey(shortcut.to_string())?;
        }
        Op::Profiles(edit) => {
            let (profiles, model) = edit.resulting(start)?;
            settings.apply_profiles_change(
                profiles,
                model.as_ref().map(|(id, choice)| (id.as_str(), *choice)),
            )?;
        }
        Op::ProfileModel { id, model } => settings.set_profile_model(id, *model)?,
        Op::DictionaryAdd {
            profile,
            term,
            aliases,
        } => {
            let aliases: Vec<String> = aliases.iter().map(|alias| alias.to_string()).collect();
            settings
                .add_dictionary_entry(profile, term, &aliases)
                .map_err(|refusal| refusal.to_string())?;
        }
        Op::InitialPrompt(text) => match surface {
            Surface::Cli => settings
                .merge_default_dictionary(text)
                .map_err(|refusal| refusal.to_string())?,
            Surface::App => {
                let profile = settings.default_profile().id;
                settings
                    .set_profile_dictionary(&profile, text.to_string())
                    .map_err(|refusal| refusal.to_string())?;
            }
        },
        // `config reset hotkey` edits the default profile's `shortcut` and applies
        // the list; a profile with explicit push-to-talk or toggle bindings keeps
        // them, and they decide the shortcut. The app's setter (and `config set
        // hotkey`) moves the bindings along. A difference nobody decided: the
        // row that has bindings names it.
        Op::Reset("hotkey") if surface == Surface::Cli => {
            let mut profiles = settings.resolved_hotkey_profiles();
            let index = HotkeyProfile::default_index(&profiles);
            profiles[index].shortcut = DEFAULT_HOTKEY.into();
            settings.apply_profiles_change(profiles, None)?;
        }
        Op::Reset(key) => apply(&reset_as_op(key), surface, start, settings)?,
    }
    Ok(())
}

/// The text the dictionary editor of the app saves after the user added the
/// entry of a [`Op::DictionaryAdd`] to the dictionary it had read: the entry
/// merged into the text, whether or not the settings accept it.
pub fn dictionary_text_with(current: &str, term: &str, aliases: &[&str]) -> String {
    let mut glossary = Glossary::parse(current);
    glossary.upsert(
        term.to_string(),
        aliases.iter().map(|alias| alias.to_string()).collect(),
    );
    glossary.render()
}

/// The top-level settings fields in which `a` and `b` differ.
pub(crate) fn differing_fields(a: &Settings, b: &Settings) -> Vec<String> {
    let (Value::Object(a), Value::Object(b)) = (
        serde_json::to_value(a).expect("settings serialize"),
        serde_json::to_value(b).expect("settings serialize"),
    ) else {
        panic!("settings serialize to an object");
    };
    let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter(|key| a.get(*key) != b.get(*key))
        .cloned()
        .collect()
}

/// Check what `surface` persisted for `case`. `before` and `after` are the
/// settings file as the surface loads it, before and after it ran; `ran` is
/// the surface's own verdict (the CLI's exit status, the setter's `Result`).
///
/// - An operation core accepts must succeed and leave exactly the state
///   [`expected`] computes, in every field.
/// - An operation core refuses must fail and leave the file as it was.
pub fn assert_persisted(
    case: &Case,
    surface: Surface,
    ran: Result<(), String>,
    before: &Settings,
    after: &Settings,
) {
    let label = format!("{} [{surface:?}]", case.name);
    match expected(case, surface, before) {
        Ok(want) => {
            if let Err(error) = ran {
                panic!("{label}: core accepts this, the surface refused it: {error}");
            }
            let differing = differing_fields(&want, after);
            assert!(
                differing.is_empty(),
                "{label}: the persisted settings differ from the shared operation in {differing:?}\n\
                 expected {}\n\
                 persisted {}",
                serde_json::to_string(&want).unwrap(),
                serde_json::to_string(after).unwrap(),
            );
        }
        Err(refusal) => {
            assert!(
                ran.is_err(),
                "{label}: core refuses this ({refusal}), the surface accepted it"
            );
            let differing = differing_fields(before, after);
            assert!(
                differing.is_empty(),
                "{label}: a refused change still wrote {differing:?}"
            );
        }
    }
}

#[cfg(test)]
#[path = "settings_ops_table_tests.rs"]
mod tests;
