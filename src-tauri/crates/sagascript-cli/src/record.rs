use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::AudioCaptureService;
use sagascript_core::audio::resample::TARGET_SAMPLE_RATE;
use sagascript_core::error::DictationError;
use sagascript_core::transcription::model;
use sagascript_core::transcription::{Glossary, TranscribeOptions, WhisperBackend};

use super::transcribe::{
    copy_to_clipboard, effective_glossary, model_id_string, parse_language,
    resolve_effective_model, resolve_profile,
};

#[derive(Args)]
pub struct RecordArgs {
    /// Language for transcription [possible values: en, sv, no, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// Whisper model ID to use [see: sagascript list-models]
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

    /// Hint the decoder with domain-specific vocabulary (Whisper initial prompt).
    /// Reduces mishearings of proper nouns, foreign names, and jargon.
    /// Example: --hint "Notre Dame, Sara, Grimnir"
    #[arg(long, visible_alias = "hint", value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Read the hint/initial prompt from a file instead of the command line.
    /// Mutually exclusive with --hint/--prompt.
    #[arg(long, visible_alias = "hint-file", value_name = "PATH", conflicts_with = "prompt")]
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
    // Resolve the effective source before model work or recording. Save-only
    // output never transcribes, so it intentionally does not read a hint file.
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

    // Only validate model if we're going to transcribe
    let model = if !save_only {
        let m = resolve_effective_model(
            args.model.as_deref(),
            language,
            stored.auto_select_model,
            stored.whisper_model,
        )?;
        if !model::is_model_downloaded(m) {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' is not downloaded. Run: sagascript download-model {}",
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

    let audio = capture.stop_capture()?;
    let duration = audio.len() as f64 / TARGET_SAMPLE_RATE as f64;
    eprintln!("Captured {:.1}s of audio ({} samples)", duration, audio.len());

    if audio.is_empty() {
        return Err(DictationError::NoAudioCaptured);
    }

    // Save WAV if requested
    if let Some(output_path) = &args.output {
        let wav_bytes = sagascript_core::audio::wav::encode_wav(&audio);
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

    let decoder_prompt = glossary.decoder_prompt();
    let prompt = decoder_prompt.as_deref();
    let opts = TranscribeOptions {
        prompt: prompt.map(str::to_string),
        ..TranscribeOptions::default()
    };
    let text = if duration > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(
            ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%")
                .unwrap(),
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

    let (text, vocabulary_corrections) = glossary.correct_text(&text);
    let has_text = !text.trim().is_empty();

    // Output
    if args.json {
        let json = serde_json::json!({
            "text": text,
            "language": language,
            "model": model_id_string(model),
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
    use super::write_plain_record_output;

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
