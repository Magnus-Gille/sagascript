use super::{context_parameters, dtw_preset, ContextProfile, WhisperModel};
use whisper_rs::DtwMode;

/// A token-alignment request used to fail at load time in a build without
/// the `diarization` feature (#371). DTW is part of whisper.cpp, which is
/// always linked, so every build must be able to ask for it. Model weights
/// are not needed: the profile decides these parameters before any weights
/// are opened.
#[test]
fn token_alignment_asks_for_dtw_in_every_build() {
    for model in WhisperModel::ALL {
        let params = context_parameters(model, ContextProfile::TokenAlignment);
        // whisper.cpp disables DTW on a flash-attention context.
        assert!(!params.flash_attn, "{model:?}");
        match params.dtw_parameters.mode {
            DtwMode::ModelPreset { model_preset } => assert_eq!(
                format!("{model_preset:?}"),
                format!("{:?}", dtw_preset(model)),
                "{model:?}"
            ),
            other => panic!("{model:?}: expected a DTW model preset, got {other:?}"),
        }
    }
}

#[test]
fn flash_attention_leaves_dtw_off() {
    for model in WhisperModel::ALL {
        let params = context_parameters(model, ContextProfile::FlashAttention);
        assert!(params.flash_attn, "{model:?}");
        assert!(
            matches!(params.dtw_parameters.mode, DtwMode::None),
            "{model:?}: {:?}",
            params.dtw_parameters.mode
        );
    }
}
