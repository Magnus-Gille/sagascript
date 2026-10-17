//! Settings fixtures shared by the commands' test files.

use sagascript_core::settings::{HotkeyProfile, Language, Settings};

/// Three profiles (Swedish, English, Auto-detect) with scoped dictionaries and
/// a global one, the shape the file-transcription and dictionary tests need.
pub(super) fn scoped_file_settings() -> Settings {
    let mut settings = Settings {
        language: Language::English,
        initial_prompt: "Codex = code x\nmerge = merch".into(),
        hotkey_profiles: vec![
            HotkeyProfile {
                id: "swedish".into(),
                name: "Swedish".into(),
                shortcut: "Super+S".into(),
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
                language: Language::Swedish,
            },
            HotkeyProfile {
                id: "english".into(),
                name: "English".into(),
                shortcut: "Super+E".into(),
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
                language: Language::English,
            },
            HotkeyProfile {
                id: "automatic".into(),
                name: "Automatic".into(),
                shortcut: "Super+A".into(),
                push_to_talk_shortcut: None,
                toggle_shortcut: None,
                language: Language::Auto,
            },
        ],
        ..Settings::default()
    };
    settings.profile_glossaries.insert(
        "swedish".into(),
        "merge = merch\nOpenRouter = open router".into(),
    );
    settings
        .profile_glossaries
        .insert("automatic".into(), "merge = merch".into());
    settings
}
