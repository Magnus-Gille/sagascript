use clap::{Args, Subcommand};

use sagascript_core::error::DictationError;
use sagascript_core::settings::{
    self, validate_hotkey, EnginePrewarm, FileModelPreference, HotkeyMode, HotkeyProfile, Language, Settings,
    WhisperModel,
};
use sagascript_core::transcription::Glossary;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Show all settings with current and default values
    #[command(long_about = "\
Show all settings in a table with their current values and defaults.

Valid keys: language, whisper_model, file_transcription_model, hotkey_mode (push, toggle), show_overlay, \
auto_paste, auto_select_model, pianissimo_dictation, hotkey, initial_prompt, \
beam_size, temperature_fallback, vad_enabled, engine_prewarm, engine_idle_unload_minutes.")]
    List,

    /// Get a single setting value
    #[command(
        long_about = "\
Print the current value of a single setting to stdout.

Valid keys: language, whisper_model, file_transcription_model, hotkey_mode (push, toggle), show_overlay, \
auto_paste, auto_select_model, pianissimo_dictation, hotkey, initial_prompt, \
beam_size, temperature_fallback, vad_enabled, engine_prewarm, engine_idle_unload_minutes",
        after_long_help = "\
EXAMPLES:
  sagascript config get language
  sagascript config get hotkey
  sagascript config get initial_prompt"
    )]
    Get {
        /// Setting key [possible values: language, whisper_model, file_transcription_model, hotkey_mode, show_overlay, auto_paste, auto_select_model, pianissimo_dictation, hotkey, initial_prompt, beam_size, temperature_fallback, vad_enabled, engine_prewarm, engine_idle_unload_minutes]
        key: String,
    },

    /// Set a setting value
    #[command(
        long_about = "\
Update a setting. The new value takes effect immediately — the GUI \
 hot-reloads changes made via CLI. Legacy language, whisper_model, auto_select_model, \
 pianissimo_dictation, and initial_prompt keys edit the default profile. The \
 initial_prompt alias only merges entries and never erases saved aliases; prefer \
 `config profiles update ID --language ... --model ...`.

Valid values per key:
  language             en, sv, no, fi, auto (auto uses a generic model — less accurate)
  whisper_model        tiny.en, tiny, base.en, base, kb-whisper-tiny,
                       kb-whisper-base, kb-whisper-small, nb-whisper-tiny,
                       nb-whisper-base, nb-whisper-small, fi-whisper-tiny
  file_transcription_model  auto, any compatible Whisper model ID, pianissimo-sv
  hotkey_mode          push, toggle
  show_overlay         true, false
  auto_paste           true, false (enabling requires Accessibility approval for the installed GUI)
  auto_select_model    true, false
  pianissimo_dictation true, false (Swedish live dictation only)
  hotkey               Modifier+Key; bare F13-F24 on macOS (Accessibility) or Windows
  initial_prompt       Personal dictionary text; aliases use TERM = ALIAS | ALIAS
  beam_size            Integer >= 0 (0 = greedy/fast, 5 = beam search/accurate)
  temperature_fallback true, false
  vad_enabled          true, false
  engine_prewarm       off, on_app_start, on_key_down (when the Pianissimo engine loads its model)
  engine_idle_unload_minutes  Integer >= 0 (default 60; engine host exits after twice this; 0 = never unload)",
        after_long_help = "\
EXAMPLES:
  sagascript config set language sv
  sagascript config set whisper_model kb-whisper-base
  sagascript config set hotkey 'Option+Space'
  sagascript config set hotkey F13
  sagascript config set auto_paste false
  sagascript config set pianissimo_dictation true
  sagascript config set initial_prompt $'OpenRouter = open router | open vrouter\\nmerge = merch'"
    )]
    Set {
        /// Setting key [possible values: language, whisper_model, file_transcription_model, hotkey_mode, show_overlay, auto_paste, auto_select_model, pianissimo_dictation, hotkey, initial_prompt, beam_size, temperature_fallback, vad_enabled, engine_prewarm, engine_idle_unload_minutes]
        key: String,
        /// New value for the setting
        value: String,
    },

    /// Reset one or all settings to defaults
    #[command(
        long_about = "\
Reset a single application setting or all application settings to their default values.

If KEY is provided, only that setting is reset. \
If KEY is omitted, all application settings are reset. External personal \
 dictionaries are preserved. `sagascript glossary clear --yes` clears the \
 default profile; add `--profile ID` for another dictionary.",
        after_long_help = "\
EXAMPLES:
  # Reset just the language
  sagascript config reset language
  sagascript config reset pianissimo_dictation

  # Reset everything
  sagascript config reset"
    )]
    Reset {
        /// Setting key to reset (omit to reset all)
        key: Option<String>,
    },

    /// Print the settings file path
    #[command(long_about = "\
Print the absolute path to the settings JSON file. Use `sagascript glossary \
path` for the separate personal dictionary.")]
    Path,

    /// Manage per-shortcut dictation profiles with language and model
    Profiles {
        #[command(subcommand)]
        action: ProfileAction,
    },
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// List all dictation profiles
    List,
    /// Create a dictation profile
    #[command(alias = "add")]
    Create {
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(long = "push-to-talk-shortcut", conflicts_with = "clear_push_to_talk_shortcut")]
        push_to_talk_shortcut: Option<String>,
        #[arg(long = "toggle-shortcut", conflicts_with = "clear_toggle_shortcut")]
        toggle_shortcut: Option<String>,
        #[arg(long = "clear-push-to-talk-shortcut")]
        clear_push_to_talk_shortcut: bool,
        #[arg(long = "clear-toggle-shortcut")]
        clear_toggle_shortcut: bool,
        #[arg(long)]
        language: String,
        /// Live dictation model ID (auto, compatible Whisper model, or Swedish pianissimo-sv)
        #[arg(long)]
        model: Option<String>,
    },
    /// Update a dictation profile
    #[command(alias = "set")]
    Update {
        id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        hotkey: Option<String>,
        #[arg(long = "push-to-talk-shortcut", conflicts_with = "clear_push_to_talk_shortcut")]
        push_to_talk_shortcut: Option<String>,
        #[arg(long = "toggle-shortcut", conflicts_with = "clear_toggle_shortcut")]
        toggle_shortcut: Option<String>,
        #[arg(long = "clear-push-to-talk-shortcut")]
        clear_push_to_talk_shortcut: bool,
        #[arg(long = "clear-toggle-shortcut")]
        clear_toggle_shortcut: bool,
        #[arg(long)]
        language: Option<String>,
        /// Live dictation model ID (auto, compatible Whisper model, or Swedish pianissimo-sv)
        #[arg(long)]
        model: Option<String>,
    },
    /// Remove a dictation profile (at least one must remain)
    Remove { id: String },
}

const VALID_KEYS: &[&str] = &[
    "language",
    "whisper_model",
    "file_transcription_model",
    "hotkey_mode",
    "show_overlay",
    "auto_paste",
    "auto_select_model",
    "pianissimo_dictation",
    "hotkey",
    "initial_prompt",
    "beam_size",
    "temperature_fallback",
    "vad_enabled",
    "engine_prewarm",
    "engine_idle_unload_minutes",
];

pub fn run(args: ConfigArgs) -> Result<(), DictationError> {
    match args.action {
        ConfigAction::List => cmd_list(),
        ConfigAction::Get { key } => cmd_get(&key),
        ConfigAction::Set { key, value } => cmd_set(&key, &value),
        ConfigAction::Reset { key } => cmd_reset(key.as_deref()),
        ConfigAction::Path => cmd_path(),
        ConfigAction::Profiles { action } => cmd_profiles(action),
    }
}

fn cmd_profiles(action: ProfileAction) -> Result<(), DictationError> {
    match action {
        ProfileAction::List => {
            println!("{:<16} {:<20} {:<28} {:<28} {:<28} {:<12} MODEL", "ID", "NAME", "LEGACY", "PUSH-TO-TALK", "TOGGLE", "LANGUAGE");
            let settings = settings::store::load();
            for profile in settings.resolved_hotkey_profiles() {
                let model = settings.profile_models.get(&profile.id).copied().unwrap_or(FileModelPreference::Auto);
                let model_id = serde_json::to_value(model).and_then(serde_json::from_value::<String>)
                    .map_err(|error| DictationError::SettingsError(error.to_string()))?;
                println!(
                    "{:<16} {:<20} {:<28} {:<28} {:<28} {:<12} {}",
                    profile.id,
                    profile.name,
                    profile.shortcut,
                    profile.push_to_talk_shortcut.as_deref().unwrap_or("-"),
                    profile.toggle_shortcut.as_deref().unwrap_or("-"),
                    format_language(profile.language),
                    model_id,
                );
            }
            Ok(())
        }
        ProfileAction::Create {
            id,
            name,
            hotkey,
            push_to_talk_shortcut,
            toggle_shortcut,
            clear_push_to_talk_shortcut,
            clear_toggle_shortcut,
            language,
            model,
        } => {
            let language = parse_enum_value::<Language>(&language, "language")?;
            let model = model.as_deref().map(FileModelPreference::parse_id).transpose()
                .map_err(DictationError::SettingsError)?;
            if clear_push_to_talk_shortcut || clear_toggle_shortcut {
                return Err(DictationError::SettingsError(
                    "Clear shortcut options are only valid when updating a profile".to_string(),
                ));
            }
            let shortcut = hotkey
                .or_else(|| push_to_talk_shortcut.clone())
                .or_else(|| toggle_shortcut.clone())
                .ok_or_else(|| {
                    DictationError::SettingsError(
                        "Specify --hotkey or at least one explicit shortcut".to_string(),
                    )
                })?;
            let hotkey_warnings = [
                shortcut_warning(&shortcut),
                push_to_talk_shortcut.as_deref().and_then(shortcut_warning),
                toggle_shortcut.as_deref().and_then(shortcut_warning),
            ];
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            if profiles.iter().any(|profile| profile.id == id) {
                return Err(DictationError::SettingsError(format!(
                    "Profile '{id}' already exists"
                )));
            }
            profiles.push(HotkeyProfile {
                id: id.clone(),
                name,
                shortcut,
                language,
                push_to_talk_shortcut,
                toggle_shortcut,
            });
            persist_profiles(profiles, model.map(|choice| (id.as_str(), choice)))?;
            eprintln!("Created profile {id}");
            for warning in hotkey_warnings.into_iter().flatten().collect::<std::collections::HashSet<_>>() {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Update {
            id,
            name,
            hotkey,
            push_to_talk_shortcut,
            toggle_shortcut,
            clear_push_to_talk_shortcut,
            clear_toggle_shortcut,
            language,
            model,
        } => {
            if name.is_none()
                && hotkey.is_none()
                && push_to_talk_shortcut.is_none()
                && toggle_shortcut.is_none()
                && !clear_push_to_talk_shortcut
                && !clear_toggle_shortcut
                && language.is_none()
                && model.is_none()
            {
                return Err(DictationError::SettingsError(
                    "Specify at least one profile field or shortcut option".to_string(),
                ));
            }
            let hotkey_warnings = [
                hotkey.as_deref().and_then(shortcut_warning),
                push_to_talk_shortcut.as_deref().and_then(shortcut_warning),
                toggle_shortcut.as_deref().and_then(shortcut_warning),
            ];
            let language = language
                .as_deref()
                .map(|value| parse_enum_value::<Language>(value, "language"))
                .transpose()?;
            let model = model.as_deref().map(FileModelPreference::parse_id).transpose()
                .map_err(DictationError::SettingsError)?;
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            let profile = profiles
                .iter_mut()
                .find(|profile| profile.id == id)
                .ok_or_else(|| DictationError::SettingsError(format!("Unknown profile '{id}'")))?;
            if let Some(name) = name {
                profile.name = name;
            }
            if let Some(hotkey) = hotkey {
                profile.set_primary_shortcut(hotkey);
            }
            if clear_push_to_talk_shortcut {
                profile.push_to_talk_shortcut = None;
            }
            if clear_toggle_shortcut {
                profile.toggle_shortcut = None;
            }
            if let Some(shortcut) = push_to_talk_shortcut {
                profile.push_to_talk_shortcut = Some(shortcut);
            }
            if let Some(shortcut) = toggle_shortcut {
                profile.toggle_shortcut = Some(shortcut);
            }
            if let Some(language) = language {
                profile.language = language;
            }
            persist_profiles(profiles, model.map(|choice| (id.as_str(), choice)))?;
            eprintln!("Updated profile {id}");
            for warning in hotkey_warnings.into_iter().flatten().collect::<std::collections::HashSet<_>>() {
                eprintln!("Warning: {warning}");
            }
            Ok(())
        }
        ProfileAction::Remove { id } => {
            let stored = settings::store::load();
            let dictionary_kept = stored
                .profile_glossaries
                .get(&id)
                .is_some_and(|source| !source.trim().is_empty());
            let mut profiles = stored.resolved_hotkey_profiles();
            let original_len = profiles.len();
            profiles.retain(|profile| profile.id != id);
            if profiles.len() == original_len {
                return Err(DictationError::SettingsError(format!(
                    "Unknown profile '{id}'"
                )));
            }
            persist_profiles(profiles, None)?;
            eprintln!("Removed profile {id}");
            if dictionary_kept {
                eprintln!(
                    "Its personal dictionary was kept. Inspect it with `sagascript glossary list --profile {id}` or remove it with `sagascript glossary clear --profile {id}`."
                );
            }
            Ok(())
        }
    }
}

fn persist_profiles(profiles: Vec<HotkeyProfile>, model: Option<(&str, FileModelPreference)>) -> Result<(), DictationError> {
    Settings::validate_hotkey_profiles(&profiles).map_err(DictationError::SettingsError)?;
    settings::store::try_update(|settings| {
        if let Some((id, preference)) = model {
            settings.profile_models.insert(id.to_string(), preference);
        }
        settings.replace_hotkey_profiles(profiles)?;
        if let Some((id, preference)) = model {
            settings.set_profile_model(id, preference)?;
        }
        for profile in settings.resolved_hotkey_profiles() {
            settings.dictation_model_for_profile(&profile.id)?;
        }
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;
    Ok(())
}

fn cmd_list() -> Result<(), DictationError> {
    let current = settings::store::load();
    let defaults = Settings::default();

    println!("{:<20} {:<24} DEFAULT", "KEY", "CURRENT");
    println!("{:<20} {:<24} -------", "---", "-------");
    println!(
        "{:<20} {:<24} {}",
        "language",
        get_setting_value(&current, "language"),
        get_setting_value(&defaults, "language")
    );
    println!(
        "{:<20} {:<24} {}",
        "whisper_model",
        get_setting_value(&current, "whisper_model"),
        get_setting_value(&defaults, "whisper_model")
    );
    println!(
        "{:<20} {:<24} {}",
        "file_transcription_model",
        format_file_model(current.file_transcription_model),
        format_file_model(defaults.file_transcription_model)
    );
    println!(
        "{:<20} {:<24} {}",
        "hotkey_mode",
        format_hotkey_mode(current.hotkey_mode),
        format_hotkey_mode(defaults.hotkey_mode)
    );
    println!(
        "{:<20} {:<24} {}",
        "show_overlay", current.show_overlay, defaults.show_overlay
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_paste", current.auto_paste, defaults.auto_paste
    );
    println!(
        "{:<20} {:<24} {}",
        "auto_select_model", get_setting_value(&current, "auto_select_model"), get_setting_value(&defaults, "auto_select_model")
    );
    println!(
        "{:<20} {:<24} {}",
        "pianissimo_dictation", get_setting_value(&current, "pianissimo_dictation"), get_setting_value(&defaults, "pianissimo_dictation")
    );
    println!(
        "{:<20} {:<24} {}",
        "hotkey", current.hotkey, defaults.hotkey
    );
    println!(
        "{:<20} {:<24} {}",
        "initial_prompt", get_setting_value(&current, "initial_prompt"), get_setting_value(&defaults, "initial_prompt")
    );
    println!(
        "{:<20} {:<24} {}",
        "beam_size", current.beam_size, defaults.beam_size
    );
    println!(
        "{:<20} {:<24} {}",
        "temperature_fallback", current.temperature_fallback, defaults.temperature_fallback
    );
    println!(
        "{:<20} {:<24} {}",
        "vad_enabled", current.vad_enabled, defaults.vad_enabled
    );
    println!(
        "{:<20} {:<24} {}",
        "engine_prewarm", current.engine_prewarm.id(), defaults.engine_prewarm.id()
    );
    println!(
        "{:<20} {:<24} {}",
        "engine_idle_unload_minutes", current.engine_idle_unload_minutes, defaults.engine_idle_unload_minutes
    );
    Ok(())
}

fn cmd_get(key: &str) -> Result<(), DictationError> {
    validate_key(key)?;
    let settings = settings::store::load();
    let value = get_setting_value(&settings, key);
    println!("{value}");
    Ok(())
}

fn cmd_set(key: &str, value: &str) -> Result<(), DictationError> {
    validate_key(key)?;
    // Parse before acquiring the settings lock so invalid input never writes.
    let mut validation_target = settings::store::load();
    apply_setting_value(&mut validation_target, key, value)?;
    if matches!(key, "hotkey" | "hotkey_mode") {
        validation_target
            .validate_shortcut_configuration()
            .map_err(DictationError::SettingsError)?;
    }
    let settings = settings::store::try_update(|settings| {
        apply_setting_value(settings, key, value).map_err(|error| error.to_string())?;
        if matches!(key, "hotkey" | "hotkey_mode") {
            settings.validate_shortcut_configuration()?;
        }
        Ok(())
    })
    .map_err(DictationError::SettingsError)?;

    eprintln!("Set {key} = {}", get_setting_value(&settings, key));
    if let Some(warning) = setting_warning(key, &settings) {
        eprintln!("Warning: {warning}");
    }
    Ok(())
}

fn setting_warning(key: &str, settings: &Settings) -> Option<&'static str> {
    if key == "auto_paste" && settings.auto_paste {
        Some(
            "auto-paste requires Accessibility approval for the installed Sagascript app; \
             until it is granted, the GUI will keep or reset auto-paste to false",
        )
    } else if key == "hotkey" {
        shortcut_warning(&settings.hotkey)
    } else {
        None
    }
}

/// Warning for a shortcut: the brightness-key hint takes precedence because a
/// key that never reaches the app makes the Accessibility note moot.
fn shortcut_warning(shortcut: &str) -> Option<&'static str> {
    brightness_key_hotkey_warning(shortcut).or_else(|| bare_extended_hotkey_warning(shortcut))
}

/// macOS maps bare F14/F15 to the screen-brightness keys, so they usually never
/// reach the app. Mirrors the hint in src/lib/hotkey.js.
fn brightness_key_hotkey_warning(shortcut: &str) -> Option<&'static str> {
    let is_brightness_key = matches!(shortcut.trim().to_ascii_uppercase().as_str(), "F14" | "F15");
    (cfg!(target_os = "macos") && is_brightness_key).then_some(
        "F14 and F15 control screen brightness on Macs and usually don't reach Sagascript. \
         F13 and F16-F19 work",
    )
}

fn bare_extended_hotkey_warning(shortcut: &str) -> Option<&'static str> {
    // Strict parity with src/lib/hotkey.js (/^F(\d{1,2})$/i): at most two
    // ASCII digits, so "F013" never warns as a bare extended key.
    let normalized = shortcut.trim().to_ascii_lowercase();
    let digits = normalized.strip_prefix('f')?;
    let is_bare_extended = !digits.is_empty()
        && digits.len() <= 2
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits
            .parse::<u8>()
            .is_ok_and(|number| (13..=24).contains(&number));
    (cfg!(target_os = "macos") && is_bare_extended)
        .then_some("bare F13-F24 requires Accessibility approval for the installed Sagascript app")
}

fn apply_setting_value(
    settings: &mut Settings,
    key: &str,
    value: &str,
) -> Result<(), DictationError> {
    match key {
        "language" => {
            settings
                .set_default_profile_language(parse_enum_value::<Language>(value, "language")?)
                .map_err(DictationError::SettingsError)?;
        }
        "whisper_model" => {
            let model = parse_enum_value::<WhisperModel>(value, "whisper_model")?;
            settings.set_profile_model(&settings.default_profile().id, FileModelPreference::Whisper(model))
                .map_err(DictationError::SettingsError)?;
            settings.whisper_model = model; // retain rollback compatibility
            settings.auto_select_model = false;
        }
        "file_transcription_model" => {
            settings.file_transcription_model = FileModelPreference::parse_id(value)
                .map_err(DictationError::SettingsError)?;
        }
        "hotkey_mode" => {
            if value == "presenter" {
                return Err(DictationError::SettingsError(
                    "Invalid value 'presenter' for hotkey_mode. Supported values: push, toggle."
                        .to_string(),
                ));
            }
            settings
                .replace_hotkey_mode(parse_enum_value::<HotkeyMode>(value, "hotkey_mode")?)
                .map_err(DictationError::SettingsError)?;
        }
        "show_overlay" => {
            settings.show_overlay = parse_bool(value, "show_overlay")?;
        }
        "auto_paste" => {
            settings.auto_paste = parse_bool(value, "auto_paste")?;
        }
        "auto_select_model" => {
            let enabled = parse_bool(value, "auto_select_model")?;
            settings.set_profile_model(&settings.default_profile().id, if enabled {
                FileModelPreference::Auto
            } else {
                FileModelPreference::Whisper(settings.whisper_model)
            }).map_err(DictationError::SettingsError)?;
            settings.auto_select_model = enabled;
        }
        "pianissimo_dictation" => {
            let enabled = parse_bool(value, "pianissimo_dictation")?;
            settings.set_profile_model(&settings.default_profile().id, if enabled {
                FileModelPreference::PianissimoOriginal
            } else {
                FileModelPreference::Auto
            }).map_err(DictationError::SettingsError)?;
            settings.pianissimo_dictation = enabled; // retained for rollback only
        }
        "hotkey" => {
            validate_hotkey(value).map_err(DictationError::SettingsError)?;
            settings
                .try_set_legacy_hotkey(value.to_string())
                .map_err(DictationError::SettingsError)?;
        }
        "initial_prompt" => {
            append_legacy_hints(settings, value)?;
        }
        "beam_size" => {
            settings.beam_size = value.parse::<u32>().map_err(|_| {
                DictationError::SettingsError(format!(
                    "beam_size must be a non-negative integer, got '{value}'"
                ))
            })?;
        }
        "temperature_fallback" => {
            settings.temperature_fallback = parse_bool(value, "temperature_fallback")?;
        }
        "vad_enabled" => {
            settings.vad_enabled = parse_bool(value, "vad_enabled")?;
        }
        "engine_prewarm" => {
            settings.engine_prewarm = EnginePrewarm::parse_id(value).map_err(DictationError::SettingsError)?;
        }
        "engine_idle_unload_minutes" => {
            settings.engine_idle_unload_minutes = value.parse::<u32>().map_err(|_| {
                DictationError::SettingsError(format!(
                    "engine_idle_unload_minutes must be a non-negative integer, got '{value}'"
                ))
            })?;
        }
        _ => unreachable!(), // validate_key already checked
    }
    Ok(())
}

/// The old `initial_prompt` command is a compatibility alias for *merging*
/// default-profile entries and aliases. Never replace a profile dictionary: it may
/// contain the user's reviewed deterministic aliases and has no implicit
/// rollback file of its own.
fn append_legacy_hints(settings: &mut Settings, value: &str) -> Result<(), DictationError> {
    let id = settings.default_profile().id;
    let source = settings.profile_glossaries.entry(id).or_default();
    if value.trim().is_empty() && !source.trim().is_empty() {
        return Err(DictationError::SettingsError(
            "Use `sagascript glossary clear --yes` to remove reviewed profile entries".into(),
        ));
    }
    let mut glossary = Glossary::parse(source);
    for entry in Glossary::parse(value).entries() {
        glossary.upsert(entry.canonical.clone(), entry.aliases.clone());
    }
    *source = glossary.render();
    Ok(())
}

fn cmd_reset(key: Option<&str>) -> Result<(), DictationError> {
    if let Some(key) = key {
        validate_key(key)?;
        let defaults = Settings::default();
        if key == "hotkey" {
            let mut profiles = settings::store::load().resolved_hotkey_profiles();
            let index = profiles
                .iter()
                .position(|profile| profile.id == "default")
                .unwrap_or(0);
            profiles[index].shortcut = defaults.hotkey;
            persist_profiles(profiles, None)?;
            eprintln!("Reset hotkey to {}", settings::store::load().hotkey);
            return Ok(());
        }
        let settings =
            settings::store::try_update(|settings| reset_setting_value(settings, key, &defaults))
                .map_err(DictationError::SettingsError)?;
        eprintln!("Reset {key} to {}", get_setting_value(&settings, key));
    } else {
        settings::store::try_update(|current| {
            reset_all_settings(current)?;
            Ok(())
        })
        .map_err(DictationError::SettingsError)?;
        eprintln!("All application settings reset to defaults; personal dictionaries preserved");
    }
    Ok(())
}

fn reset_setting_value(
    settings: &mut Settings,
    key: &str,
    defaults: &Settings,
) -> Result<(), String> {
    match key {
        "language" => settings.set_default_profile_language(defaults.language),
        "whisper_model" => {
            settings.whisper_model = defaults.whisper_model;
            settings.set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(())
        }
        "file_transcription_model" => {
            settings.file_transcription_model = defaults.file_transcription_model;
            Ok(())
        }
        "hotkey_mode" => settings.replace_hotkey_mode(defaults.hotkey_mode),
        "show_overlay" => {
            settings.show_overlay = defaults.show_overlay;
            Ok(())
        }
        "auto_paste" => {
            settings.auto_paste = defaults.auto_paste;
            Ok(())
        }
        "auto_select_model" => {
            settings.auto_select_model = defaults.auto_select_model;
            settings.set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(())
        }
        "pianissimo_dictation" => {
            settings.pianissimo_dictation = defaults.pianissimo_dictation;
            settings.set_profile_model(&settings.default_profile().id, FileModelPreference::Auto)?;
            Ok(())
        }
        "hotkey" => unreachable!("hotkey reset handled transactionally above"),
        "initial_prompt" => {
            if settings.profile_glossaries.get(&settings.default_profile().id)
                .is_some_and(|source| !source.trim().is_empty()) {
                Err("Default profile dictionary contains entries; use `sagascript glossary clear --yes` after reviewing them".into())
            } else {
                Ok(())
            }
        }
        "beam_size" => {
            settings.beam_size = defaults.beam_size;
            Ok(())
        }
        "temperature_fallback" => {
            settings.temperature_fallback = defaults.temperature_fallback;
            Ok(())
        }
        "vad_enabled" => {
            settings.vad_enabled = defaults.vad_enabled;
            Ok(())
        }
        "engine_prewarm" => {
            settings.engine_prewarm = defaults.engine_prewarm;
            Ok(())
        }
        "engine_idle_unload_minutes" => {
            settings.engine_idle_unload_minutes = defaults.engine_idle_unload_minutes;
            Ok(())
        }
        _ => unreachable!("validate_key already checked the setting key"),
    }
}

fn reset_all_settings(current: &mut Settings) -> Result<(), String> {
    let defaults = Settings::default();
    let mut validation = current.clone();
    validation.replace_hotkey_profiles(vec![HotkeyProfile::legacy_default(
        defaults.hotkey.clone(),
        defaults.language,
    )])?;

    let initial_prompt = std::mem::take(&mut current.initial_prompt);
    let profile_glossaries = std::mem::take(&mut current.profile_glossaries);
    let profile_glossary_migrated = current.profile_glossary_migrated;
    let default_profile = HotkeyProfile::legacy_default(defaults.hotkey.clone(), defaults.language);
    let mut profile_models = std::collections::BTreeMap::new();
    profile_models.insert(default_profile.id.clone(), FileModelPreference::Auto);
    *current = Settings {
        initial_prompt,
        profile_glossaries,
        profile_glossary_migrated,
        hotkey_profiles: vec![default_profile],
        profile_models,
        ..defaults
    };
    Ok(())
}

fn cmd_path() -> Result<(), DictationError> {
    println!("{}", settings::store::settings_path().display());
    Ok(())
}

// -- Helpers --

fn validate_key(key: &str) -> Result<(), DictationError> {
    if VALID_KEYS.contains(&key) {
        Ok(())
    } else {
        Err(DictationError::SettingsError(format!(
            "Unknown setting '{key}'. Valid keys: {}",
            VALID_KEYS.join(", ")
        )))
    }
}

fn get_setting_value(settings: &Settings, key: &str) -> String {
    match key {
        "language" => format_language(settings.default_profile().language),
        "whisper_model" => match settings.dictation_model_for_profile(&settings.default_profile().id) {
            Ok(sagascript_core::settings::FileModel::Whisper(model)) => format_model(model),
            Ok(sagascript_core::settings::FileModel::PianissimoOriginal) => "pianissimo-sv".to_string(),
            _ => format_model(WhisperModel::recommended(settings.default_profile().language)),
        },
        "file_transcription_model" => format_file_model(settings.file_transcription_model),
        "hotkey_mode" => format_hotkey_mode(settings.hotkey_mode),
        "show_overlay" => settings.show_overlay.to_string(),
        "auto_paste" => settings.auto_paste.to_string(),
        "auto_select_model" => settings.profile_models.get(&settings.default_profile().id)
            .map_or(settings.auto_select_model, |model| *model == FileModelPreference::Auto).to_string(),
        "pianissimo_dictation" => settings.profile_models.get(&settings.default_profile().id)
            .map_or(settings.pianissimo_dictation, |model| *model == FileModelPreference::PianissimoOriginal).to_string(),
        "hotkey" => settings.hotkey.clone(),
        "initial_prompt" => settings.profile_glossaries.get(&settings.default_profile().id).cloned()
            .unwrap_or_else(|| if settings.profile_glossary_migrated { String::new() } else { settings.initial_prompt.clone() }),
        "beam_size" => settings.beam_size.to_string(),
        "temperature_fallback" => settings.temperature_fallback.to_string(),
        "vad_enabled" => settings.vad_enabled.to_string(),
        "engine_prewarm" => settings.engine_prewarm.id().to_string(),
        "engine_idle_unload_minutes" => settings.engine_idle_unload_minutes.to_string(),
        _ => "unknown".to_string(),
    }
}

fn format_language(lang: Language) -> String {
    serde_json::to_value(lang)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", lang))
}

fn format_file_model(model: FileModelPreference) -> String {
    serde_json::to_value(model)
        .and_then(serde_json::from_value::<String>)
        .expect("file model preference has a stable string ID")
}

fn format_model(model: WhisperModel) -> String {
    serde_json::to_value(model)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", model))
}

fn format_hotkey_mode(mode: HotkeyMode) -> String {
    serde_json::to_value(mode)
        .and_then(serde_json::from_value::<String>)
        .unwrap_or_else(|_| format!("{:?}", mode))
}

fn parse_enum_value<T: serde::de::DeserializeOwned>(
    value: &str,
    key: &str,
) -> Result<T, DictationError> {
    let quoted = format!("\"{}\"", value);
    serde_json::from_str::<T>(&quoted).map_err(|_| {
        DictationError::SettingsError(format!(
            "Invalid value '{value}' for {key}. Run 'sagascript config get {key}' to see current value."
        ))
    })
}

fn parse_bool(value: &str, key: &str) -> Result<bool, DictationError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(DictationError::SettingsError(format!(
            "Invalid value '{value}' for {key}. Must be 'true' or 'false'."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_hotkey_valid_shortcuts() {
        let valid = [
            "Control+Shift+Space",
            "Option+Space",
            "Alt+Space",
            "Command+A",
            "CmdOrCtrl+Space",
            "Ctrl+Shift+Alt+F1",
            "Super+Shift+KeyX",
            "Shift+Enter",
            "Control+Tab",
            "CommandOrControl+Z",
        ];
        for s in valid {
            assert!(validate_hotkey(s).is_ok(), "should be valid: {s}");
        }
    }

    #[test]
    fn validate_hotkey_case_insensitive() {
        assert!(validate_hotkey("control+shift+space").is_ok());
        assert!(validate_hotkey("CONTROL+SHIFT+SPACE").is_ok());
        assert!(validate_hotkey("Control+SHIFT+Space").is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_hotkey_rejects_macos_quit_shortcut_aliases() {
        for shortcut in [
            "Command+Q",
            "Cmd+KeyQ",
            "Super+Q",
            "CmdOrCtrl+Q",
            "Control+Super+Shift+Q",
        ] {
            let error = validate_hotkey(shortcut).unwrap_err();
            assert!(
                error.contains("reserved for Quit on macOS"),
                "unexpected error for {shortcut}: {error}"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_hotkey_rejects_macos_cut_shortcut_before_settings_mutation() {
        let mut settings = Settings::default();
        let original = settings.hotkey.clone();

        let error = apply_setting_value(&mut settings, "hotkey", "Super+X").unwrap_err();

        assert!(error.to_string().contains("reserved for Cut on macOS"));
        assert_eq!(settings.hotkey, original);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reserved_hotkey_is_rejected_before_settings_mutation() {
        let mut settings = Settings::default();
        let original = settings.hotkey.clone();

        let error = apply_setting_value(&mut settings, "hotkey", "Super+Q").unwrap_err();

        assert!(error.to_string().contains("reserved for Quit on macOS"));
        assert_eq!(settings.hotkey, original);
    }

    #[test]
    fn validate_hotkey_rejects_bare_key() {
        let err = validate_hotkey("Space").unwrap_err();
        assert!(err.to_string().contains("modifier is required"));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn apply_setting_value_accepts_bare_extended_function_key() {
        let mut settings = Settings::default();
        apply_setting_value(&mut settings, "hotkey", "F13").unwrap();
        assert_eq!(settings.hotkey, "F13");
        assert_eq!(settings.resolved_hotkey_profiles()[0].shortcut, "F13");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn apply_setting_value_accepts_bare_f24_on_macos() {
        let mut settings = Settings::default();
        apply_setting_value(&mut settings, "hotkey", "F24").unwrap();
        assert_eq!(settings.hotkey, "F24");
    }

    #[test]
    fn validate_hotkey_rejects_unknown_key() {
        let err = validate_hotkey("Control+FooBar").unwrap_err();
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn validate_hotkey_rejects_modifier_as_key() {
        let err = validate_hotkey("Control+Shift").unwrap_err();
        assert!(err.to_string().contains("is a modifier"));
    }

    #[test]
    fn validate_hotkey_rejects_empty() {
        assert!(validate_hotkey("").is_err());
    }

    #[test]
    fn validate_hotkey_rejects_double_plus() {
        assert!(validate_hotkey("Control++Space").is_err());
    }

    #[test]
    fn validate_hotkey_rejects_unknown_modifier() {
        let err = validate_hotkey("Hyper+Space").unwrap_err();
        assert!(err.to_string().contains("unknown modifier"));
    }

    // -- validate_key --

    #[test]
    fn validate_key_accepts_all_valid_keys() {
        for key in VALID_KEYS {
            assert!(validate_key(key).is_ok(), "should accept: {key}");
        }
    }

    #[test]
    fn validate_key_rejects_unknown() {
        assert!(validate_key("nonexistent").is_err());
        assert!(validate_key("").is_err());
        assert!(validate_key("Language").is_err()); // case-sensitive
    }

    #[test]
    fn validate_key_error_lists_valid_keys() {
        let err = validate_key("bogus").unwrap_err();
        let msg = err.to_string();
        for key in VALID_KEYS {
            assert!(
                msg.contains(key),
                "error should list valid key '{key}': {msg}"
            );
        }
    }

    // -- VALID_KEYS exhaustiveness --

    #[test]
    fn valid_keys_matches_settings_fields() {
        // Ensure every VALID_KEY is handled in get_setting_value (not returning "unknown")
        let settings = Settings::default();
        for key in VALID_KEYS {
            let value = get_setting_value(&settings, key);
            assert_ne!(
                value, "unknown",
                "VALID_KEYS contains '{key}' but get_setting_value doesn't handle it"
            );
        }
    }

    #[test]
    fn valid_keys_count_matches_settings_struct() {
        // Internal fields that are serialized but not user-configurable via `config`.
        // These have dedicated CLI commands instead (e.g. `reset-onboarding`).
        const INTERNAL_FIELDS: &[&str] = &[
            "has_completed_onboarding",
            "hotkey_profiles",
            "profile_glossaries",
            "profile_models",
            "profile_glossary_migrated",
            "model_lineup_version",
            "engine_defaults_version",
        ];

        let settings = Settings::default();
        let json = serde_json::to_value(&settings).unwrap();
        let field_count = json.as_object().unwrap().len() - INTERNAL_FIELDS.len();
        assert_eq!(
            VALID_KEYS.len(),
            field_count,
            "VALID_KEYS has {} entries but Settings has {} user-facing fields — did you forget to add a new setting?",
            VALID_KEYS.len(),
            field_count
        );
    }

    // -- parse_enum_value --

    #[test]
    fn parse_enum_value_all_valid_languages() {
        let valid = ["en", "sv", "no", "fi", "auto"];
        for v in valid {
            let result = parse_enum_value::<Language>(v, "language");
            assert!(result.is_ok(), "should parse language '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_language() {
        let result = parse_enum_value::<Language>("de", "language");
        assert!(result.is_err());
    }

    #[test]
    fn language_change_with_profile_dictionary_is_rejected_without_mutation() {
        let mut settings = Settings::default();
        settings.hotkey_profiles = vec![HotkeyProfile {
            id: "default".to_string(),
            name: "Default".to_string(),
            shortcut: settings.hotkey.clone(),
            language: Language::Swedish,
            push_to_talk_shortcut: None,
            toggle_shortcut: None,
        }];
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = apply_setting_value(&mut settings, "language", "en").unwrap_err();

        assert!(error.to_string().contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn reset_all_without_dictionaries_uses_defaults_and_preserves_global_source() {
        let mut settings = Settings {
            language: Language::Swedish,
            initial_prompt: "Codex".to_string(),
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Option+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            }],
            ..Default::default()
        };

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.language, Settings::default().language);
        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(settings.profile_models["default"], FileModelPreference::Auto);
        assert_eq!(settings.initial_prompt, "Codex");
        assert!(settings.profile_glossaries.is_empty());
    }

    #[test]
    fn reset_all_preserves_same_language_active_default_dictionary() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::English,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(settings.language, Language::English);
        assert_eq!(
            settings
                .profile_glossaries
                .get("default")
                .map(String::as_str),
            Some("merge = merch")
        );
    }

    #[test]
    fn reset_all_rejects_default_language_change_with_active_dictionary_atomically() {
        let mut settings = Settings {
            language: Language::Swedish,
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::Swedish,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = reset_all_settings(&mut settings).unwrap_err();

        assert!(error.contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn reset_all_rejects_implicit_swedish_default_dictionary_atomically() {
        let mut settings = Settings {
            language: Language::Swedish,
            hotkey_profiles: Vec::new(),
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".into(), "merge = merch".into());
        let before = settings.clone();
        let error = reset_all_settings(&mut settings).unwrap_err();
        assert!(error.contains("personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
        assert_eq!(settings.initial_prompt, before.initial_prompt);
    }

    #[test]
    fn reset_all_keeps_removed_profile_dictionary_inactive() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile::legacy_default(
                "Option+Space".to_string(),
                Language::English,
            )],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("removed".to_string(), "merge = merch".to_string());

        reset_all_settings(&mut settings).unwrap();

        assert_eq!(settings.hotkey_profiles[0].id, "default");
        assert_eq!(
            settings
                .profile_glossaries
                .get("removed")
                .map(String::as_str),
            Some("merge = merch")
        );
        assert_eq!(settings.effective_glossary_source(Some("removed")), "");
    }

    #[test]
    fn reset_all_rejects_orphan_default_dictionary_atomically() {
        let mut settings = Settings {
            hotkey_profiles: vec![HotkeyProfile {
                id: "swedish".to_string(),
                name: "Swedish".to_string(),
                shortcut: "Option+Space".to_string(),
                language: Language::Swedish,
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
            }],
            ..Default::default()
        };
        settings
            .profile_glossaries
            .insert("default".to_string(), "merge = merch".to_string());
        let before = settings.clone();

        let error = reset_all_settings(&mut settings).unwrap_err();

        assert!(error.contains("inactive personal dictionary"));
        assert_eq!(settings.language, before.language);
        assert_eq!(settings.hotkey_profiles, before.hotkey_profiles);
        assert_eq!(settings.initial_prompt, before.initial_prompt);
        assert_eq!(settings.profile_glossaries, before.profile_glossaries);
    }

    #[test]
    fn parse_enum_value_all_valid_models() {
        let valid = [
            "tiny.en",
            "tiny",
            "base.en",
            "base",
            "kb-whisper-tiny",
            "kb-whisper-base",
            "kb-whisper-small",
            "nb-whisper-tiny",
            "nb-whisper-base",
            "nb-whisper-small",
            "fi-whisper-tiny",
        ];
        for v in valid {
            let result = parse_enum_value::<WhisperModel>(v, "whisper_model");
            assert!(result.is_ok(), "should parse model '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_model() {
        let result = parse_enum_value::<WhisperModel>("large-v3", "whisper_model");
        assert!(result.is_err());
    }

    #[test]
    fn parse_enum_value_all_valid_hotkey_modes() {
        let valid = ["push", "toggle"];
        for v in valid {
            let result = parse_enum_value::<HotkeyMode>(v, "hotkey_mode");
            assert!(result.is_ok(), "should parse hotkey_mode '{v}'");
        }
    }

    #[test]
    fn parse_enum_value_invalid_hotkey_mode() {
        let result = parse_enum_value::<HotkeyMode>("hold", "hotkey_mode");
        assert!(result.is_err());
    }

    #[test]
    fn presenter_hotkey_mode_is_rejected_through_cli_helper() {
        let mut settings = Settings::default();
        let before_mode = settings.hotkey_mode;
        let before_hotkey = settings.hotkey.clone();
        let error = apply_setting_value(&mut settings, "hotkey_mode", "presenter").unwrap_err();
        assert!(error.to_string().contains("Supported values: push, toggle"));
        assert_eq!(settings.hotkey_mode, before_mode);
        assert_eq!(settings.hotkey, before_hotkey);
    }

    // -- parse_bool --

    #[test]
    fn parse_bool_valid() {
        assert!(parse_bool("true", "test").unwrap());
        assert!(!parse_bool("false", "test").unwrap());
    }

    #[test]
    fn parse_bool_rejects_invalid() {
        assert!(parse_bool("yes", "test").is_err());
        assert!(parse_bool("no", "test").is_err());
        assert!(parse_bool("1", "test").is_err());
        assert!(parse_bool("0", "test").is_err());
        assert!(parse_bool("True", "test").is_err());
        assert!(parse_bool("FALSE", "test").is_err());
        assert!(parse_bool("", "test").is_err());
    }

    #[test]
    fn parse_bool_error_message() {
        let err = parse_bool("yes", "auto_paste").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("yes"), "error should mention input: {msg}");
        assert!(
            msg.contains("auto_paste"),
            "error should mention key: {msg}"
        );
    }

    // -- get_setting_value / format helpers --

    #[test]
    fn file_model_setting_is_independent_of_live_model() {
        let mut settings = Settings::default();
        let live = settings.whisper_model;
        apply_setting_value(&mut settings, "file_transcription_model", "pianissimo-sv").unwrap();
        assert_eq!(settings.whisper_model, live);
        assert_eq!(get_setting_value(&settings, "file_transcription_model"), "pianissimo-sv");
        assert!(apply_setting_value(&mut settings, "file_transcription_model", "unknown").is_err());
        assert_eq!(get_setting_value(&settings, "file_transcription_model"), "pianissimo-sv");
    }

    // Selecting Pianissimo is refused unless the platform gate is open.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn pianissimo_dictation_can_be_set_and_reset_to_default() {
        let mut settings = Settings::default();
        assert!(!settings.pianissimo_dictation);
        assert!(apply_setting_value(&mut settings, "pianissimo_dictation", "true").is_err());
        settings.set_default_profile_language(Language::Swedish).unwrap();

        apply_setting_value(&mut settings, "pianissimo_dictation", "true").unwrap();
        assert!(settings.pianissimo_dictation);
        assert_eq!(get_setting_value(&settings, "pianissimo_dictation"), "true");

        let defaults = Settings::default();
        reset_setting_value(&mut settings, "pianissimo_dictation", &defaults).unwrap();
        assert_eq!(settings.pianissimo_dictation, defaults.pianissimo_dictation);
        assert_eq!(
            get_setting_value(&settings, "pianissimo_dictation"),
            defaults.pianissimo_dictation.to_string()
        );
    }

    #[test]
    fn legacy_initial_prompt_alias_never_overwrites_default_profile_aliases() {
        let mut settings = Settings::default();
        settings.profile_glossaries.insert("default".into(), "merge = merch".into());
        apply_setting_value(&mut settings, "initial_prompt", "OpenRouter").unwrap();
        assert!(settings.profile_glossaries["default"].contains("merge = merch"));
        assert!(settings.profile_glossaries["default"].contains("OpenRouter"));
        apply_setting_value(&mut settings, "initial_prompt", "OpenRouter = open router").unwrap();
        assert!(settings.profile_glossaries["default"].contains("OpenRouter = open router"));
        let before = settings.profile_glossaries["default"].clone();
        assert!(reset_setting_value(&mut settings, "initial_prompt", &Settings::default()).is_err());
        assert_eq!(settings.profile_glossaries["default"], before);
    }

    #[test]
    fn pianissimo_dictation_rejects_invalid_boolean_without_mutating() {
        let mut settings = Settings {
            pianissimo_dictation: true,
            ..Settings::default()
        };

        for value in ["yes", "1", "TRUE"] {
            let error =
                apply_setting_value(&mut settings, "pianissimo_dictation", value).unwrap_err();
            let message = error.to_string();
            assert!(message.contains(value));
            assert!(message.contains("pianissimo_dictation"));
            assert!(settings.pianissimo_dictation);
        }
    }

    #[test]
    fn get_setting_value_returns_serialized_values() {
        let settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "language"), "en");
        assert_eq!(get_setting_value(&settings, "hotkey_mode"), "push");
        assert_eq!(get_setting_value(&settings, "show_overlay"), "true");
        assert_eq!(get_setting_value(&settings, "auto_paste"), "true");
        assert_eq!(get_setting_value(&settings, "auto_select_model"), "true");
        assert_eq!(
            get_setting_value(&settings, "pianissimo_dictation"),
            "false"
        );
        assert_eq!(
            get_setting_value(&settings, "hotkey"),
            "Control+Shift+Space"
        );
        assert_eq!(get_setting_value(&settings, "initial_prompt"), "");
        assert_eq!(get_setting_value(&settings, "beam_size"), "0");
        assert_eq!(get_setting_value(&settings, "temperature_fallback"), "true");
        assert_eq!(get_setting_value(&settings, "vad_enabled"), "false");
    }

    #[test]
    fn engine_settings_can_be_set_reset_and_reject_bad_values() {
        let mut settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "engine_prewarm"), "on_app_start");
        assert_eq!(get_setting_value(&settings, "engine_idle_unload_minutes"), "60");

        apply_setting_value(&mut settings, "engine_prewarm", "on_app_start").unwrap();
        apply_setting_value(&mut settings, "engine_idle_unload_minutes", "3").unwrap();
        assert_eq!(get_setting_value(&settings, "engine_prewarm"), "on_app_start");
        assert_eq!(get_setting_value(&settings, "engine_idle_unload_minutes"), "3");

        assert!(apply_setting_value(&mut settings, "engine_prewarm", "always").is_err());
        assert!(apply_setting_value(&mut settings, "engine_idle_unload_minutes", "-1").is_err());
        assert!(apply_setting_value(&mut settings, "engine_idle_unload_minutes", "ten").is_err());
        assert_eq!(get_setting_value(&settings, "engine_prewarm"), "on_app_start");

        let defaults = Settings::default();
        reset_setting_value(&mut settings, "engine_prewarm", &defaults).unwrap();
        reset_setting_value(&mut settings, "engine_idle_unload_minutes", &defaults).unwrap();
        assert_eq!(get_setting_value(&settings, "engine_prewarm"), "on_app_start");
        assert_eq!(get_setting_value(&settings, "engine_idle_unload_minutes"), "60");
    }

    #[test]
    fn get_setting_value_unknown_key_returns_unknown() {
        let settings = Settings::default();
        assert_eq!(get_setting_value(&settings, "nonexistent"), "unknown");
    }

    #[test]
    fn enabling_auto_paste_warns_about_gui_accessibility_requirement() {
        let settings = Settings::default();
        let warning = setting_warning("auto_paste", &settings).unwrap();
        assert!(warning.contains("Accessibility approval"));
        assert!(warning.contains("reset auto-paste to false"));

        let disabled = Settings {
            auto_paste: false,
            ..Default::default()
        };
        assert!(setting_warning("auto_paste", &disabled).is_none());
        assert!(setting_warning("show_overlay", &Settings::default()).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bare_extended_hotkey_warns_about_gui_accessibility_requirement() {
        let settings = Settings {
            hotkey: "F24".to_string(),
            ..Default::default()
        };
        let warning = setting_warning("hotkey", &settings).unwrap();
        assert!(warning.contains("F13-F24"));
        assert!(warning.contains("Accessibility approval"));

        assert!(bare_extended_hotkey_warning("Shift+F24").is_none());
        assert!(bare_extended_hotkey_warning("F013").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn brightness_key_hotkey_warns_only_for_bare_f14_and_f15() {
        assert!(brightness_key_hotkey_warning("F14").unwrap().contains("brightness"));
        assert!(brightness_key_hotkey_warning(" f15 ").is_some());
        assert!(brightness_key_hotkey_warning("F13").is_none());
        assert!(brightness_key_hotkey_warning("F16").is_none());
        assert!(brightness_key_hotkey_warning("Shift+F14").is_none());
        assert!(brightness_key_hotkey_warning("F140").is_none());
        assert!(shortcut_warning("F14").unwrap().contains("brightness"));
        assert!(shortcut_warning("F13").unwrap().contains("Accessibility"));
    }
}
