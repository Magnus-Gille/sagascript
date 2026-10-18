//! Lenient per-field decoding of the settings object (ADR 0007).
//!
//! One value the current [`Settings`] cannot read, such as a model removed
//! from [`WhisperModel`](super::WhisperModel), a language a newer release
//! added or a hand-edited wrong type, used to load the whole file as
//! defaults, and the next save wrote those defaults over every other value
//! (#340). Instead only that top-level field takes its default. A field is
//! the unit: a bad value nested inside one resets the whole field, so no
//! half-read structure is kept. The exception is `hotkey_profiles`, read
//! profile by profile (see [`readable_profiles`]): one profile with, say, a
//! language a newer release added does not cost the user the others. The
//! store treats `profile_models` per entry too.
//!
//! The store reports the reset fields by name, never by value. The file
//! itself is not strictly valid, so every save first preserves its exact
//! bytes (see `store::save_to`) and the dropped value stays recoverable.

use serde_json::Value;

use super::{migrations::Object, HotkeyProfile, Settings};

/// Settings decoded from a settings object.
pub(crate) struct Decoded {
    pub settings: Settings,
    /// Top-level fields that did not decode and took their defaults, sorted.
    /// Empty when the object decoded strictly.
    pub reset: Vec<String>,
}

const HOTKEY_PROFILES: &str = "hotkey_profiles";

/// Decode `object` as [`Settings`]. A strictly valid object decodes as it
/// is. Otherwise each top-level field that does not decode on its own is
/// reset (`hotkey_profiles` keeps the profiles [`readable_profiles`] can
/// read) and the rest decodes; unknown keys never count as invalid. Fails
/// only if the object is invalid without any single field to blame.
pub(crate) fn decode(object: &Object) -> Result<Decoded, serde_json::Error> {
    let strict_error = match serde_json::from_value(Value::Object(object.clone())) {
        Ok(settings) => {
            return Ok(Decoded {
                settings,
                reset: Vec::new(),
            })
        }
        Err(error) => error,
    };
    let mut reset: Vec<String> = object
        .iter()
        .filter(|(key, value)| !decodes_alone(key, value))
        .map(|(key, _)| key.clone())
        .collect();
    if reset.is_empty() {
        return Err(strict_error);
    }
    reset.sort();
    let mut rest = object.clone();
    for key in &reset {
        rest.remove(key);
    }
    let mut settings: Settings = serde_json::from_value(Value::Object(rest))?;
    if let Some(profiles) = object
        .get(HOTKEY_PROFILES)
        .filter(|_| reset.iter().any(|key| key == HOTKEY_PROFILES))
    {
        settings.hotkey_profiles = readable_profiles(profiles, &settings);
    }
    Ok(Decoded { settings, reset })
}

/// The profiles of a `hotkey_profiles` value that does not decode, read one
/// by one. A profile that does not read is dropped, except the primary one
/// (`default`, else the first), whose shortcut and language `hotkey` and
/// `language` mirror: dropping it would leave `hotkey` naming a shortcut
/// that nothing registers. It is kept, and each of its fields that does not
/// read is the one a file without profiles gives it
/// ([`HotkeyProfile::legacy_default`] of `settings`). If even that fails,
/// or the value is not a list, the whole field resets (no profiles).
fn readable_profiles(value: &Value, settings: &Settings) -> Vec<HotkeyProfile> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    let primary = entries
        .iter()
        .position(|entry| entry.get("id").and_then(Value::as_str) == Some("default"))
        .unwrap_or(0);
    let mut profiles = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        match serde_json::from_value(entry.clone()) {
            Ok(profile) => profiles.push(profile),
            Err(_) if index == primary => match repaired_profile(entry, settings) {
                Some(profile) => profiles.push(profile),
                None => return Vec::new(),
            },
            Err(_) => {}
        }
    }
    profiles
}

/// [`HotkeyProfile::legacy_default`] of `settings` with every field of
/// `entry` that reads in its place.
fn repaired_profile(entry: &Value, settings: &Settings) -> Option<HotkeyProfile> {
    let fallback = HotkeyProfile::legacy_default(settings.hotkey.clone(), settings.language);
    let Ok(Value::Object(mut repaired)) = serde_json::to_value(fallback) else {
        return None;
    };
    for (key, value) in entry.as_object()? {
        let mut candidate = repaired.clone();
        candidate.insert(key.clone(), value.clone());
        if serde_json::from_value::<HotkeyProfile>(Value::Object(candidate)).is_ok() {
            repaired.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(Value::Object(repaired)).ok()
}

/// Whether `key: value` decodes as [`Settings`] with every other field at
/// its default.
fn decodes_alone(key: &str, value: &Value) -> bool {
    let mut single = Object::new();
    single.insert(key.to_string(), value.clone());
    serde_json::from_value::<Settings>(Value::Object(single)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Language, WhisperModel};
    use serde_json::json;

    fn object(value: Value) -> Object {
        match value {
            Value::Object(object) => object,
            other => panic!("not an object: {other}"),
        }
    }

    #[test]
    fn a_strictly_valid_object_decodes_as_it_is() {
        let valid = object(json!({"language": "sv", "beam_size": 5, "future_key": [1]}));
        let decoded = decode(&valid).unwrap();
        assert!(decoded.reset.is_empty());
        let strict: Settings = serde_json::from_value(Value::Object(valid)).unwrap();
        assert_eq!(
            serde_json::to_value(decoded.settings).unwrap(),
            serde_json::to_value(strict).unwrap()
        );
    }

    #[test]
    fn invalid_fields_reset_alone_and_unknown_keys_are_not_invalid() {
        let decoded = decode(&object(json!({
            "language": "sv",
            "whisper_model": "kb-whisper-xl-2027",
            "show_overlay": "no",
            "beam_size": 5,
            "newer_release_key": {"x": 1}
        })))
        .unwrap();
        assert_eq!(decoded.reset, ["show_overlay", "whisper_model"]);
        assert_eq!(decoded.settings.language, Language::Swedish);
        assert_eq!(decoded.settings.beam_size, 5);
        assert_eq!(decoded.settings.whisper_model, WhisperModel::default());
        assert!(decoded.settings.show_overlay);
    }

    #[test]
    fn a_bad_nested_value_resets_its_whole_field() {
        let decoded = decode(&object(json!({
            "language": "sv",
            "profile_glossaries": {"default": "merge = merch", "english": 5}
        })))
        .unwrap();
        assert_eq!(decoded.reset, ["profile_glossaries"]);
        assert!(decoded.settings.profile_glossaries.is_empty());
        assert_eq!(decoded.settings.language, Language::Swedish);
    }

    #[test]
    fn an_unreadable_profile_is_dropped_alone() {
        let svenska = json!({"id": "default", "name": "Svenska", "shortcut": "Option+Space", "language": "sv"});
        let deutsch =
            json!({"id": "german", "name": "Deutsch", "shortcut": "Option+D", "language": "de"});
        let english =
            json!({"id": "english", "name": "English", "shortcut": "Option+E", "language": "en"});
        let decoded = decode(&object(json!({
            "language": "sv",
            "hotkey_profiles": [svenska, deutsch, english, "not a profile"]
        })))
        .unwrap();
        assert_eq!(decoded.reset, ["hotkey_profiles"]);
        let ids: Vec<&str> = decoded
            .settings
            .hotkey_profiles
            .iter()
            .map(|profile| profile.id.as_str())
            .collect();
        assert_eq!(ids, ["default", "english"]);
        assert_eq!(decoded.settings.language, Language::Swedish);

        // Without a list at all, the whole field resets.
        let decoded = decode(&object(json!({"hotkey_profiles": "not a list"}))).unwrap();
        assert_eq!(decoded.reset, ["hotkey_profiles"]);
        assert!(decoded.settings.hotkey_profiles.is_empty());
    }

    #[test]
    fn the_primary_profile_is_repaired_not_dropped() {
        let unreadable_default = json!({"id": "default", "name": "Klingon", "shortcut": "Super+K", "push_to_talk_shortcut": "Super+P", "language": "tlh"});
        let english =
            json!({"id": "english", "name": "English", "shortcut": "Super+E", "language": "en"});
        for (language, expected) in [
            (json!("sv"), Language::Swedish),
            (json!("tlh"), Language::default()),
        ] {
            let decoded = decode(&object(json!({
                "language": language,
                "hotkey": "Super+K",
                "hotkey_profiles": [english, unreadable_default]
            })))
            .unwrap();
            assert!(decoded.reset.contains(&"hotkey_profiles".to_string()));
            let profiles = &decoded.settings.hotkey_profiles;
            assert_eq!(profiles.len(), 2);
            // Its unreadable language is the one `language` gives it.
            let primary = &profiles[1];
            assert_eq!(
                (
                    primary.id.as_str(),
                    primary.name.as_str(),
                    primary.shortcut.as_str()
                ),
                ("default", "Klingon", "Super+K")
            );
            assert_eq!(primary.push_to_talk_shortcut.as_deref(), Some("Super+P"));
            assert_eq!(primary.language, expected);
        }

        // Without a `default` profile, the first one is the primary.
        let decoded = decode(&object(json!({
            "hotkey": "Super+K",
            "hotkey_profiles": [{"id": "work", "name": 7, "shortcut": "Super+K", "language": "tlh"}, english]
        })))
        .unwrap();
        let first = &decoded.settings.hotkey_profiles[0];
        assert_eq!(
            (first.id.as_str(), first.name.as_str()),
            ("work", "Default")
        );

        // A primary entry that is not a profile at all resets the field.
        let decoded = decode(&object(json!({"hotkey_profiles": ["default", english]}))).unwrap();
        assert!(decoded.settings.hotkey_profiles.is_empty());
    }

    #[test]
    fn an_object_invalid_only_as_a_whole_is_an_error() {
        // Each key is fine alone; together they name one field twice.
        let conflict = object(json!({
            "hasCompletedOnboarding": true,
            "has_completed_onboarding": false
        }));
        assert!(decode(&conflict).is_err());
    }
}
