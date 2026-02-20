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

    /// Language: en, sv, no, auto
    #[arg(short, long, default_value = "auto")]
    pub language: String,

    /// Model ID (e.g. base.en, nb-whisper-base). Default: auto-select for language
    #[arg(short, long)]
    pub model: Option<String>,

    /// Output result as JSON
    #[arg(long)]
    pub json: bool,

    /// Copy result to clipboard
    #[arg(long)]
    pub clipboard: bool,
}

pub fn run(args: TranscribeArgs) -> Result<(), DictationError> {
    let language = parse_language(&args.language)?;
    let model = resolve_model(args.model.as_deref(), language)?;

    // Check model is downloaded
    if !model::is_model_downloaded(model) {
        return Err(DictationError::TranscriptionFailed(format!(
            "Model '{}' is not downloaded. Run: flowdictate download-model {}",
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
            "language": args.language,
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
            "Unknown model '{other}'. Run 'flowdictate list-models' to see available models."
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
