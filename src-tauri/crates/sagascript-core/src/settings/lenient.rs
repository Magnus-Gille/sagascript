//! Lenient per-field decoding of the settings object (ADR 0007).
//!
//! One value the current [`Settings`] cannot read, such as a model removed
//! from [`WhisperModel`](super::WhisperModel), a language a newer release
//! added or a hand-edited wrong type, used to load the whole file as
//! defaults, and the next save wrote those defaults over every other value
//! (#340). Instead only that top-level field takes its default. A field is
//! the unit: a bad value nested inside one (say, one profile's language)
//! resets the whole field, so no half-read structure is ever kept.
//!
//! The store reports the reset fields by name, never by value. The file
//! itself is not strictly valid, so every save first preserves its exact
//! bytes (see `store::save_to`) and the dropped value stays recoverable.

use serde_json::Value;

use super::{migrations::Object, Settings};

/// Settings decoded from a settings object.
pub(crate) struct Decoded {
    pub settings: Settings,
    /// Top-level fields that did not decode and took their defaults, sorted.
    /// Empty when the object decoded strictly.
    pub reset: Vec<String>,
}

/// Decode `object` as [`Settings`]. A strictly valid object decodes as it
/// is. Otherwise each top-level field that does not decode on its own is
/// reset and the rest decodes; unknown keys never count as invalid. Fails
/// only if the object is invalid without any single field to blame.
pub(crate) fn decode(object: &Object) -> Result<Decoded, String> {
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
        return Err(strict_error.to_string());
    }
    reset.sort();
    let mut rest = object.clone();
    for key in &reset {
        rest.remove(key);
    }
    let settings =
        serde_json::from_value(Value::Object(rest)).map_err(|error| error.to_string())?;
    Ok(Decoded { settings, reset })
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
            "hotkey_profiles": [
                {"id": "default", "name": "Svenska", "shortcut": "Option+Space", "language": "sv"},
                {"id": "german", "name": "Deutsch", "shortcut": "Option+D", "language": "de"}
            ]
        })))
        .unwrap();
        assert_eq!(decoded.reset, ["hotkey_profiles"]);
        assert!(decoded.settings.hotkey_profiles.is_empty());
        assert_eq!(decoded.settings.language, Language::Swedish);
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
