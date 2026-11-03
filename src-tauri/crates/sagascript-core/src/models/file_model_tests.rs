use super::*;

#[test]
fn swedish_lineup_per_platform() {
    let ids = |models: Vec<FileModel>| -> Vec<String> {
        models
            .into_iter()
            .map(|m| match m {
                FileModel::Pianissimo(model) => model.id().to_string(),
                FileModel::Whisper(w) => w.display_name().to_string(),
            })
            .collect()
    };
    assert_eq!(
        ids(FileModel::app_lineup(Language::Swedish, true, None)),
        ["pianissimo-sv", "KB-Whisper Large"]
    );
    assert_eq!(
        ids(FileModel::app_lineup(Language::Swedish, false, None)),
        ["KB-Whisper Medium", "KB-Whisper Large"]
    );
    // Other languages are unchanged and ignore the gate.
    for gate in [true, false] {
        for language in [
            Language::English,
            Language::Norwegian,
            Language::Finnish,
            Language::Auto,
        ] {
            let expected: Vec<FileModel> = WhisperModel::models_for_language(language)
                .iter()
                .copied()
                .map(FileModel::Whisper)
                .collect();
            assert_eq!(FileModel::app_lineup(language, gate, None), expected);
        }
        // Danish: its Pianissimo model first where it runs, then Whisper.
        let whisper = WhisperModel::models_for_language(Language::Danish)
            .iter()
            .copied()
            .map(FileModel::Whisper);
        let expected: Vec<FileModel> = if gate {
            std::iter::once(FileModel::Pianissimo(PianissimoModel::Danish))
                .chain(whisper)
                .collect()
        } else {
            whisper.collect()
        };
        assert_eq!(
            FileModel::app_lineup(Language::Danish, gate, None),
            expected
        );
    }
    // The retired models stay in the full catalogue.
    let all = WhisperModel::models_for_language(Language::Swedish);
    assert_eq!(all.len(), 5);
    assert_eq!(all.iter().filter(|m| m.is_hidden_in_app()).count(), 3);
}

#[test]
fn lineup_keeps_the_current_hidden_choice_selectable() {
    let base = FileModel::Whisper(WhisperModel::KbWhisperBase);
    let lineup = FileModel::app_lineup(Language::Swedish, true, Some(base));
    assert_eq!(lineup.last(), Some(&base));
    assert_eq!(lineup.len(), 3);
    // An incompatible or unsupported selection is not injected.
    assert_eq!(
        FileModel::app_lineup(Language::English, true, Some(base)).len(),
        WhisperModel::models_for_language(Language::English).len()
    );
    assert_eq!(
        FileModel::app_lineup(
            Language::Swedish,
            false,
            Some(FileModel::Pianissimo(PianissimoModel::Swedish))
        )
        .len(),
        2
    );
}

#[test]
fn file_model_ids_round_trip_and_extend_the_whisper_ids() {
    let mut choices: Vec<FileModel> = WhisperModel::ALL
        .into_iter()
        .map(FileModel::Whisper)
        .collect();
    choices.push(FileModel::Pianissimo(PianissimoModel::Swedish));
    for choice in &choices {
        assert_eq!(FileModel::from_id(choice.id()), Some(*choice));
        assert_eq!(choice.id().parse::<FileModel>(), Ok(*choice));
    }
    assert_eq!(
        FileModel::Pianissimo(PianissimoModel::Swedish).id(),
        "pianissimo-sv"
    );
    assert_eq!(PianissimoModel::Swedish.id(), "pianissimo-sv");
    assert_eq!(
        FileModel::Whisper(WhisperModel::KbWhisperMedium).id(),
        WhisperModel::KbWhisperMedium.id()
    );
    // `auto` is a preference, not a model.
    for unknown in ["", "auto", "Pianissimo-sv", "large-v3"] {
        assert_eq!(FileModel::from_id(unknown), None, "{unknown:?}");
    }
    assert_eq!(
        "auto".parse::<FileModel>(),
        Err(UnknownModelId("auto".to_owned()))
    );
}

#[test]
fn a_pianissimo_model_serves_only_its_own_language() {
    for model in PianissimoModel::ALL {
        let choice = FileModel::Pianissimo(model);
        assert_eq!(FileModel::from_id(model.id()), Some(choice));
        assert_eq!(FileModel::recommended_for(model.language(), true), choice);
        assert_ne!(FileModel::recommended_for(model.language(), false), choice);
        for language in Language::ALL {
            assert_eq!(
                choice.is_compatible_with(language),
                language == model.language()
            );
            // `include` never smuggles a model into another language's picker.
            let offered = FileModel::app_lineup(language, true, Some(choice)).contains(&choice);
            assert_eq!(offered, language == model.language(), "{language:?}");
        }
        // Nor onto a machine that cannot run it.
        assert!(!FileModel::app_lineup(model.language(), false, Some(choice)).contains(&choice));
    }
}
