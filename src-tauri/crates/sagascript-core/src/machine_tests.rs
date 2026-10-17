use super::*;
use crate::settings::HotkeyProfile;

/// The platform gate of the wrappers is the gate of this machine, whatever it
/// is: each one equals its `_gated` sibling given `pianissimo_supported()`.
/// Red if a wrapper hard-codes the gate: on a machine that cannot run
/// Pianissimo (Linux, Intel macOS) `true` shows in the Swedish `Auto` rows, and
/// on one that can `false` does.
#[test]
fn the_wrappers_ask_this_machines_platform_gate() {
    let supported = pianissimo_supported();
    assert_eq!(
        supported,
        pianissimo_backend::runtime_supported_on_this_os()
    );
    assert_eq!(ThisMachine.pianissimo_supported(), supported);

    let mut settings = Settings {
        hotkey_profiles: vec![
            HotkeyProfile::legacy_default("Super+S".into(), Language::Swedish),
            HotkeyProfile {
                id: "english".into(),
                ..HotkeyProfile::legacy_default("Super+E".into(), Language::English)
            },
        ],
        ..Settings::default()
    };
    settings
        .profile_models
        .insert("default".into(), FileModelPreference::Auto);
    settings
        .profile_models
        .insert("english".into(), FileModelPreference::Auto);

    for id in ["default", "english", "missing"] {
        assert_eq!(
            settings.dictation_model_for_profile(id),
            settings.dictation_model_for_profile_gated(id, supported)
        );
        assert_eq!(
            settings.live_model_for_profile(id),
            settings.live_model_for_profile_gated(id, supported)
        );
        assert_eq!(
            settings.profile_uses_pianissimo(id),
            settings.profile_uses_pianissimo_gated(id, supported)
        );
    }
    assert_eq!(
        settings.any_profile_uses_pianissimo(),
        settings.any_profile_uses_pianissimo_gated(supported)
    );
    assert_eq!(
        settings.warm_model_plan(4, 10_000),
        settings.warm_model_plan_gated(4, 10_000, supported)
    );
    // The Swedish `Auto` profile is where the gate shows.
    assert_eq!(
        settings.dictation_model_for_profile("default"),
        Ok(if supported {
            FileModel::PianissimoOriginal
        } else {
            FileModel::Whisper(WhisperModel::KbWhisperMedium)
        })
    );

    let mut by_wrapper = settings.clone();
    let mut by_gated = settings.clone();
    assert_eq!(
        by_wrapper.set_profile_model("default", FileModelPreference::PianissimoOriginal),
        by_gated.set_profile_model_gated(
            "default",
            FileModelPreference::PianissimoOriginal,
            supported
        )
    );
    assert_eq!(by_wrapper.profile_models, by_gated.profile_models);

    let mut by_wrapper = settings.clone();
    let mut by_gated = settings.clone();
    assert_eq!(
        by_wrapper.set_default_profile_language(Language::Norwegian),
        by_gated.set_default_profile_language_gated(Language::Norwegian, supported)
    );
    let profiles = settings.resolved_hotkey_profiles();
    assert_eq!(
        by_wrapper.replace_hotkey_profiles(profiles.clone()),
        by_gated.replace_hotkey_profiles_gated(profiles, supported)
    );
}

/// An explicit file model never consults readiness, so the file-model wrappers
/// can be compared without looking at the models directory.
#[test]
fn the_file_model_wrappers_ask_this_machines_gate_for_explicit_choices() {
    let supported = pianissimo_supported();
    let explicit = |preference| Settings {
        file_transcription_model: preference,
        ..Settings::default()
    };
    for (preference, language) in [
        (FileModelPreference::PianissimoOriginal, Language::Swedish),
        (FileModelPreference::PianissimoOriginal, Language::English),
        (
            FileModelPreference::Whisper(WhisperModel::KbWhisperLarge),
            Language::Swedish,
        ),
    ] {
        let settings = explicit(preference);
        let never_asked = |_: FileModel| unreachable!("an explicit choice needs no readiness");
        assert_eq!(
            settings.effective_file_model_for(language),
            settings.effective_file_model_for_gated(language, supported, &never_asked)
        );
        assert_eq!(
            settings.effective_whisper_file_model_for(language),
            settings.effective_whisper_file_model_for_gated(language, &never_asked)
        );
    }
}
