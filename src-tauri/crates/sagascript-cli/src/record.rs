use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::resample::TARGET_SAMPLE_RATE;
use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::{Glossary, TranscribeOptions, WhisperBackend};

use super::transcribe::{
    copy_to_clipboard, effective_glossary, model_id_string, parse_language,
    resolve_effective_model, resolve_profile,
};

#[derive(Args)]
pub struct RecordArgs {
    /// Language for transcription [possible values: en, sv, no, fi, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// Whisper model ID or pianissimo-sv (Swedish only) [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Max recording duration in seconds (default: record until Ctrl+C)
    #[arg(short, long, value_name = "SECONDS")]
    pub duration: Option<f64>,

    /// Save audio to WAV file instead of transcribing
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<String>,

    /// Output result as JSON (includes text, language, model, duration)
    #[arg(long)]
    pub json: bool,

    /// Copy transcription result to clipboard
    #[arg(long)]
    pub clipboard: bool,

    /// Hint the Whisper decoder with domain-specific vocabulary. Not supported by pianissimo-sv.
    /// Reduces mishearings of proper nouns, foreign names, and jargon.
    /// Example: --hint "Notre Dame, Sara, Grimnir"
    #[arg(long, visible_alias = "hint", value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Read the Whisper hint/initial prompt from a file instead of the command line.
    /// Not supported by pianissimo-sv.
    /// Mutually exclusive with --hint/--prompt.
    #[arg(
        long,
        visible_alias = "hint-file",
        value_name = "PATH",
        conflicts_with = "prompt"
    )]
    pub prompt_file: Option<PathBuf>,
}

pub fn run(args: RecordArgs) -> Result<(), DictationError> {
    let stored = sagascript_core::settings::store::load();
    let profile = args
        .profile
        .as_deref()
        .map(|profile_id| resolve_profile(&stored, profile_id))
        .transpose()?;
    let language = match (&profile, &args.language) {
        (Some(profile), _) => profile.language,
        (None, Some(language)) => parse_language(language)?,
        (None, None) => stored.language,
    };
    let save_only = args.output.is_some();
    // Saving audio does not select or require a transcription model. Resolve
    // and validate the engine before capture for every transcription run.
    let selected_model = if save_only {
        None
    } else {
        let selected = resolve_record_model(
            args.model.as_deref(),
            language,
            stored.auto_select_model,
            stored.whisper_model,
            stored.uses_pianissimo_for_dictation(language),
        )?;
        if selected == RecordModel::Pianissimo {
            validate_pianissimo_record_options(args.prompt.is_some(), args.prompt_file.is_some())?;
        }
        Some(selected)
    };
    // Save-only output does not transcribe, so it intentionally does not read
    // a hint file. Transcription keeps the normal scoped glossary behavior.
    let glossary = if save_only {
        Glossary::parse("")
    } else {
        effective_glossary(
            &stored,
            args.profile.as_deref(),
            args.prompt.as_deref(),
            args.prompt_file.as_deref(),
        )?
    };
    if let Some(selected) = selected_model {
        match selected {
            RecordModel::Whisper(model) => {
                if !model::is_model_downloaded(model) {
                    return Err(DictationError::TranscriptionFailed(format!(
                        "Model '{}' is not downloaded. Run: sagascript download-model {}",
                        model.display_name(),
                        model_id_string(model)
                    )));
                }
            }
            RecordModel::Pianissimo => {
                if !pianissimo_model::is_downloaded() {
                    return Err(DictationError::TranscriptionFailed(
                        "Pianissimo Q8 is not downloaded. Run: sagascript download-model pianissimo-sv".into(),
                    ));
                }
            }
        }
    }

    // Set up Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc_handler(r);

    // Start recording
    let mut capture = AudioCaptureService::new();
    capture.start_capture()?;

    if let Some(secs) = args.duration {
        eprintln!("Recording for {secs}s... (press Ctrl+C to stop early)");
    } else {
        eprintln!("Recording... press Ctrl+C to stop");
    }

    // Wait for duration or Ctrl+C
    let start = std::time::Instant::now();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if !running.load(Ordering::Relaxed) {
            break;
        }
        if let Some(secs) = args.duration {
            if start.elapsed().as_secs_f64() >= secs {
                break;
            }
        }
    }

    let audio = capture.stop_capture()?;
    let duration = audio.len() as f64 / TARGET_SAMPLE_RATE as f64;
    eprintln!(
        "Captured {:.1}s of audio ({} samples)",
        duration,
        audio.len()
    );

    if audio.is_empty() {
        return Err(DictationError::NoAudioCaptured);
    }

    // Save WAV if requested
    if let Some(output_path) = &args.output {
        let wav_bytes = sagascript_core::audio::wav::encode_wav(&audio);
        std::fs::write(output_path, &wav_bytes)
            .map_err(|e| DictationError::FileDecodeError(format!("Failed to write WAV: {e}")))?;
        eprintln!("Saved to {output_path}");
        return Ok(());
    }

    // Transcribe
    let selected_model = selected_model.expect("transcription model resolved before capture");
    let (text, model_id) = match selected_model {
        RecordModel::Whisper(model) => {
            eprintln!("Loading model: {}...", model.display_name());
            let backend = WhisperBackend::new();
            backend.load_model(model)?;

            let decoder_prompt = glossary.decoder_prompt();
            let opts = TranscribeOptions {
                prompt: decoder_prompt,
                ..TranscribeOptions::default()
            };
            let text = if duration > 10.0 {
                let pb = ProgressBar::new(100);
                pb.set_style(
                    ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap(),
                );
                let pb_cb = pb.clone();
                let text = backend.transcribe_live_sync_with_options(
                    &audio,
                    language,
                    &opts,
                    move |pct| {
                        crate::set_transcription_progress(&pb_cb, pct);
                    },
                )?;
                pb.finish_and_clear();
                text
            } else {
                eprintln!("Transcribing...");
                backend.transcribe_live_sync_with_options(&audio, language, &opts, |_| {})?
            };
            (text, model_id_string(model))
        }
        RecordModel::Pianissimo => {
            if glossary.decoder_prompt().is_some() {
                eprintln!("Pianissimo does not use Whisper decoder hints; aliases in the selected profile still correct the transcript after inference.");
            }
            eprintln!("Loading model: Pianissimo Q8...");
            let backend = PianissimoBackend::start()?;
            let result = transcribe_pianissimo_with_progress(duration, |callback| {
                backend.transcribe(&audio, callback)
            })?;
            (result.text, "pianissimo-sv".to_string())
        }
    };

    let (text, vocabulary_corrections) = glossary.correct_text(&text);
    let has_text = !text.trim().is_empty();

    // Output
    if args.json {
        let json = serde_json::json!({
            "text": text,
            "language": language,
            "model": model_id,
            "duration_seconds": duration,
            "vocabulary_corrections": vocabulary_corrections,
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        let mut stdout = io::stdout().lock();
        write_plain_record_output(&mut stdout, &text).map_err(|error| {
            DictationError::TranscriptionFailed(format!("Failed to write output: {error}"))
        })?;
    }

    if args.clipboard && has_text {
        copy_to_clipboard(&text)?;
        eprintln!("Copied to clipboard.");
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordModel {
    Whisper(WhisperModel),
    Pianissimo,
}

fn resolve_record_model(
    model_arg: Option<&str>,
    language: Language,
    auto_select_model: bool,
    whisper_fallback: WhisperModel,
    use_saved_pianissimo: bool,
) -> Result<RecordModel, DictationError> {
    match model_arg {
        Some("pianissimo-sv") => {
            if language != Language::Swedish {
                return Err(DictationError::SettingsError(
                    "Pianissimo Q8 supports Swedish dictation only; use --language sv or choose a Whisper model".into(),
                ));
            }
            Ok(RecordModel::Pianissimo)
        }
        Some(model_arg) => resolve_effective_model(
            Some(model_arg),
            language,
            auto_select_model,
            whisper_fallback,
        )
        .map(RecordModel::Whisper),
        None if use_saved_pianissimo && language == Language::Swedish => {
            Ok(RecordModel::Pianissimo)
        }
        None => resolve_effective_model(None, language, auto_select_model, whisper_fallback)
            .map(RecordModel::Whisper),
    }
}

fn validate_pianissimo_record_options(
    prompt: bool,
    prompt_file: bool,
) -> Result<(), DictationError> {
    if prompt || prompt_file {
        return Err(DictationError::SettingsError(
            "Pianissimo Q8 does not support Whisper decoder hints (--hint/--prompt/--prompt-file); remove that option, add replacement aliases to a profile with `sagascript glossary add TERM --alias ALIAS --profile ID` and select it with --profile ID, or choose a Whisper model with --model MODEL_ID".into(),
        ));
    }
    Ok(())
}

fn transcribe_pianissimo_with_progress<T>(
    duration: f64,
    transcribe: impl FnOnce(Box<dyn Fn(u8) + Send>) -> Result<T, DictationError>,
) -> Result<T, DictationError> {
    if duration > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap());
        let pb_cb = pb.clone();
        let result = transcribe(Box::new(move |pct| {
            crate::set_transcription_progress(&pb_cb, i32::from(pct));
        }));
        pb.finish_and_clear();
        result
    } else {
        eprintln!("Transcribing...");
        transcribe(Box::new(|_| {}))
    }
}

fn write_plain_record_output(writer: &mut impl Write, text: &str) -> io::Result<bool> {
    if text.trim().is_empty() {
        return Ok(false);
    }

    writeln!(writer, "{text}")?;
    Ok(true)
}

fn ctrlc_handler(running: Arc<AtomicBool>) {
    let _ = ctrlc::set_handler(move || {
        running.store(false, Ordering::Relaxed);
    });
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_record_model, validate_pianissimo_record_options, write_plain_record_output,
        RecordModel,
    };
    use sagascript_core::settings::{Language, WhisperModel};

    const FALLBACK: WhisperModel = WhisperModel::KbWhisperLarge;

    #[test]
    fn explicit_pianissimo_selects_swedish_engine() {
        assert_eq!(
            resolve_record_model(
                Some("pianissimo-sv"),
                Language::Swedish,
                true,
                FALLBACK,
                false,
            )
            .unwrap(),
            RecordModel::Pianissimo
        );
    }

    #[test]
    fn explicit_pianissimo_rejects_non_swedish_language() {
        for language in [
            Language::English,
            Language::Norwegian,
            Language::Finnish,
            Language::Auto,
        ] {
            let error =
                resolve_record_model(Some("pianissimo-sv"), language, false, FALLBACK, false)
                    .unwrap_err();
            assert!(error.to_string().contains("Swedish dictation only"));
        }
    }

    #[test]
    fn saved_pianissimo_setting_applies_only_to_swedish() {
        assert_eq!(
            resolve_record_model(None, Language::Swedish, false, FALLBACK, true).unwrap(),
            RecordModel::Pianissimo
        );
        assert_eq!(
            resolve_record_model(None, Language::Swedish, false, FALLBACK, false).unwrap(),
            RecordModel::Whisper(FALLBACK)
        );
        assert_eq!(
            resolve_record_model(None, Language::English, false, FALLBACK, true).unwrap(),
            RecordModel::Whisper(FALLBACK)
        );
    }

    #[test]
    fn explicit_whisper_model_overrides_saved_pianissimo_setting() {
        assert_eq!(
            resolve_record_model(
                Some("kb-whisper-small"),
                Language::Swedish,
                false,
                FALLBACK,
                true,
            )
            .unwrap(),
            RecordModel::Whisper(WhisperModel::KbWhisperSmall)
        );
    }

    #[test]
    fn unknown_explicit_model_errors_instead_of_falling_back() {
        let error = resolve_record_model(
            Some("not-a-model"),
            Language::Swedish,
            false,
            FALLBACK,
            true,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unknown model"));
    }

    #[test]
    fn non_swedish_language_keeps_whisper_resolution() {
        assert_eq!(
            resolve_record_model(None, Language::English, true, FALLBACK, true).unwrap(),
            RecordModel::Whisper(WhisperModel::recommended(Language::English))
        );
    }

    #[test]
    fn explicit_pianissimo_hint_options_return_actionable_error() {
        assert!(validate_pianissimo_record_options(false, false).is_ok());

        for error in [
            validate_pianissimo_record_options(true, false).unwrap_err(),
            validate_pianissimo_record_options(false, true).unwrap_err(),
        ] {
            let message = error.to_string();
            assert!(message.contains("does not support Whisper decoder hints"));
            assert!(message.contains("sagascript glossary add TERM --alias ALIAS --profile ID"));
            assert!(message.contains("--model"));
        }
    }

    #[test]
    fn empty_plain_record_output_emits_nothing() {
        for text in ["", " \n\t"] {
            let mut output = Vec::new();
            assert!(!write_plain_record_output(&mut output, text).unwrap());
            assert!(output.is_empty());
        }
    }

    #[test]
    fn nonempty_plain_record_output_keeps_text_and_newline() {
        let mut output = Vec::new();
        assert!(write_plain_record_output(&mut output, "hello").unwrap());
        assert_eq!(output, b"hello\n");
    }
}
