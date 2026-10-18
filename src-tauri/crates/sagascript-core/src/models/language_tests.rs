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

#[test]
fn language_ids_are_the_serde_names_and_round_trip() {
    for language in Language::ALL {
        let json = serde_json::to_string(&language).unwrap();
        assert_eq!(json, format!("\"{}\"", language.id()), "{language:?}");
        assert_eq!(Language::from_id(language.id()), Some(language));
    }
    assert_eq!(Language::from_id("english"), None, "an id is not a name");
    assert_eq!(Language::from_id("EN"), None);
    assert_eq!(Language::from_id(""), None);
}

#[test]
fn language_all_lists_every_variant_once() {
    // A new variant fails to compile here until it is in `ALL` and has an id.
    for language in Language::ALL {
        match language {
            Language::English
            | Language::Swedish
            | Language::Norwegian
            | Language::Finnish
            | Language::Auto => {}
        }
    }
    let mut ids: Vec<_> = Language::ALL.iter().map(|language| language.id()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), Language::ALL.len());
}

#[test]
fn language_from_str_takes_the_id_or_the_english_name_and_nothing_else() {
    use std::str::FromStr;

    for (text, expected) in [
        ("en", Language::English),
        ("english", Language::English),
        ("sv", Language::Swedish),
        ("swedish", Language::Swedish),
        ("no", Language::Norwegian),
        ("norwegian", Language::Norwegian),
        ("fi", Language::Finnish),
        ("finnish", Language::Finnish),
        ("auto", Language::Auto),
    ] {
        assert_eq!(Language::from_str(text), Ok(expected), "{text}");
    }
    for text in [
        "",
        " en",
        "en ",
        "EN",
        "English",
        "ENGLISH",
        "de",
        "svenska",
        "auto-detect",
        "\"sv\"",
        "\\u0073v",
    ] {
        assert_eq!(
            Language::from_str(text),
            Err(UnknownLanguage(text.to_owned())),
            "{text:?}"
        );
    }
}

#[test]
fn unknown_language_message_lists_the_ids() {
    assert_eq!(
        UnknownLanguage("de".into()).to_string(),
        "Unknown language 'de'. Valid: en, sv, no, fi, auto"
    );
}
