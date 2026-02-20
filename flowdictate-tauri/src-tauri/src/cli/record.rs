use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Args;

use crate::audio::AudioCaptureService;
use crate::audio::resample::TARGET_SAMPLE_RATE;
use crate::error::DictationError;
use crate::transcription::model;
use crate::transcription::WhisperBackend;

use super::transcribe::{copy_to_clipboard, model_id_string, parse_language, resolve_model};

#[derive(Args)]
pub struct RecordArgs {
    /// Language: en, sv, no, auto
    #[arg(short, long, default_value = "auto")]
    pub language: String,

    /// Model ID (e.g. base.en, nb-whisper-base). Default: auto-select for language
    #[arg(short, long)]
    pub model: Option<String>,

    /// Max recording duration in seconds (default: record until Ctrl+C)
    #[arg(short, long)]
    pub duration: Option<f64>,

    /// Save audio to WAV file instead of transcribing
    #[arg(short, long)]
    pub output: Option<String>,

    /// Output result as JSON
    #[arg(long)]
    pub json: bool,

    /// Copy result to clipboard
    #[arg(long)]
    pub clipboard: bool,
}

pub fn run(args: RecordArgs) -> Result<(), DictationError> {
    let language = parse_language(&args.language)?;
    let save_only = args.output.is_some();

    // Only validate model if we're going to transcribe
    let model = if !save_only {
        let m = resolve_model(args.model.as_deref(), language)?;
        if !model::is_model_downloaded(m) {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' is not downloaded. Run: flowdictate download-model {}",
                m.display_name(),
                model_id_string(m)
            )));
        }
        Some(m)
    } else {
        None
    };

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

    let audio = capture.stop_capture();
    let duration = audio.len() as f64 / TARGET_SAMPLE_RATE as f64;
    eprintln!("Captured {:.1}s of audio ({} samples)", duration, audio.len());

    if audio.is_empty() {
        return Err(DictationError::NoAudioCaptured);
    }

    // Save WAV if requested
    if let Some(output_path) = &args.output {
        let wav_bytes = crate::audio::wav::encode_wav(&audio);
        std::fs::write(output_path, &wav_bytes).map_err(|e| {
            DictationError::FileDecodeError(format!("Failed to write WAV: {e}"))
        })?;
        eprintln!("Saved to {output_path}");
        return Ok(());
    }

    // Transcribe
    let model = model.unwrap();
    eprintln!("Loading model: {}...", model.display_name());
    let backend = WhisperBackend::new();
    backend.load_model(model)?;

    eprintln!("Transcribing...");
    let text = backend.transcribe_sync(&audio, language)?;

    // Output
    if args.json {
        let json = serde_json::json!({
            "text": text,
            "language": args.language,
            "model": model_id_string(model),
            "duration_seconds": duration,
        });
        println!("{}", serde_json::to_string_pretty(&json).unwrap());
    } else {
        println!("{text}");
    }

    if args.clipboard {
        copy_to_clipboard(&text)?;
        eprintln!("Copied to clipboard.");
    }

    Ok(())
}

fn ctrlc_handler(running: Arc<AtomicBool>) {
    let _ = ctrlc::set_handler(move || {
        running.store(false, Ordering::Relaxed);
    });
}
