//! Table tests over the injected probes (#372): every answer the settings code
//! used to get by asking the machine (what is downloaded, whether Pianissimo
//! runs, whether it is downloaded) is a row's input here, and no row touches
//! the real machine or the user's settings.

use super::*;
use crate::models::{FileModel, Language, WhisperModel};
use crate::settings::{store, FileModelPreference, HotkeyProfile, Settings};
use sagascript_testsupport::fake_settings;
use std::fs;

const KB_SMALL: WhisperModel = WhisperModel::KbWhisperSmall;
const KB_MEDIUM: WhisperModel = WhisperModel::KbWhisperMedium;
const KB_LARGE: WhisperModel = WhisperModel::KbWhisperLarge;

/// A settings file from before the Swedish lineup change: one Swedish profile
/// on a model the lineup retired.
const LEGACY_SWEDISH: &str = r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"kb-whisper-small"}}"#;

/// The same file after the lineup change, with Pianissimo chosen on a machine
/// that supported it.
const PIANISSIMO_SWEDISH: &str = r#"{"language":"sv","hotkey":"Super+S","profile_glossary_migrated":true,"model_lineup_version":1,"engine_defaults_version":1,"schema_version":6,"hotkey_profiles":[{"id":"default","name":"Swedish","shortcut":"Super+S","language":"sv"}],"profile_models":{"default":"pianissimo-sv"},"file_transcription_model":"pianissimo-sv","pianissimo_dictation":true}"#;

fn load_with_probes(
    file: &str,
    pianissimo_supported: bool,
    pianissimo_downloaded: bool,
    downloaded: &[WhisperModel],
) -> (Settings, String) {
    let settings = fake_settings();
    fs::write(settings.path(), file).unwrap();
    let is_downloaded = |model: WhisperModel| downloaded.contains(&model);
    let env = LineupEnv {
        pianissimo_supported,
        pianissimo_downloaded,
        whisper_downloaded: &is_downloaded,
    };
    let loaded = store::load_at(settings.path(), &[], &env);
    (loaded, fs::read_to_string(settings.path()).unwrap())
}

/// Red if the load ignores a probe: each row differs from its neighbours in
/// exactly the probe that decides it.
#[test]
fn the_startup_load_resolves_the_lineup_from_the_injected_probes() {
    use FileModelPreference::{PianissimoOriginal as Pianissimo, Whisper};
    // (Pianissimo supported, Pianissimo downloaded, Whisper models downloaded, expected)
    let rows: [(bool, bool, &[WhisperModel], FileModelPreference); 9] = [
        // Supported: the target is Pianissimo, and it moves a profile when it
        // is downloaded or when the old model is not usable either...
        (true, true, &[], Pianissimo),
        (true, true, &[KB_SMALL], Pianissimo),
        (true, false, &[], Pianissimo),
        // ...but a downloaded old model stays while the target is missing.
        (true, false, &[KB_SMALL], Whisper(KB_SMALL)),
        // Unsupported: the target is KB-Whisper Medium; Pianissimo's download
        // state no longer matters.
        (false, true, &[KB_MEDIUM], Whisper(KB_MEDIUM)),
        (false, false, &[KB_MEDIUM], Whisper(KB_MEDIUM)),
        (false, true, &[], Whisper(KB_MEDIUM)),
        (false, true, &[KB_SMALL], Whisper(KB_SMALL)),
        (false, false, &[KB_SMALL, KB_MEDIUM], Whisper(KB_MEDIUM)),
    ];
    for (supported, pianissimo_downloaded, downloaded, expected) in rows {
        let (loaded, _) =
            load_with_probes(LEGACY_SWEDISH, supported, pianissimo_downloaded, downloaded);
        assert_eq!(
            loaded.profile_models["default"], expected,
            "supported={supported} pianissimo_downloaded={pianissimo_downloaded} downloaded={downloaded:?}"
        );
    }
}

/// A file copied from a Pianissimo machine loads as Auto where Pianissimo
/// cannot run, in memory only: the file keeps the choice.
#[test]
fn an_unsupported_machine_demotes_pianissimo_in_memory_and_leaves_the_file_alone() {
    for (supported, expected_profile, expected_file, expected_flag) in [
        (
            true,
            FileModelPreference::PianissimoOriginal,
            FileModelPreference::PianissimoOriginal,
            true,
        ),
        (
            false,
            FileModelPreference::Auto,
            FileModelPreference::Auto,
            false,
        ),
    ] {
        // Whether Pianissimo is downloaded cannot matter: the lineup step has run.
        for pianissimo_downloaded in [true, false] {
            let (loaded, on_disk) =
                load_with_probes(PIANISSIMO_SWEDISH, supported, pianissimo_downloaded, &[]);
            assert_eq!(loaded.profile_models["default"], expected_profile);
            assert_eq!(loaded.file_transcription_model, expected_file);
            assert_eq!(loaded.pianissimo_dictation, expected_flag);
            assert_eq!(
                on_disk, PIANISSIMO_SWEDISH,
                "the file must not be rewritten"
            );
        }
    }
}

/// A Swedish profile on Auto, plus one Norwegian profile on a Norwegian model.
fn two_language_settings() -> Settings {
    let mut norwegian = HotkeyProfile::legacy_default("Super+N".into(), Language::Norwegian);
    norwegian.id = "norwegian".into();
    Settings {
        hotkey_profiles: vec![
            HotkeyProfile::legacy_default("Super+S".into(), Language::Swedish),
            norwegian,
        ],
        profile_models: [
            ("default".to_string(), FileModelPreference::Auto),
            (
                "norwegian".to_string(),
                FileModelPreference::Whisper(WhisperModel::NbWhisperBase),
            ),
        ]
        .into(),
        ..Settings::default()
    }
}

#[test]
fn the_profile_methods_follow_the_injected_platform_gate() {
    for (supported, engine, pianissimo) in [
        (true, FileModel::PianissimoOriginal, true),
        (false, FileModel::Whisper(KB_MEDIUM), false),
    ] {
        let settings = two_language_settings();
        assert_eq!(
            settings.dictation_model_for_profile_gated("default", supported),
            Ok(engine),
            "supported={supported}"
        );
        assert_eq!(
            settings.live_model_for_profile_gated("default", supported),
            Ok((KB_MEDIUM, pianissimo)),
            "supported={supported}"
        );
        assert_eq!(
            settings.profile_uses_pianissimo_gated("default", supported),
            pianissimo
        );
        // Norwegian profiles never use it.
        assert!(!settings.profile_uses_pianissimo_gated("norwegian", supported));
        assert_eq!(
            settings.any_profile_uses_pianissimo_gated(supported),
            pianissimo
        );
    }
}

#[test]
fn a_language_change_names_the_recommendation_of_the_injected_gate() {
    for (supported, recommended) in [(true, "Pianissimo"), (false, "KB-Whisper Medium")] {
        let mut settings = two_language_settings();
        let mut profiles = settings.resolved_hotkey_profiles();
        profiles[1].language = Language::Swedish;
        let notices = settings
            .replace_hotkey_profiles_gated(profiles, supported)
            .unwrap();
        assert_eq!(notices.len(), 1, "supported={supported}");
        assert_eq!(notices[0].profile_id, "norwegian");
        assert_eq!(
            notices[0].message,
            format!(
                "Model for profile 'Default' changed to Recommended ({recommended}) because NB-Whisper Base does not support Swedish."
            ),
            "supported={supported}"
        );
        assert_eq!(
            settings.profile_models["norwegian"],
            FileModelPreference::Auto
        );
        // The same change through the default profile.
        let mut by_default = two_language_settings();
        by_default
            .set_default_profile_language_gated(Language::Norwegian, supported)
            .unwrap();
        assert_eq!(
            by_default.resolved_hotkey_profiles()[0].language,
            Language::Norwegian
        );
    }
}

#[test]
fn selecting_pianissimo_needs_the_injected_gate() {
    for (supported, expected) in [
        (true, Ok(())),
        (
            false,
            Err(crate::models::PIANISSIMO_UNSUPPORTED_MESSAGE.to_string()),
        ),
    ] {
        let mut settings = two_language_settings();
        assert_eq!(
            settings.set_profile_model_gated(
                "default",
                FileModelPreference::PianissimoOriginal,
                supported
            ),
            expected,
            "supported={supported}"
        );
    }
}

#[test]
fn the_warm_model_plan_leaves_out_pianissimo_profiles_only_where_it_runs() {
    for (supported, expected) in [
        (
            true,
            vec![(WhisperModel::NbWhisperBase, Language::Norwegian)],
        ),
        (
            false,
            vec![
                (KB_MEDIUM, Language::Swedish),
                (WhisperModel::NbWhisperBase, Language::Norwegian),
            ],
        ),
    ] {
        assert_eq!(
            two_language_settings().warm_model_plan_gated(4, 10_000, supported),
            expected,
            "supported={supported}"
        );
    }
}

/// Swedish file transcription on Auto, over all three probes: whether
/// Pianissimo is supported, whether it is downloaded and which Whisper models
/// are.
#[test]
fn auto_file_transcription_resolves_from_the_injected_readiness() {
    let auto = Settings::default();
    let pianissimo = FileModel::PianissimoOriginal;
    let whisper = FileModel::Whisper;
    // (supported, ready models, expected for any work, expected for Whisper-only work)
    let rows: [(bool, Vec<FileModel>, FileModel, FileModel); 8] = [
        // Supported and downloaded: Pianissimo, except for Whisper-only work.
        (true, vec![pianissimo], pianissimo, whisper(KB_MEDIUM)),
        // Supported but not downloaded: the largest downloaded Whisper model.
        (
            true,
            vec![whisper(KB_MEDIUM)],
            whisper(KB_MEDIUM),
            whisper(KB_MEDIUM),
        ),
        (
            true,
            vec![whisper(KB_SMALL), whisper(KB_LARGE)],
            whisper(KB_LARGE),
            whisper(KB_LARGE),
        ),
        // Nothing downloaded: name the recommended download.
        (true, vec![], pianissimo, whisper(KB_MEDIUM)),
        (false, vec![], whisper(KB_MEDIUM), whisper(KB_MEDIUM)),
        // Unsupported: a downloaded Pianissimo is no candidate.
        (
            false,
            vec![pianissimo],
            whisper(KB_MEDIUM),
            whisper(KB_MEDIUM),
        ),
        (
            false,
            vec![whisper(KB_LARGE)],
            whisper(KB_LARGE),
            whisper(KB_LARGE),
        ),
        (
            false,
            vec![whisper(KB_MEDIUM), whisper(KB_LARGE)],
            whisper(KB_MEDIUM),
            whisper(KB_MEDIUM),
        ),
    ];
    for (supported, ready, expected, expected_whisper_only) in rows {
        let is_ready = |model: FileModel| ready.contains(&model);
        assert_eq!(
            auto.effective_file_model_for_gated(Language::Swedish, supported, &is_ready),
            Ok(expected),
            "supported={supported} ready={ready:?}"
        );
        assert_eq!(
            auto.effective_whisper_file_model_for_gated(Language::Swedish, &is_ready),
            Ok(expected_whisper_only),
            "ready={ready:?}"
        );
    }
}
