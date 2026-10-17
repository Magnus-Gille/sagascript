use super::*;

#[test]
fn language_default_is_english() {
    assert_eq!(Language::default(), Language::English);
}

#[test]
fn language_display_names() {
    assert_eq!(Language::English.display_name(), "English");
    assert_eq!(Language::Swedish.display_name(), "Swedish");
    assert_eq!(Language::Norwegian.display_name(), "Norwegian");
    assert_eq!(Language::Finnish.display_name(), "Finnish");
    assert_eq!(Language::Auto.display_name(), "Auto-detect");
}

#[test]
fn language_whisper_codes() {
    assert_eq!(Language::English.whisper_code(), Some("en"));
    assert_eq!(Language::Swedish.whisper_code(), Some("sv"));
    assert_eq!(Language::Norwegian.whisper_code(), Some("no"));
    assert_eq!(Language::Finnish.whisper_code(), Some("fi"));
    assert_eq!(Language::Auto.whisper_code(), None);
}

#[test]
fn language_serde_roundtrip() {
    let lang = Language::Swedish;
    let json = serde_json::to_string(&lang).unwrap();
    assert_eq!(json, "\"sv\"");
    let deserialized: Language = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, lang);
}

#[test]
fn language_serde_all_variants() {
    let pairs = [
        (Language::English, "\"en\""),
        (Language::Swedish, "\"sv\""),
        (Language::Norwegian, "\"no\""),
        (Language::Finnish, "\"fi\""),
        (Language::Auto, "\"auto\""),
    ];
    for (lang, expected) in pairs {
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, expected, "serialize {:?}", lang);
        let back: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang, "deserialize {:?}", lang);
    }
}
