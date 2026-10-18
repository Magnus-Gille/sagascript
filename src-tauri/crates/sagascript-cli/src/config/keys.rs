//! The `config` keys as a table (#373).
//!
//! A key used to be a string that nine `match` arms had to agree on, three of
//! them ending in an arm that could only panic, plus a hand-written list in `VALID_KEYS`,
//! in `cmd_list` and in four help texts. Now a key is one [`Key`] in [`KEYS`]:
//! `get`, `set`, `reset`, the warning and the line of `config set --help` sit
//! together, and [`VALID_KEYS`], `config list`, the key lists of
//! `config list|get --help` and the table of `config set --help` are generated
//! from it. A new setting is one entry (docs/recipes/add-setting.md).
//!
//! `set` and `reset` apply the same operation the app's setter applies
//! (`sagascript_core::settings::ops`) whenever there is one; what is parsed here
//! is the text only, and a value is parsed by the type's own `FromStr` /
//! `from_id`, the parser the flags use, so `config set language english` and
//! `transcribe --language english` take the same spellings.

use std::str::FromStr;

use sagascript_core::error::DictationError;
use sagascript_core::settings::{
    validate_hotkey, Accessibility, DictionaryRefusal, EnginePrewarm, FileModel,
    FileModelPreference, HotkeyMode, HotkeyProfile, Language, ModelResetNotice, Settings,
    WhisperModel,
};

use super::shortcut_warning;

/// What `set` and `reset` report: the profiles whose model went back to Auto
/// because a language change made it unusable.
type Notices = Vec<ModelResetNotice>;

/// One `config` key.
pub(super) struct Key {
    pub name: &'static str,
    /// Shown after the name in the key lists of `config list --help` and
    /// `config get --help`.
    pub note: Option<&'static str>,
    /// The entry of `config set --help`: the accepted values, a line per element.
    pub help: &'static [&'static str],
    /// The value `config get` prints.
    pub get: fn(&Settings) -> String,
    /// Parse `value` and apply it. An error changes nothing the caller keeps.
    pub set: fn(&mut Settings, &str) -> Result<Notices, DictationError>,
    /// Put the setting back to its value in the second argument, the defaults.
    pub reset: fn(&mut Settings, &Settings) -> Result<Notices, String>,
    /// A warning for `config set` to print after the value was stored.
    pub warning: fn(&Settings) -> Option<&'static str>,
}

fn no_warning(_: &Settings) -> Option<&'static str> {
    None
}

/// A key that is a `bool` field of [`Settings`] with nothing to apply beside it.
macro_rules! bool_key {
    ($name:literal, $field:ident, $help:expr) => {
        Key {
            name: $name,
            note: None,
            help: $help,
            get: |settings| settings.$field.to_string(),
            set: |settings, value| {
                settings.$field = parse_bool(value, $name)?;
                Ok(Vec::new())
            },
            reset: |settings, defaults| {
                settings.$field = defaults.$field;
                Ok(Vec::new())
            },
            warning: no_warning,
        }
    };
}

/// A key that is a `u32` field of [`Settings`].
macro_rules! count_key {
    ($name:literal, $field:ident, $help:expr) => {
        Key {
            name: $name,
            note: None,
            help: $help,
            get: |settings| settings.$field.to_string(),
            set: |settings, value| {
                settings.$field = value.parse::<u32>().map_err(|_| {
                    DictationError::SettingsError(format!(
                        "{} must be a non-negative integer, got '{value}'",
                        $name
                    ))
                })?;
                Ok(Vec::new())
            },
            reset: |settings, defaults| {
                settings.$field = defaults.$field;
                Ok(Vec::new())
            },
            warning: no_warning,
        }
    };
}

/// Every key, in the order `config list` prints them.
pub(super) const KEYS: &[Key] = &[
    Key {
        name: "language",
        note: None,
        help: &["en, sv, no, fi, auto (auto uses a generic model — less accurate)"],
        get: |settings| settings.default_profile().language.id().to_string(),
        set: |settings, value| {
            let language = parse_value::<Language>(value, "language")?;
            settings
                .apply_language_selection(language)
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| settings.apply_language_selection(defaults.language),
        warning: no_warning,
    },
    Key {
        name: "whisper_model",
        note: None,
        help: &[
            "tiny.en, tiny, base.en, base, kb-whisper-tiny,",
            "kb-whisper-base, kb-whisper-small, nb-whisper-tiny,",
            "nb-whisper-base, nb-whisper-small, fi-whisper-tiny",
        ],
        get: whisper_model_value,
        set: |settings, value| {
            let model = parse_value::<WhisperModel>(value, "whisper_model")?;
            settings
                .set_whisper_model(model)
                .map(|()| Vec::new())
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| {
            settings.whisper_model = defaults.whisper_model;
            settings
                .set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    Key {
        name: "file_transcription_model",
        note: None,
        help: &["auto, any compatible Whisper model ID, pianissimo-sv"],
        get: |settings| settings.file_transcription_model.id().to_string(),
        set: |settings, value| {
            settings.file_transcription_model =
                FileModelPreference::parse_id(value).map_err(DictationError::SettingsError)?;
            Ok(Vec::new())
        },
        reset: |settings, defaults| {
            settings.file_transcription_model = defaults.file_transcription_model;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    Key {
        name: "hotkey_mode",
        note: Some("(push, toggle)"),
        help: &["push, toggle"],
        get: |settings| settings.hotkey_mode.id().to_string(),
        set: |settings, value| {
            if value == "presenter" {
                return Err(DictationError::SettingsError(
                    "Invalid value 'presenter' for hotkey_mode. Supported values: push, toggle."
                        .to_string(),
                ));
            }
            let mode =
                HotkeyMode::from_id(value).ok_or_else(|| invalid_value(value, "hotkey_mode"))?;
            settings
                .replace_hotkey_mode(mode)
                .map(|()| Vec::new())
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| {
            settings
                .replace_hotkey_mode(defaults.hotkey_mode)
                .map(|()| Vec::new())
        },
        warning: no_warning,
    },
    bool_key!("show_overlay", show_overlay, &["true, false"]),
    Key {
        name: "auto_paste",
        note: None,
        help: &["true, false (enabling requires Accessibility approval for the installed GUI)"],
        get: |settings| settings.auto_paste.to_string(),
        set: |settings, value| {
            // The CLI cannot ask for the installed app's permission: it stores
            // the request and warns (below); the app applies the permission
            // when it loads the settings.
            settings.set_auto_paste(parse_bool(value, "auto_paste")?, Accessibility::Unknown);
            Ok(Vec::new())
        },
        reset: |settings, defaults| {
            settings.auto_paste = defaults.auto_paste;
            Ok(Vec::new())
        },
        warning: |settings| {
            settings.auto_paste.then_some(
                "auto-paste requires Accessibility approval for the installed Sagascript app; \
                 until it is granted, the GUI will keep or reset auto-paste to false",
            )
        },
    },
    Key {
        name: "auto_select_model",
        note: None,
        help: &["true, false"],
        get: |settings| {
            settings
                .profile_models
                .get(&settings.default_profile().id)
                .map_or(settings.auto_select_model, |model| {
                    *model == FileModelPreference::Auto
                })
                .to_string()
        },
        set: |settings, value| {
            let enabled = parse_bool(value, "auto_select_model")?;
            settings
                .set_auto_select_model(enabled)
                .map(|()| Vec::new())
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| {
            settings.auto_select_model = defaults.auto_select_model;
            settings
                .set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    Key {
        name: "pianissimo_dictation",
        note: None,
        help: &["true, false (Swedish live dictation only)"],
        get: |settings| {
            settings
                .profile_models
                .get(&settings.default_profile().id)
                .map_or(settings.pianissimo_dictation, |model| {
                    *model == FileModelPreference::PianissimoOriginal
                })
                .to_string()
        },
        set: |settings, value| {
            let enabled = parse_bool(value, "pianissimo_dictation")?;
            settings
                .set_pianissimo_dictation(enabled)
                .map(|()| Vec::new())
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| {
            settings.pianissimo_dictation = defaults.pianissimo_dictation;
            settings
                .set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    Key {
        name: "hotkey",
        note: None,
        help: &["Modifier+Key; bare F13-F24 on macOS (Accessibility) or Windows"],
        get: |settings| settings.hotkey.clone(),
        set: |settings, value| {
            validate_hotkey(value).map_err(DictationError::SettingsError)?;
            settings
                .try_set_legacy_hotkey(value.to_string())
                .map(|()| Vec::new())
                .map_err(DictationError::SettingsError)
        },
        reset: |settings, defaults| {
            let mut profiles = settings.resolved_hotkey_profiles();
            let index = HotkeyProfile::default_index(&profiles);
            profiles[index].shortcut = defaults.hotkey.clone();
            settings.apply_profiles_change(profiles, None).map(drop)?;
            Ok(Vec::new())
        },
        warning: |settings| shortcut_warning(&settings.hotkey),
    },
    Key {
        name: "initial_prompt",
        note: None,
        help: &["Personal dictionary text; aliases use TERM = ALIAS | ALIAS"],
        get: |settings| {
            settings
                .profile_glossaries
                .get(&settings.default_profile().id)
                .cloned()
                .unwrap_or_else(|| {
                    if settings.profile_glossary_migrated {
                        String::new()
                    } else {
                        settings.initial_prompt.clone()
                    }
                })
        },
        set: |settings, value| append_legacy_hints(settings, value).map(|()| Vec::new()),
        reset: |settings, _| {
            if settings
                .profile_glossaries
                .get(&settings.default_profile().id)
                .is_some_and(|source| !source.trim().is_empty())
            {
                Err("Default profile dictionary contains entries; use `sagascript glossary clear --yes` after reviewing them".into())
            } else {
                Ok(Vec::new())
            }
        },
        warning: no_warning,
    },
    count_key!(
        "beam_size",
        beam_size,
        &["Integer >= 0 (0 = greedy/fast, 5 = beam search/accurate)"]
    ),
    bool_key!(
        "temperature_fallback",
        temperature_fallback,
        &["true, false"]
    ),
    Key {
        name: "vad_enabled",
        note: None,
        help: &["true, false"],
        get: |settings| settings.vad_enabled.to_string(),
        set: |settings, value| {
            // The app also fetches the VAD model when it turns this on; the CLI
            // fetches it on first use (`transcribe --vad`).
            settings.set_vad_enabled(parse_bool(value, "vad_enabled")?);
            Ok(Vec::new())
        },
        reset: |settings, defaults| {
            settings.vad_enabled = defaults.vad_enabled;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    Key {
        name: "engine_prewarm",
        note: None,
        help: &["off, on_app_start, on_key_down (when the Pianissimo engine loads its model)"],
        get: |settings| settings.engine_prewarm.id().to_string(),
        set: |settings, value| {
            settings.engine_prewarm =
                EnginePrewarm::parse_id(value).map_err(DictationError::SettingsError)?;
            Ok(Vec::new())
        },
        reset: |settings, defaults| {
            settings.engine_prewarm = defaults.engine_prewarm;
            Ok(Vec::new())
        },
        warning: no_warning,
    },
    count_key!(
        "engine_idle_unload_minutes",
        engine_idle_unload_minutes,
        &["Integer >= 0 (default 60; engine host exits after twice this; 0 = never unload)"]
    ),
];

/// The key names, generated from [`KEYS`].
pub(super) const VALID_KEYS: [&str; KEYS.len()] = {
    let mut names = [""; KEYS.len()];
    let mut index = 0;
    while index < KEYS.len() {
        names[index] = KEYS[index].name;
        index += 1;
    }
    names
};

/// The key called `name`, or the error `config get|set|reset` reports.
pub(super) fn lookup(name: &str) -> Result<&'static Key, DictationError> {
    KEYS.iter().find(|key| key.name == name).ok_or_else(|| {
        DictationError::SettingsError(format!(
            "Unknown setting '{name}'. Valid keys: {}",
            VALID_KEYS.join(", ")
        ))
    })
}

// -- Help text, generated --

/// The key list of `config list --help` and `config get --help`, with the
/// notes some keys carry: `language, …, hotkey_mode (push, toggle), …`.
pub(super) fn key_list_help() -> String {
    KEYS.iter()
        .map(|key| match key.note {
            Some(note) => format!("{} {note}", key.name),
            None => key.name.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `language, whisper_model, …` for the `[possible values: …]` of the key argument.
pub(super) fn possible_values() -> String {
    VALID_KEYS.join(", ")
}

/// The table of `config set --help`: a line per key, its values aligned.
pub(super) fn valid_values_help() -> String {
    const NAME_COLUMN: usize = 20;
    // Two spaces of indent, the name column and a space: where a value starts.
    const CONTINUATION: &str = "                       ";
    let mut lines = Vec::new();
    for key in KEYS {
        let mut values = key.help.iter();
        let first = values.next().copied().unwrap_or_default();
        // A name wider than the column is followed by two spaces instead.
        lines.push(if key.name.len() <= NAME_COLUMN {
            format!("  {:<NAME_COLUMN$} {first}", key.name)
        } else {
            format!("  {}  {first}", key.name)
        });
        lines.extend(values.map(|rest| format!("{CONTINUATION}{rest}")));
    }
    lines.join("\n")
}

// -- Reading what the user typed --

pub(super) fn invalid_value(value: &str, key: &str) -> DictationError {
    DictationError::SettingsError(format!(
        "Invalid value '{value}' for {key}. Run 'sagascript config get {key}' to see current value."
    ))
}

/// Parse `value` with the type's own `FromStr`, the parser the flags use.
pub(super) fn parse_value<T: FromStr>(value: &str, key: &str) -> Result<T, DictationError> {
    value.parse().map_err(|_| invalid_value(value, key))
}

pub(super) fn parse_bool(value: &str, key: &str) -> Result<bool, DictationError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(DictationError::SettingsError(format!(
            "Invalid value '{value}' for {key}. Must be 'true' or 'false'."
        ))),
    }
}

// -- Keys that need more than a field --

fn whisper_model_value(settings: &Settings) -> String {
    let profile = settings.default_profile();
    match settings.dictation_model_for_profile(&profile.id) {
        Ok(model) => model.id().to_string(),
        Err(_) => FileModel::Whisper(WhisperModel::recommended(profile.language))
            .id()
            .to_string(),
    }
}

/// The old `initial_prompt` command is a compatibility alias for *merging*
/// default-profile entries and aliases (`Settings::merge_default_dictionary`).
/// Never replace a profile dictionary: it may contain the user's reviewed
/// deterministic aliases and has no implicit rollback file of its own.
fn append_legacy_hints(settings: &mut Settings, value: &str) -> Result<(), DictationError> {
    settings.merge_default_dictionary(value).map_err(|refusal| {
        DictationError::SettingsError(match refusal {
            DictionaryRefusal::WouldEraseEntries => {
                "Use `sagascript glossary clear --yes` to remove reviewed profile entries".into()
            }
            other => other.to_string(),
        })
    })
}
