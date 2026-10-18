use std::cell::Cell;

use super::*;
use crate::test_support::model_env::FixedModelEnv;

fn ids(models: &[ModelInfo]) -> Vec<&'static str> {
    models.iter().map(|info| info.id).collect()
}

#[test]
fn the_catalog_is_the_models_of_the_language_in_list_models_order() {
    let env = FixedModelEnv::new();
    for language in Language::ALL {
        let expected: Vec<&str> = WhisperModel::models_for_language(language)
            .iter()
            .map(|model| model.id())
            .collect();
        assert_eq!(ids(&catalog(language, &env)), expected, "{language:?}");
    }
}

#[test]
fn pianissimo_closes_the_swedish_catalog_only_where_it_runs() {
    let without = ids(&catalog(Language::Swedish, &FixedModelEnv::new()));
    assert_eq!(without.last(), Some(&"kb-whisper-large"));
    assert!(!without.contains(&"pianissimo-sv"));

    let with = ids(&catalog(
        Language::Swedish,
        &FixedModelEnv::with_pianissimo(),
    ));
    assert_eq!(
        with,
        [
            "kb-whisper-tiny",
            "kb-whisper-base",
            "kb-whisper-small",
            "kb-whisper-medium",
            "kb-whisper-large",
            "pianissimo-sv"
        ]
    );
    // No other language lists it, whatever the gate.
    for language in [
        Language::English,
        Language::Norwegian,
        Language::Finnish,
        Language::Auto,
    ] {
        assert!(
            !ids(&catalog(language, &FixedModelEnv::with_pianissimo())).contains(&"pianissimo-sv")
        );
    }
}

/// The two surfaces showed different sizes for Pianissimo: `list-models` the
/// size on disk, the app's picker the download. Both are facts of the entry now,
/// and each surface keeps the one it showed.
#[test]
fn pianissimo_carries_both_its_download_and_its_installed_size() {
    let info = model_info(
        FileModel::PianissimoOriginal,
        &FixedModelEnv::with_pianissimo(),
    );
    assert_eq!(info.download_size_mb, 1_000);
    assert_eq!(info.installed_size_mb, 1_400);
}

#[test]
fn a_whisper_model_downloads_what_it_occupies() {
    let env = FixedModelEnv::new();
    for model in WhisperModel::ALL {
        let info = model_info(FileModel::Whisper(model), &env);
        assert_eq!(info.download_size_mb, model.size_mb(), "{model:?}");
        assert_eq!(info.installed_size_mb, model.size_mb(), "{model:?}");
    }
}

#[test]
fn an_entry_carries_the_names_of_its_model() {
    let env = FixedModelEnv::with_pianissimo();
    for model in WhisperModel::ALL {
        let info = model_info(FileModel::Whisper(model), &env);
        assert_eq!(info.model, FileModel::Whisper(model));
        assert_eq!(info.id, model.id());
        assert_eq!(info.display_name, model.display_name());
        assert_eq!(info.description, model.description());
        assert_eq!(info.hidden_in_app, model.is_hidden_in_app());
    }
    let pianissimo = model_info(FileModel::PianissimoOriginal, &env);
    assert_eq!(pianissimo.id, FileModel::PIANISSIMO_ID);
    assert_eq!(pianissimo.display_name, "Pianissimo");
    assert!(pianissimo
        .description
        .starts_with("Swedish file transcription"));
    assert!(!pianissimo.hidden_in_app);
}

#[test]
fn installed_follows_the_machine() {
    let env = FixedModelEnv {
        downloaded: vec![WhisperModel::KbWhisperLarge, WhisperModel::BaseEn],
        pianissimo_downloaded: true,
        ..FixedModelEnv::with_pianissimo()
    };
    let swedish = catalog(Language::Swedish, &env);
    let installed: Vec<&str> = swedish
        .iter()
        .filter(|info| info.installed)
        .map(|info| info.id)
        .collect();
    assert_eq!(installed, ["kb-whisper-large", "pianissimo-sv"]);

    let english = catalog(Language::English, &env);
    let installed: Vec<&str> = english
        .iter()
        .filter(|info| info.installed)
        .map(|info| info.id)
        .collect();
    assert_eq!(installed, ["base.en"]);
}

/// The answers that touch the disk are asked for the models that are listed,
/// as `list-models` and the picker always did: an English list never checks
/// Pianissimo, which verifies and repairs files while it looks.
#[test]
fn the_disk_is_asked_only_about_listed_models() {
    struct Counting {
        pianissimo_checks: Cell<u32>,
        whisper_checks: Cell<u32>,
    }
    impl ModelEnv for Counting {
        fn runs_pianissimo(&self) -> bool {
            true
        }
        fn pianissimo_downloaded(&self) -> bool {
            self.pianissimo_checks.set(self.pianissimo_checks.get() + 1);
            false
        }
        fn pianissimo_download_bytes(&self) -> u64 {
            0
        }
        fn pianissimo_installed_bytes(&self) -> u64 {
            0
        }
        fn whisper_downloaded(&self, _: WhisperModel) -> bool {
            self.whisper_checks.set(self.whisper_checks.get() + 1);
            false
        }
    }
    let env = Counting {
        pianissimo_checks: Cell::new(0),
        whisper_checks: Cell::new(0),
    };

    catalog(Language::English, &env);
    assert_eq!(
        (env.pianissimo_checks.get(), env.whisper_checks.get()),
        (0, 4)
    );

    catalog(Language::Swedish, &env);
    assert_eq!(
        (env.pianissimo_checks.get(), env.whisper_checks.get()),
        (1, 9)
    );

    app_picker(Language::Swedish, None, &env);
    assert_eq!(
        (env.pianissimo_checks.get(), env.whisper_checks.get()),
        (2, 10),
        "Pianissimo and KB-Whisper Large"
    );
}

#[test]
fn the_app_picker_is_the_app_lineup_in_its_order() {
    for supported in [false, true] {
        let env = if supported {
            FixedModelEnv::with_pianissimo()
        } else {
            FixedModelEnv::new()
        };
        for language in Language::ALL {
            for include in [None, Some(FileModel::Whisper(WhisperModel::KbWhisperBase))] {
                let expected: Vec<FileModel> = FileModel::app_lineup(language, supported, include);
                let picked: Vec<FileModel> = app_picker(language, include, &env)
                    .iter()
                    .map(|info| info.model)
                    .collect();
                assert_eq!(picked, expected, "{language:?} {supported} {include:?}");
            }
        }
    }
}

#[test]
fn the_app_picker_keeps_a_selected_model_visible() {
    let env = FixedModelEnv::with_pianissimo();
    let base = FileModel::Whisper(WhisperModel::KbWhisperBase);
    assert_eq!(
        ids(&app_picker(Language::Swedish, Some(base), &env)),
        ["pianissimo-sv", "kb-whisper-large", "kb-whisper-base"]
    );
    // Not for a language it cannot serve.
    assert_eq!(
        ids(&app_picker(Language::English, Some(base), &env)),
        ["tiny.en", "base.en", "small.en", "medium.en"]
    );
    // A multilingual model the English catalog does not list is still offered
    // to an English profile that selected it.
    let multilingual = FileModel::Whisper(WhisperModel::Base);
    assert_eq!(
        ids(&app_picker(Language::English, Some(multilingual), &env)).last(),
        Some(&"base")
    );
}

/// The app hides the models its lineup does not name; `list-models` lists them
/// by id. `hidden_in_app` is the note the CLI prints, and it does not follow the
/// platform: with Pianissimo, the picker also leaves out KB-Whisper Medium, which
/// is not flagged. The two lists differ here by design (#373 part 2, an open
/// owner decision).
#[test]
fn the_catalog_lists_what_the_picker_hides() {
    let env = FixedModelEnv::with_pianissimo();
    let catalog = catalog(Language::Swedish, &env);
    let hidden: Vec<&str> = catalog
        .iter()
        .filter(|info| info.hidden_in_app)
        .map(|info| info.id)
        .collect();
    assert_eq!(
        hidden,
        ["kb-whisper-tiny", "kb-whisper-base", "kb-whisper-small"]
    );

    let picker = ids(&app_picker(Language::Swedish, None, &env));
    assert!(!picker.contains(&"kb-whisper-medium"));
    let medium = catalog
        .iter()
        .find(|info| info.id == "kb-whisper-medium")
        .unwrap();
    assert!(!medium.hidden_in_app);
}

#[test]
fn recommended_follows_the_pianissimo_gate() {
    let without = FixedModelEnv::new();
    let with = FixedModelEnv::with_pianissimo();
    let recommended = |language: Language, env: &FixedModelEnv| -> Vec<&'static str> {
        catalog(language, env)
            .iter()
            .filter(|info| info.is_recommended_for(language))
            .map(|info| info.id)
            .collect()
    };
    assert_eq!(
        recommended(Language::Swedish, &without),
        ["kb-whisper-medium"]
    );
    assert_eq!(recommended(Language::Swedish, &with), ["pianissimo-sv"]);
    assert_eq!(recommended(Language::English, &with), ["base.en"]);
    assert_eq!(
        recommended(Language::Norwegian, &without),
        ["nb-whisper-base"]
    );
    assert_eq!(recommended(Language::Finnish, &with), ["base"]);
    assert_eq!(recommended(Language::Auto, &without), ["base"]);
}

#[test]
fn every_language_of_an_entry_lists_it() {
    let env = FixedModelEnv::with_pianissimo();
    for language in Language::ALL {
        for info in catalog(language, &env) {
            assert!(
                info.languages.contains(&language),
                "{} {language:?}",
                info.id
            );
            assert_eq!(model_info(info.model, &env), info);
        }
    }
    assert_eq!(
        model_info(FileModel::Whisper(WhisperModel::Base), &env).languages,
        [Language::Finnish, Language::Auto]
    );
    assert_eq!(
        model_info(FileModel::PianissimoOriginal, &env).languages,
        [Language::Swedish]
    );
}
