use std::path::PathBuf;

use clap::Args;

use crate::audio::decoder::decode_audio_file;
use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};
use crate::transcription::model;
use crate::transcription::WhisperBackend;

#[derive(Args)]
pub struct TranscribeArgs {
    /// Path to the audio/video file to transcribe
    pub file: PathBuf,

    /// Language for transcription [possible values: en, sv, no, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Whisper model ID to use [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Output result as JSON (includes text, language, model, duration)
    #[arg(long)]
    pub json: bool,

    /// Copy transcription result to clipboard
    #[arg(long)]
    pub clipboard: bool,
}

pub fn run(args: TranscribeArgs) -> Result<(), DictationError> {
    let stored = crate::settings::store::load();
    let language = match &args.language {
        Some(l) => parse_language(l)?,
        None => stored.language,
    };
    let model = match &args.model {
        Some(m) => parse_model(m)?,
        None => {
            if stored.auto_select_model {
                WhisperModel::recommended(language)
            } else {
                stored.whisper_model
            }
        }
    };

    // Check model is downloaded
    if !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: sagascript download-model {}",
            model.display_name(),
            model_id_string(model)
        )));
    }

    // Decode audio file
    eprintln!("Decoding {}...", args.file.display());
    let audio = decode_audio_file(&args.file)?;
    let duration = audio.len() as f64 / 16_000.0;
    eprintln!("Audio: {:.1}s, {} samples", duration, audio.len());

    // Load model
    eprintln!("Loading model: {}...", model.display_name());
    let backend = WhisperBackend::new();
    backend.load_model(model)?;

    // Transcribe
    eprintln!("Transcribing...");
    let text = backend.transcribe_sync(&audio, language)?;

    // Output
    if args.json {
        let json = serde_json::json!({
            "text": text,
            "language": language,
            "model": model_id_string(model),
            "file": args.file.display().to_string(),
            "duration_seconds": duration,
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!("{text}");
    }

    // Clipboard
    if args.clipboard {
        copy_to_clipboard(&text)?;
        eprintln!("Copied to clipboard.");
    }

    Ok(())
}

pub fn parse_language(s: &str) -> Result<Language, DictationError> {
    match s {
        "en" | "english" => Ok(Language::English),
        "sv" | "swedish" => Ok(Language::Swedish),
        "no" | "norwegian" => Ok(Language::Norwegian),
        "auto" => Ok(Language::Auto),
        other => Err(DictationError::SettingsError(format!(
            "Unknown language '{other}'. Valid: en, sv, no, auto"
        ))),
    }
}

pub fn resolve_model(
    model_str: Option<&str>,
    language: Language,
) -> Result<WhisperModel, DictationError> {
    match model_str {
        None => Ok(WhisperModel::recommended(language)),
        Some(s) => parse_model(s),
    }
}

pub fn parse_model(s: &str) -> Result<WhisperModel, DictationError> {
    match s {
        "tiny.en" => Ok(WhisperModel::TinyEn),
        "tiny" => Ok(WhisperModel::Tiny),
        "base.en" => Ok(WhisperModel::BaseEn),
        "base" => Ok(WhisperModel::Base),
        "kb-whisper-tiny" => Ok(WhisperModel::KbWhisperTiny),
        "kb-whisper-base" => Ok(WhisperModel::KbWhisperBase),
        "kb-whisper-small" => Ok(WhisperModel::KbWhisperSmall),
        "nb-whisper-tiny" => Ok(WhisperModel::NbWhisperTiny),
        "nb-whisper-base" => Ok(WhisperModel::NbWhisperBase),
        "nb-whisper-small" => Ok(WhisperModel::NbWhisperSmall),
        other => Err(DictationError::SettingsError(format!(
            "Unknown model '{other}'. Run 'sagascript list-models' to see available models."
        ))),
    }
}

pub fn model_id_string(model: WhisperModel) -> &'static str {
    match model {
        WhisperModel::TinyEn => "tiny.en",
        WhisperModel::Tiny => "tiny",
        WhisperModel::BaseEn => "base.en",
        WhisperModel::Base => "base",
        WhisperModel::KbWhisperTiny => "kb-whisper-tiny",
        WhisperModel::KbWhisperBase => "kb-whisper-base",
        WhisperModel::KbWhisperSmall => "kb-whisper-small",
        WhisperModel::NbWhisperTiny => "nb-whisper-tiny",
        WhisperModel::NbWhisperBase => "nb-whisper-base",
        WhisperModel::NbWhisperSmall => "nb-whisper-small",
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<(), DictationError> {
    use arboard::Clipboard;
    let mut clipboard =
        Clipboard::new().map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;
    clipboard
        .set_text(text)
        .map_err(|e| DictationError::PasteError(format!("Clipboard error: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_language --

    #[test]
    fn parse_language_valid_codes() {
        assert_eq!(parse_language("en").unwrap(), Language::English);
        assert_eq!(parse_language("sv").unwrap(), Language::Swedish);
        assert_eq!(parse_language("no").unwrap(), Language::Norwegian);
        assert_eq!(parse_language("auto").unwrap(), Language::Auto);
    }

    #[test]
    fn parse_language_long_names() {
        assert_eq!(parse_language("english").unwrap(), Language::English);
        assert_eq!(parse_language("swedish").unwrap(), Language::Swedish);
        assert_eq!(parse_language("norwegian").unwrap(), Language::Norwegian);
    }

    #[test]
    fn parse_language_invalid() {
        assert!(parse_language("de").is_err());
        assert!(parse_language("").is_err());
        assert!(parse_language("ENGLISH").is_err()); // case-sensitive
    }

    #[test]
    fn parse_language_error_message() {
        let err = parse_language("fr").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("fr"), "error should mention input: {msg}");
        assert!(msg.contains("en"), "error should list valid options: {msg}");
    }

    // -- parse_model --

    #[test]
    fn parse_model_all_valid_ids() {
        let cases = [
            ("tiny.en", WhisperModel::TinyEn),
            ("tiny", WhisperModel::Tiny),
            ("base.en", WhisperModel::BaseEn),
            ("base", WhisperModel::Base),
            ("kb-whisper-tiny", WhisperModel::KbWhisperTiny),
            ("kb-whisper-base", WhisperModel::KbWhisperBase),
            ("kb-whisper-small", WhisperModel::KbWhisperSmall),
            ("nb-whisper-tiny", WhisperModel::NbWhisperTiny),
            ("nb-whisper-base", WhisperModel::NbWhisperBase),
            ("nb-whisper-small", WhisperModel::NbWhisperSmall),
        ];
        for (id, expected) in cases {
            assert_eq!(parse_model(id).unwrap(), expected, "parse_model({id})");
        }
    }

    #[test]
    fn parse_model_invalid() {
        assert!(parse_model("large-v3").is_err());
        assert!(parse_model("").is_err());
        assert!(parse_model("BASE").is_err()); // case-sensitive
    }

    #[test]
    fn parse_model_error_message() {
        let err = parse_model("nonexistent").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nonexistent"));
        assert!(msg.contains("list-models"));
    }

    // -- model_id_string --

    #[test]
    fn model_id_string_all_variants() {
        let models = [
            (WhisperModel::TinyEn, "tiny.en"),
            (WhisperModel::Tiny, "tiny"),
            (WhisperModel::BaseEn, "base.en"),
            (WhisperModel::Base, "base"),
            (WhisperModel::KbWhisperTiny, "kb-whisper-tiny"),
            (WhisperModel::KbWhisperBase, "kb-whisper-base"),
            (WhisperModel::KbWhisperSmall, "kb-whisper-small"),
            (WhisperModel::NbWhisperTiny, "nb-whisper-tiny"),
            (WhisperModel::NbWhisperBase, "nb-whisper-base"),
            (WhisperModel::NbWhisperSmall, "nb-whisper-small"),
        ];
        for (model, expected) in models {
            assert_eq!(model_id_string(model), expected);
        }
    }

    #[test]
    fn model_id_string_roundtrip_with_parse() {
        let all_models = [
            WhisperModel::TinyEn,
            WhisperModel::Tiny,
            WhisperModel::BaseEn,
            WhisperModel::Base,
            WhisperModel::KbWhisperTiny,
            WhisperModel::KbWhisperBase,
            WhisperModel::KbWhisperSmall,
            WhisperModel::NbWhisperTiny,
            WhisperModel::NbWhisperBase,
            WhisperModel::NbWhisperSmall,
        ];
        for model in all_models {
            let id = model_id_string(model);
            let parsed = parse_model(id).unwrap();
            assert_eq!(parsed, model, "roundtrip failed for {id}");
        }
    }

    // -- resolve_model --

    #[test]
    fn resolve_model_none_uses_recommended() {
        let result = resolve_model(None, Language::English).unwrap();
        assert_eq!(result, WhisperModel::BaseEn);

        let result = resolve_model(None, Language::Swedish).unwrap();
        assert_eq!(result, WhisperModel::KbWhisperBase);
    }

    #[test]
    fn resolve_model_explicit_overrides() {
        let result = resolve_model(Some("tiny.en"), Language::Swedish).unwrap();
        assert_eq!(result, WhisperModel::TinyEn);
    }

    #[test]
    fn resolve_model_invalid_string() {
        assert!(resolve_model(Some("invalid"), Language::Auto).is_err());
    }
}
