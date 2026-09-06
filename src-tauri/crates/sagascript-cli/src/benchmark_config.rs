use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, Settings, WhisperModel};

use super::transcribe::{model_id_string, parse_model};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResolvedBenchmarkConfig {
    pub(crate) model: WhisperModel,
    pub(crate) beam_size: u32,
    pub(crate) temperature_fallback: bool,
}

pub(crate) fn resolve(
    language: Language,
    model_id: Option<&str>,
    beam_size: Option<u32>,
    disable_temperature_fallback: bool,
) -> Result<ResolvedBenchmarkConfig, DictationError> {
    if language == Language::Auto {
        return Err(DictationError::SettingsError(
            "Benchmark language must be explicit: en, sv, no, fi, or pl".to_string(),
        ));
    }

    let defaults = Settings::default();
    let model = match model_id {
        Some(model_id) => {
            let model = parse_model(model_id)?;
            if !WhisperModel::models_for_language(language).contains(&model) {
                return Err(DictationError::SettingsError(format!(
                    "Model '{}' is not supported for benchmark language {}",
                    model_id_string(model),
                    language.display_name(),
                )));
            }
            model
        }
        None => WhisperModel::recommended(language),
    };
    let beam_size = beam_size.unwrap_or(defaults.beam_size);
    if beam_size == 1 || beam_size > 16 {
        return Err(DictationError::SettingsError(
            "Benchmark beam size must be 0 or between 2 and 16".to_string(),
        ));
    }

    Ok(ResolvedBenchmarkConfig {
        model,
        beam_size,
        temperature_fallback: defaults.temperature_fallback && !disable_temperature_fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANGUAGES: [Language; 5] = [
        Language::English,
        Language::Swedish,
        Language::Norwegian,
        Language::Finnish,
        Language::Polish,
    ];

    #[test]
    fn defaults_use_language_recommendations_and_default_decoder_settings() {
        let cases = [
            (Language::English, WhisperModel::BaseEn),
            (Language::Swedish, WhisperModel::KbWhisperBase),
            (Language::Norwegian, WhisperModel::NbWhisperBase),
            (Language::Finnish, WhisperModel::Base),
            (Language::Polish, WhisperModel::Base),
        ];
        for (language, expected_model) in cases {
            let config = resolve(language, None, None, false).unwrap();
            assert_eq!(config.model, expected_model);
            assert_eq!(config.beam_size, 0);
            assert!(config.temperature_fallback);
        }
    }

    #[test]
    fn every_model_is_accepted_for_its_language() {
        for language in LANGUAGES {
            for &model in WhisperModel::models_for_language(language) {
                let config =
                    resolve(language, Some(model_id_string(model)), Some(2), false).unwrap();
                assert_eq!(config.model, model);
            }
        }
    }

    #[test]
    fn finnish_tiny_is_optional_while_base_remains_default() {
        let config = resolve(
            Language::Finnish,
            Some("fi-whisper-tiny"),
            Some(2),
            false,
        )
        .unwrap();
        assert_eq!(config.model, WhisperModel::FinnishWhisperTiny);
        assert_eq!(
            resolve(Language::Finnish, None, None, false).unwrap().model,
            WhisperModel::Base
        );
        assert!(resolve(Language::English, Some("fi-whisper-tiny"), None, false).is_err());
    }

    #[test]
    fn polish_specialist_small_is_optional_while_base_remains_default() {
        let config = resolve(
            Language::Polish,
            Some("pl-whisper-small"),
            Some(2),
            false,
        )
        .unwrap();
        assert_eq!(config.model, WhisperModel::PolishWhisperSmall);
        assert_eq!(
            resolve(Language::Polish, None, None, false).unwrap().model,
            WhisperModel::Base
        );
        assert!(resolve(Language::English, Some("pl-whisper-small"), None, false).is_err());
    }

    #[test]
    fn language_specific_models_are_rejected_for_other_explicit_languages() {
        for language in LANGUAGES {
            for other_language in LANGUAGES {
                if language == other_language {
                    continue;
                }
                for &model in WhisperModel::models_for_language(other_language) {
                    // Generic multilingual models are intentionally shared
                    // across explicit languages; only specialized models
                    // must be rejected outside their target language.
                    if !model.is_language_optimized() {
                        continue;
                    }
                    let result = resolve(language, Some(model_id_string(model)), None, false);
                    assert!(result.is_err(), "{model:?} accepted for {language:?}");
                }
            }
        }
    }

    #[test]
    fn beam_boundaries_are_explicit() {
        for beam_size in [0, 2, 16] {
            assert!(resolve(Language::English, None, Some(beam_size), false).is_ok());
        }
        for beam_size in [1, 17, u32::MAX] {
            let error = resolve(Language::English, None, Some(beam_size), false).unwrap_err();
            assert!(error.to_string().contains("beam size"));
        }
    }

    #[test]
    fn unknown_model_auto_language_and_auto_model_are_rejected() {
        for model_id in ["not-a-model", "auto"] {
            let error = resolve(Language::English, Some(model_id), None, false).unwrap_err();
            assert!(error.to_string().contains(model_id));
        }
        let error = resolve(Language::Auto, None, None, false).unwrap_err();
        assert!(error.to_string().contains("explicit"));
    }

    #[test]
    fn temperature_fallback_disable_flag_only_turns_default_off() {
        assert!(
            resolve(Language::English, None, None, false)
                .unwrap()
                .temperature_fallback
        );
        assert!(
            !resolve(Language::English, None, None, true)
                .unwrap()
                .temperature_fallback
        );
    }
}
