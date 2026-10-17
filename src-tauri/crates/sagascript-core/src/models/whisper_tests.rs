use super::*;

#[test]
fn whisper_model_default_is_base() {
    assert_eq!(WhisperModel::default(), WhisperModel::Base);
}

#[test]
fn english_only_models() {
    assert!(WhisperModel::TinyEn.is_english_only());
    assert!(WhisperModel::BaseEn.is_english_only());
    assert!(WhisperModel::SmallEn.is_english_only());
    assert!(WhisperModel::MediumEn.is_english_only());
    assert!(!WhisperModel::Tiny.is_english_only());
    assert!(!WhisperModel::FinnishWhisperTiny.is_english_only());
    assert!(!WhisperModel::Base.is_english_only());
    assert!(!WhisperModel::Small.is_english_only());
    assert!(!WhisperModel::Medium.is_english_only());
    assert!(!WhisperModel::LargeV3Turbo.is_english_only());
    assert!(!WhisperModel::KbWhisperTiny.is_english_only());
    assert!(!WhisperModel::NbWhisperBase.is_english_only());
}

#[test]
fn language_optimized_models_are_not_neutral_detectors() {
    assert!(WhisperModel::KbWhisperSmall.is_language_optimized());
    assert!(WhisperModel::NbWhisperBase.is_language_optimized());
    assert!(WhisperModel::FinnishWhisperTiny.is_language_optimized());
    assert!(!WhisperModel::Base.is_language_optimized());
    assert!(!WhisperModel::MediumEn.is_language_optimized());
}

#[cfg(target_os = "macos")]
#[test]
fn coreml_encoder_derivation() {
    // OpenAI models: URL + on-disk dir name must match whisper.cpp's
    // `.bin` → `-encoder.mlmodelc` derivation (verified against the
    // ggerganov/whisper.cpp HF repo).
    assert_eq!(
        WhisperModel::Base.coreml_encoder_url().as_deref(),
        Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base-encoder.mlmodelc.zip")
    );
    assert_eq!(
        WhisperModel::Base.coreml_encoder_dirname().as_deref(),
        Some("ggml-base-encoder.mlmodelc")
    );
    // `.en` is part of the name, not a quant suffix — must be preserved.
    assert_eq!(
        WhisperModel::BaseEn.coreml_encoder_dirname().as_deref(),
        Some("ggml-base.en-encoder.mlmodelc")
    );
    assert_eq!(
        WhisperModel::LargeV3Turbo
            .coreml_encoder_dirname()
            .as_deref(),
        Some("ggml-large-v3-turbo-encoder.mlmodelc")
    );
    // Quantised turbo strips the `-q8_0` suffix and reuses the SAME FP16
    // CoreML encoder as the f16 turbo (matches whisper.cpp's derivation).
    assert_eq!(
        WhisperModel::LargeV3TurboQ8
            .coreml_encoder_dirname()
            .as_deref(),
        Some("ggml-large-v3-turbo-encoder.mlmodelc")
    );
    assert_eq!(
        WhisperModel::LargeV3TurboQ8.coreml_encoder_url().as_deref(),
        Some("https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-encoder.mlmodelc.zip")
    );
    // KB/NB fine-tunes live in other repos and have no CoreML encoder.
    assert_eq!(WhisperModel::KbWhisperBase.coreml_encoder_url(), None);
    assert_eq!(WhisperModel::NbWhisperSmall.coreml_encoder_dirname(), None);
    assert_eq!(WhisperModel::FinnishWhisperTiny.coreml_encoder_url(), None);
}

#[test]
fn swedish_optimized_models() {
    assert!(WhisperModel::KbWhisperTiny.is_swedish_optimized());
    assert!(WhisperModel::KbWhisperBase.is_swedish_optimized());
    assert!(WhisperModel::KbWhisperSmall.is_swedish_optimized());
    assert!(WhisperModel::KbWhisperMedium.is_swedish_optimized());
    assert!(WhisperModel::KbWhisperLarge.is_swedish_optimized());
    assert!(!WhisperModel::TinyEn.is_swedish_optimized());
    assert!(!WhisperModel::NbWhisperTiny.is_swedish_optimized());
    assert!(!WhisperModel::FinnishWhisperTiny.is_swedish_optimized());
}

#[test]
fn norwegian_optimized_models() {
    assert!(WhisperModel::NbWhisperTiny.is_norwegian_optimized());
    assert!(WhisperModel::NbWhisperBase.is_norwegian_optimized());
    assert!(WhisperModel::NbWhisperSmall.is_norwegian_optimized());
    assert!(WhisperModel::NbWhisperMedium.is_norwegian_optimized());
    assert!(WhisperModel::NbWhisperLarge.is_norwegian_optimized());
    assert!(!WhisperModel::TinyEn.is_norwegian_optimized());
    assert!(!WhisperModel::KbWhisperTiny.is_norwegian_optimized());
    assert!(!WhisperModel::FinnishWhisperTiny.is_norwegian_optimized());
}

#[test]
fn all_models_have_ggml_filenames() {
    let models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::FinnishWhisperTiny,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::KbWhisperMedium,
        WhisperModel::KbWhisperLarge,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::NbWhisperMedium,
        WhisperModel::NbWhisperLarge,
        WhisperModel::SmallEn,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for m in models {
        let filename = m.ggml_filename();
        assert!(filename.ends_with(".bin"), "{:?} filename: {}", m, filename);
        assert!(!filename.is_empty());
    }
}

#[test]
fn all_models_have_download_urls() {
    let models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::FinnishWhisperTiny,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::KbWhisperMedium,
        WhisperModel::KbWhisperLarge,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::NbWhisperMedium,
        WhisperModel::NbWhisperLarge,
        WhisperModel::SmallEn,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for m in models {
        let url = m.download_url();
        assert!(
            url.starts_with("https://huggingface.co/"),
            "{:?}: {}",
            m,
            url
        );
        assert!(url.contains(".bin"), "{:?}: {}", m, url);
        assert!(
            !url.contains("/resolve/main/"),
            "mutable URL for {m:?}: {url}"
        );
        let integrity = m.download_integrity();
        assert_eq!(integrity.sha256.len(), 64, "invalid SHA-256 for {m:?}");
        assert!(integrity.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            integrity.size >= 20 * 1024 * 1024,
            "implausibly small Whisper artifact for {m:?}: {} bytes",
            integrity.size
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn all_coreml_archives_are_immutable_and_have_exact_metadata() {
    let models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::SmallEn,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for model in models {
        let url = model.coreml_encoder_url().unwrap();
        assert!(!url.contains("/resolve/main/"), "mutable URL: {url}");
        let integrity = model.coreml_encoder_integrity().unwrap();
        assert_eq!(integrity.sha256.len(), 64);
        assert!(integrity.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(
            integrity.size >= 10 * 1024 * 1024,
            "implausibly small CoreML archive for {model:?}"
        );
    }
}

#[test]
fn all_models_have_nonzero_size() {
    let models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::FinnishWhisperTiny,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::KbWhisperMedium,
        WhisperModel::KbWhisperLarge,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::NbWhisperMedium,
        WhisperModel::NbWhisperLarge,
        WhisperModel::SmallEn,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for m in models {
        assert!(m.size_mb() > 0, "{:?} has 0 size", m);
    }
}

#[test]
fn recommended_model_per_language() {
    assert_eq!(
        WhisperModel::recommended(Language::English),
        WhisperModel::BaseEn
    );
    assert_eq!(
        WhisperModel::recommended(Language::Swedish),
        WhisperModel::KbWhisperMedium
    );
    assert_eq!(
        WhisperModel::recommended(Language::Norwegian),
        WhisperModel::NbWhisperBase
    );
    assert_eq!(
        WhisperModel::recommended(Language::Finnish),
        WhisperModel::Base
    );
    assert_eq!(
        WhisperModel::recommended(Language::Auto),
        WhisperModel::Base
    );
}

#[test]
fn models_for_language_returns_correct_sets() {
    let en = WhisperModel::models_for_language(Language::English);
    assert_eq!(en.len(), 4);
    assert!(en.contains(&WhisperModel::TinyEn));
    assert!(en.contains(&WhisperModel::BaseEn));
    assert!(en.contains(&WhisperModel::SmallEn));
    assert!(en.contains(&WhisperModel::MediumEn));

    let sv = WhisperModel::models_for_language(Language::Swedish);
    assert_eq!(sv.len(), 5);
    assert!(sv.contains(&WhisperModel::KbWhisperTiny));
    assert!(sv.contains(&WhisperModel::KbWhisperBase));
    assert!(sv.contains(&WhisperModel::KbWhisperSmall));
    assert!(sv.contains(&WhisperModel::KbWhisperMedium));
    assert!(sv.contains(&WhisperModel::KbWhisperLarge));

    let no = WhisperModel::models_for_language(Language::Norwegian);
    assert_eq!(no.len(), 5);
    assert!(no.contains(&WhisperModel::NbWhisperTiny));
    assert!(no.contains(&WhisperModel::NbWhisperBase));
    assert!(no.contains(&WhisperModel::NbWhisperSmall));
    assert!(no.contains(&WhisperModel::NbWhisperMedium));
    assert!(no.contains(&WhisperModel::NbWhisperLarge));

    let fi = WhisperModel::models_for_language(Language::Finnish);
    assert_eq!(fi.len(), 7);
    assert!(fi.contains(&WhisperModel::Base));
    assert!(fi.contains(&WhisperModel::FinnishWhisperTiny));
    assert!(fi.contains(&WhisperModel::LargeV3TurboQ8));

    let auto = WhisperModel::models_for_language(Language::Auto);
    assert_eq!(auto.len(), 6);
    assert!(auto.contains(&WhisperModel::Tiny));
    assert!(auto.contains(&WhisperModel::Base));
    assert!(auto.contains(&WhisperModel::Small));
    assert!(auto.contains(&WhisperModel::Medium));
    assert!(auto.contains(&WhisperModel::LargeV3Turbo));
    assert!(auto.contains(&WhisperModel::LargeV3TurboQ8));
}

#[test]
fn whisper_model_serde_roundtrip() {
    let model = WhisperModel::KbWhisperSmall;
    let json = serde_json::to_string(&model).unwrap();
    assert_eq!(json, "\"kb-whisper-small\"");
    let back: WhisperModel = serde_json::from_str(&json).unwrap();
    assert_eq!(back, model);
}

#[test]
fn whisper_model_serde_all_variants() {
    let pairs = [
        (WhisperModel::TinyEn, "\"tiny.en\""),
        (WhisperModel::Tiny, "\"tiny\""),
        (WhisperModel::BaseEn, "\"base.en\""),
        (WhisperModel::Base, "\"base\""),
        (WhisperModel::FinnishWhisperTiny, "\"fi-whisper-tiny\""),
        (WhisperModel::KbWhisperTiny, "\"kb-whisper-tiny\""),
        (WhisperModel::KbWhisperBase, "\"kb-whisper-base\""),
        (WhisperModel::KbWhisperSmall, "\"kb-whisper-small\""),
        (WhisperModel::KbWhisperMedium, "\"kb-whisper-medium\""),
        (WhisperModel::KbWhisperLarge, "\"kb-whisper-large\""),
        (WhisperModel::NbWhisperTiny, "\"nb-whisper-tiny\""),
        (WhisperModel::NbWhisperBase, "\"nb-whisper-base\""),
        (WhisperModel::NbWhisperSmall, "\"nb-whisper-small\""),
        (WhisperModel::NbWhisperMedium, "\"nb-whisper-medium\""),
        (WhisperModel::NbWhisperLarge, "\"nb-whisper-large\""),
        (WhisperModel::SmallEn, "\"small.en\""),
        (WhisperModel::Small, "\"small\""),
        (WhisperModel::MediumEn, "\"medium.en\""),
        (WhisperModel::Medium, "\"medium\""),
        (WhisperModel::LargeV3Turbo, "\"large-v3-turbo\""),
        (WhisperModel::LargeV3TurboQ8, "\"large-v3-turbo-q8_0\""),
    ];
    for (model, expected) in pairs {
        let json = serde_json::to_string(&model).unwrap();
        assert_eq!(json, expected, "serialize {:?}", model);
        let back: WhisperModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, model, "deserialize {:?}", model);
    }
}

#[test]
fn no_speech_threshold_small_en_fully_disabled() {
    assert_eq!(WhisperModel::SmallEn.no_speech_threshold(), 0.0);
}

#[test]
fn no_speech_threshold_other_models_at_0_3() {
    let models = [
        WhisperModel::TinyEn,
        WhisperModel::Tiny,
        WhisperModel::BaseEn,
        WhisperModel::Base,
        WhisperModel::FinnishWhisperTiny,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::KbWhisperMedium,
        WhisperModel::KbWhisperLarge,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::NbWhisperMedium,
        WhisperModel::NbWhisperLarge,
        WhisperModel::Small,
        WhisperModel::MediumEn,
        WhisperModel::Medium,
        WhisperModel::LargeV3Turbo,
        WhisperModel::LargeV3TurboQ8,
    ];
    for m in models {
        assert_eq!(
            m.no_speech_threshold(),
            0.3,
            "{:?} should have threshold 0.3",
            m
        );
    }
}

#[test]
fn recommended_model_is_in_models_for_language() {
    let languages = [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Finnish,
        Language::Auto,
    ];
    for lang in languages {
        let recommended = WhisperModel::recommended(lang);
        let models = WhisperModel::models_for_language(lang);
        assert!(
            models.contains(&recommended),
            "recommended model {:?} for {:?} is not in models_for_language: {:?}",
            recommended,
            lang,
            models
        );
    }
}

#[test]
fn finnish_uses_generic_default_and_optional_specialized_tiny() {
    assert_eq!(
        WhisperModel::recommended(Language::Finnish),
        WhisperModel::Base
    );
    assert_eq!(
        serde_json::to_string(&WhisperModel::FinnishWhisperTiny).unwrap(),
        "\"fi-whisper-tiny\""
    );
    assert_eq!(
        WhisperModel::FinnishWhisperTiny.ggml_filename(),
        "ggml-model-fi-tiny.bin"
    );
    assert_eq!(
        WhisperModel::FinnishWhisperTiny.download_url(),
        "https://huggingface.co/Finnish-NLP/Finnish-finetuned-whisper-models-ggml-format/resolve/c58924b6deb4438756b3d38ecd67d65bdf20298d/ggml-model-fi-tiny.bin"
    );
    let integrity = WhisperModel::FinnishWhisperTiny.download_integrity();
    assert_eq!(integrity.size, 77_691_730);
    assert_eq!(
        integrity.sha256,
        "41cf309b7f50523cfca724ae90924fcd0e4794205de57a66abc3cce627103ce8"
    );
    for &model in WhisperModel::models_for_language(Language::Finnish) {
        assert!(model.is_compatible_with(Language::Finnish), "{model:?}");
    }
    assert!(WhisperModel::FinnishWhisperTiny.is_compatible_with(Language::Finnish));
    for language in [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Auto,
    ] {
        assert!(!WhisperModel::FinnishWhisperTiny.is_compatible_with(language));
    }
    for model in [
        WhisperModel::TinyEn,
        WhisperModel::BaseEn,
        WhisperModel::SmallEn,
        WhisperModel::MediumEn,
        WhisperModel::KbWhisperTiny,
        WhisperModel::KbWhisperBase,
        WhisperModel::KbWhisperSmall,
        WhisperModel::KbWhisperMedium,
        WhisperModel::KbWhisperLarge,
        WhisperModel::NbWhisperTiny,
        WhisperModel::NbWhisperBase,
        WhisperModel::NbWhisperSmall,
        WhisperModel::NbWhisperMedium,
        WhisperModel::NbWhisperLarge,
    ] {
        assert!(!model.is_compatible_with(Language::Finnish), "{model:?}");
    }
}

/// Exhaustive on purpose: adding a variant breaks this match until it is
/// listed, which then fails `all_lists_every_model_exactly_once` until
/// `WhisperModel::ALL` has it too.
fn ordinal(model: WhisperModel) -> usize {
    match model {
        WhisperModel::TinyEn => 0,
        WhisperModel::Tiny => 1,
        WhisperModel::BaseEn => 2,
        WhisperModel::Base => 3,
        WhisperModel::FinnishWhisperTiny => 4,
        WhisperModel::KbWhisperTiny => 5,
        WhisperModel::KbWhisperBase => 6,
        WhisperModel::KbWhisperSmall => 7,
        WhisperModel::KbWhisperMedium => 8,
        WhisperModel::KbWhisperLarge => 9,
        WhisperModel::NbWhisperTiny => 10,
        WhisperModel::NbWhisperBase => 11,
        WhisperModel::NbWhisperSmall => 12,
        WhisperModel::NbWhisperMedium => 13,
        WhisperModel::NbWhisperLarge => 14,
        WhisperModel::SmallEn => 15,
        WhisperModel::Small => 16,
        WhisperModel::MediumEn => 17,
        WhisperModel::Medium => 18,
        WhisperModel::LargeV3Turbo => 19,
        WhisperModel::LargeV3TurboQ8 => 20,
    }
}

#[test]
fn all_lists_every_model_exactly_once() {
    assert_eq!(WhisperModel::ALL.len(), 21);
    for (index, model) in WhisperModel::ALL.into_iter().enumerate() {
        assert_eq!(ordinal(model), index, "{model:?} is out of order in ALL");
    }
}

#[test]
fn ids_equal_the_serde_names_of_every_model() {
    for model in WhisperModel::ALL {
        assert_eq!(
            serde_json::to_value(model).unwrap(),
            serde_json::Value::String(model.id().to_owned()),
            "{model:?}"
        );
    }
}

#[test]
fn from_id_round_trips_every_model_and_agrees_with_serde_on_unknown_ids() {
    let mut seen = std::collections::HashSet::new();
    for model in WhisperModel::ALL {
        assert!(seen.insert(model.id()), "duplicate id {}", model.id());
        assert_eq!(WhisperModel::from_id(model.id()), Some(model));
        assert_eq!(model.id().parse::<WhisperModel>(), Ok(model));
    }
    for unknown in [
        "",
        "Base",
        " base",
        "base ",
        "large-v3",
        "pianissimo-sv",
        "auto",
    ] {
        assert_eq!(WhisperModel::from_id(unknown), None, "{unknown:?}");
        assert!(
            serde_json::from_value::<WhisperModel>(serde_json::Value::String(unknown.to_owned()))
                .is_err(),
            "serde accepts {unknown:?}"
        );
    }
    let error = "large-v3".parse::<WhisperModel>().unwrap_err();
    assert_eq!(error, UnknownModelId("large-v3".to_owned()));
    assert_eq!(error.to_string(), "Unknown model 'large-v3'");
}
