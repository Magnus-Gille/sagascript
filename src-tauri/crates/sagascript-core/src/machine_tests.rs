use super::*;
use crate::settings::HotkeyProfile;

/// Each ungated `Settings` method equals its `_gated` sibling called with
/// this machine's gate, whatever that is.
///
/// What this catches depends on the machine it runs on, because the gate is a
/// fact about the machine: a wrapper that hard-codes `true` fails wherever
/// Pianissimo cannot run (Linux, Windows x64, Intel macOS), one that hard-codes
/// `false` fails only where it can (Apple silicon on macOS 14+, Windows on
/// ARM). On Linux alone a hard-coded `false` passes; the macOS lane of CI is
/// what covers it when its runners are Apple silicon.
#[test]
fn each_wrapper_equals_its_gated_sibling_under_this_machines_gate() {
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
/// can be compared with their `_gated` siblings without looking at the models
/// directory. As above, only the platform gate is exercised, on whatever
/// machine this runs.
#[test]
fn the_file_model_wrappers_equal_their_gated_siblings_for_explicit_choices() {
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

/// The one platform where the gate has a known value in every build: Pianissimo
/// does not run on Linux. A wrapper that hard-codes `true` is red here, which
/// the comparison test above cannot say on any other machine.
#[cfg(target_os = "linux")]
#[test]
fn on_linux_the_gate_is_closed_and_swedish_auto_is_a_whisper_model() {
    assert!(!pianissimo_supported());
    assert!(!ThisMachine.pianissimo_supported());
    let swedish = Settings {
        hotkey_profiles: vec![HotkeyProfile::legacy_default(
            "Super+S".into(),
            Language::Swedish,
        )],
        profile_models: [("default".to_string(), FileModelPreference::Auto)].into(),
        ..Settings::default()
    };
    assert_eq!(
        swedish.dictation_model_for_profile("default"),
        Ok(FileModel::Whisper(WhisperModel::KbWhisperMedium))
    );
    assert!(!swedish.any_profile_uses_pianissimo());
    let mut selecting = swedish;
    assert_eq!(
        selecting.set_profile_model("default", FileModelPreference::PianissimoOriginal),
        Err(crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE.to_string())
    );
}
