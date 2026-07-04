use std::path::PathBuf;

use clap::Args;

use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::{TranscribeOptions, WhisperBackend};

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

    /// Enable speaker diarization (requires diarization models — run: sagascript download-model diarization)
    #[cfg(feature = "diarization")]
    #[arg(long)]
    pub diarize: bool,

    /// Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.85). Higher = fewer speakers.
    #[cfg(feature = "diarization")]
    #[arg(long, value_name = "THRESHOLD", default_value = "0.85",
          value_parser = parse_diarize_threshold,
          help = "Agglomerative clustering threshold for speaker diarization (0.0–2.0, default 0.85). Higher = fewer speakers.")]
    pub diarize_threshold: f32,

    /// Initial prompt to prime the decoder with domain-specific vocabulary.
    /// Reduces hallucination on technical terms, proper nouns, and jargon.
    /// Example: --prompt "Grimnir, MCP, Fortnox, bokföring, kontering, saldo"
    #[arg(long, value_name = "TEXT")]
    pub prompt: Option<String>,

    /// Enable voice activity detection (Silero VAD) to skip non-speech regions,
    /// reducing silence hallucination and repetition loops. Downloads a small
    /// model on first use. Overrides the `vad_enabled` setting.
    #[arg(long)]
    pub vad: bool,

    /// Disable VAD for this run, even if the `vad_enabled` setting is on.
    #[arg(long, conflicts_with = "vad")]
    pub no_vad: bool,

    /// Beam search width: 0 = greedy (fast), >=2 = beam search (more accurate,
    /// slower). Overrides the saved `beam_size` setting. When omitted, a saved
    /// `beam_size` >=2 is used; otherwise file transcription defaults to 5
    /// (pass --beam 0 to force greedy).
    #[arg(long = "beam", value_name = "N")]
    pub beam_size: Option<u32>,
}

pub fn run(args: TranscribeArgs) -> Result<(), DictationError> {
    let stored = sagascript_core::settings::store::load();
    let language = match &args.language {
        Some(l) => parse_language(l)?,
        None => stored.language,
    };
    let model = resolve_effective_model(
        args.model.as_deref(),
        language,
        stored.auto_select_model,
        stored.whisper_model,
    )?;

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

    // Effective prompt: an explicit --prompt, otherwise the saved initial_prompt.
    // Used by both the diarized and standard paths.
    let effective_prompt: Option<String> = args.prompt.clone().or_else(|| {
        let saved = stored.initial_prompt.trim();
        (!saved.is_empty()).then(|| saved.to_string())
    });

    // Diarization branch
    #[cfg(feature = "diarization")]
    if args.diarize {
        // The diarized path uses greedy timestamped decoding (DTW), so the
        // beam/VAD options don't apply — warn rather than silently ignore them.
        if args.beam_size.is_some() || args.vad || args.no_vad {
            eprintln!("Note: --beam / --vad have no effect with --diarize.");
        }
        use sagascript_core::diarization::{
            DiarizeConfig, TimestampedSegment,
            diarize,
            merge::{consolidate, merge_with_transcript},
            model::all_models_downloaded,
        };

        if !all_models_downloaded() {
            return Err(DictationError::DiarizationError(
                "Diarization models not found. Run: sagascript download-model diarization".to_string(),
            ));
        }

        // Run diarization and timestamped transcription in parallel isn't possible
        // with a single-threaded whisper context, so we run sequentially.
        eprintln!("Running speaker diarization...");
        let speaker_segments = diarize(&audio, &DiarizeConfig {
            threshold: args.diarize_threshold,
            ..DiarizeConfig::default()
        })?;
        eprintln!("Found {} speaker segment(s)", speaker_segments.len());

        eprintln!("Transcribing with word-level timestamps...");
        // Prefer word-level timestamps: each word gets its own entry so the
        // merge pipeline can assign speakers at per-word granularity instead
        // of collapsing a multi-speaker Whisper segment to a single speaker.
        let raw_segments = {
            let words = backend.transcribe_sync_with_word_timestamps(
                &audio,
                language,
                effective_prompt.as_deref(),
            )?;
            if words.is_empty() {
                // DTW timestamps unavailable — fall back to segment-level
                eprintln!("Word timestamps empty, falling back to segment-level timestamps");
                backend.transcribe_sync_with_timestamps(&audio, language, effective_prompt.as_deref())?
            } else {
                words
            }
        };
        eprintln!("Got {} word/segment(s) for merging", raw_segments.len());

        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let diarized = merge_with_transcript(&speaker_segments, &transcript);
        let consolidated = consolidate(&diarized);

        if args.json {
            let speakers: Vec<String> = {
                let mut seen = std::collections::HashSet::new();
                consolidated.iter().map(|s| s.speaker.clone()).filter(|s| seen.insert(s.clone())).collect()
            };
            let json = serde_json::json!({
                "segments": consolidated,
                "speakers": speakers,
                "language": language,
                "model": model_id_string(model),
                "file": args.file.display().to_string(),
                "duration_seconds": duration,
            });
            println!("{}", serde_json::to_string_pretty(&json).unwrap());
        } else {
            for seg in &consolidated {
                println!("[{}] {}", seg.speaker, seg.text.trim());
            }
        }

        if args.clipboard {
            let text: String = consolidated
                .iter()
                .map(|s| format!("[{}] {}", s.speaker, s.text.trim()))
                .collect::<Vec<_>>()
                .join("\n");
            copy_to_clipboard(&text)?;
            eprintln!("Copied to clipboard.");
        }

        return Ok(());
    }

    // Standard (non-diarized) transcription. Build options from the saved
    // settings, with CLI flags overriding.
    let vad_enabled = if args.no_vad {
        false
    } else if args.vad {
        true
    } else {
        stored.vad_enabled
    };
    let vad_model_path = if vad_enabled {
        let path = model::vad_model_path();
        if !path.exists() {
            eprintln!("Downloading Silero VAD model (~0.9 MB)...");
            tokio::runtime::Runtime::new()
                .map_err(|e| DictationError::ModelDownloadFailed(format!("tokio runtime: {e}")))?
                .block_on(model::download_vad_model(|_, _| {}))?;
        }
        path.to_str().map(str::to_string)
    } else {
        None
    };
    let opts = TranscribeOptions {
        prompt: effective_prompt,
        // File transcription isn't latency-sensitive, so default to beam search
        // (fewer repetition loops). Honor an explicit beam setting/flag.
        beam_size: args.beam_size.unwrap_or(if stored.beam_size >= 2 {
            stored.beam_size
        } else {
            sagascript_core::transcription::FILE_TRANSCRIBE_BEAM
        }),
        temperature_fallback: stored.temperature_fallback,
        vad_model_path,
    };
    if opts.beam_size >= 2 {
        eprintln!("Beam search: width {}", opts.beam_size);
    }
    if vad_enabled {
        eprintln!("VAD: enabled");
    }

    let text = if duration > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(
            ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap(),
        );
        let pb_cb = pb.clone();
        let text = backend.transcribe_sync_with_options(&audio, language, &opts, move |pct| {
            pb_cb.set_position(pct as u64);
        })?;
        pb.finish_and_clear();
        text
    } else {
        eprintln!("Transcribing...");
        backend.transcribe_sync_with_options(&audio, language, &opts, |_| {})?
    };

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

/// Validates a `--diarize-threshold` value: must parse as a finite f32 in the
/// documented 0.0-2.0 range. NaN/infinite or out-of-range values silently
/// produce degenerate agglomerative clustering downstream, so reject them at
/// the CLI boundary rather than at the clustering call site.
#[cfg(feature = "diarization")]
fn parse_diarize_threshold(s: &str) -> Result<f32, String> {
    let value: f32 = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number"))?;
    if !value.is_finite() {
        return Err(format!(
            "diarize-threshold must be a finite number, got '{s}'"
        ));
    }
    if !(0.0..=2.0).contains(&value) {
        return Err(format!(
            "diarize-threshold must be between 0.0 and 2.0, got {value}"
        ));
    }
    Ok(value)
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

/// Resolves the whisper model to use for a run: an explicit `--model` argument
/// always wins; otherwise, if `auto_select_model` is set, the model
/// recommended for `language` is used; otherwise `fallback` (the stored
/// `whisper_model` setting) is used, ignoring `language`.
///
/// This is the single source of truth for the branch shared by
/// `transcribe::run()` and `record::run()` — keep their call sites in sync
/// with this function rather than re-deriving the logic inline.
///
/// Note: this intentionally does not delegate to
/// `Settings::effective_model()` (core/settings/manager.rs), because that
/// method always uses the *stored* `Settings::language`, while callers here
/// need to honor a `--language` override that differs from the stored value
/// (e.g. `--language sv` with auto-select on should recommend the Swedish
/// model even if the stored language is English).
pub fn resolve_effective_model(
    model_arg: Option<&str>,
    language: Language,
    auto_select_model: bool,
    fallback: WhisperModel,
) -> Result<WhisperModel, DictationError> {
    match model_arg {
        Some(s) => parse_model(s),
        None => Ok(if auto_select_model {
            WhisperModel::recommended(language)
        } else {
            fallback
        }),
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
        "kb-whisper-medium" => Ok(WhisperModel::KbWhisperMedium),
        "kb-whisper-large" => Ok(WhisperModel::KbWhisperLarge),
        "nb-whisper-tiny" => Ok(WhisperModel::NbWhisperTiny),
        "nb-whisper-base" => Ok(WhisperModel::NbWhisperBase),
        "nb-whisper-small" => Ok(WhisperModel::NbWhisperSmall),
        "nb-whisper-medium" => Ok(WhisperModel::NbWhisperMedium),
        "nb-whisper-large" => Ok(WhisperModel::NbWhisperLarge),
        "small.en" => Ok(WhisperModel::SmallEn),
        "small" => Ok(WhisperModel::Small),
        "medium.en" => Ok(WhisperModel::MediumEn),
        "medium" => Ok(WhisperModel::Medium),
        "large-v3-turbo" => Ok(WhisperModel::LargeV3Turbo),
        "large-v3-turbo-q8_0" => Ok(WhisperModel::LargeV3TurboQ8),
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
        WhisperModel::KbWhisperMedium => "kb-whisper-medium",
        WhisperModel::KbWhisperLarge => "kb-whisper-large",
        WhisperModel::NbWhisperTiny => "nb-whisper-tiny",
        WhisperModel::NbWhisperBase => "nb-whisper-base",
        WhisperModel::NbWhisperSmall => "nb-whisper-small",
        WhisperModel::NbWhisperMedium => "nb-whisper-medium",
        WhisperModel::NbWhisperLarge => "nb-whisper-large",
        WhisperModel::SmallEn => "small.en",
        WhisperModel::Small => "small",
        WhisperModel::MediumEn => "medium.en",
        WhisperModel::Medium => "medium",
        WhisperModel::LargeV3Turbo => "large-v3-turbo",
        WhisperModel::LargeV3TurboQ8 => "large-v3-turbo-q8_0",
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
            ("kb-whisper-medium", WhisperModel::KbWhisperMedium),
            ("kb-whisper-large", WhisperModel::KbWhisperLarge),
            ("nb-whisper-tiny", WhisperModel::NbWhisperTiny),
            ("nb-whisper-base", WhisperModel::NbWhisperBase),
            ("nb-whisper-small", WhisperModel::NbWhisperSmall),
            ("nb-whisper-medium", WhisperModel::NbWhisperMedium),
            ("nb-whisper-large", WhisperModel::NbWhisperLarge),
            ("small.en", WhisperModel::SmallEn),
            ("small", WhisperModel::Small),
            ("medium.en", WhisperModel::MediumEn),
            ("medium", WhisperModel::Medium),
            ("large-v3-turbo", WhisperModel::LargeV3Turbo),
            ("large-v3-turbo-q8_0", WhisperModel::LargeV3TurboQ8),
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
            (WhisperModel::SmallEn, "small.en"),
            (WhisperModel::Small, "small"),
            (WhisperModel::MediumEn, "medium.en"),
            (WhisperModel::Medium, "medium"),
            (WhisperModel::LargeV3Turbo, "large-v3-turbo"),
            (WhisperModel::LargeV3TurboQ8, "large-v3-turbo-q8_0"),
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
            WhisperModel::SmallEn,
            WhisperModel::Small,
            WhisperModel::MediumEn,
            WhisperModel::Medium,
            WhisperModel::LargeV3Turbo,
            WhisperModel::LargeV3TurboQ8,
        ];
        for model in all_models {
            let id = model_id_string(model);
            let parsed = parse_model(id).unwrap();
            assert_eq!(parsed, model, "roundtrip failed for {id}");
        }
    }

    // -- resolve_effective_model --
    //
    // These exercise the exact branch used by both transcribe::run() and
    // record::run(): explicit arg wins -> auto_select_model recommends by
    // language -> otherwise the stored fallback model (language ignored).

    #[test]
    fn resolve_effective_model_none_auto_recommends_by_language() {
        let result =
            resolve_effective_model(None, Language::Swedish, true, WhisperModel::Base).unwrap();
        assert_eq!(result, WhisperModel::KbWhisperBase);
    }

    #[test]
    fn resolve_effective_model_none_no_auto_uses_fallback_ignoring_language() {
        let result = resolve_effective_model(
            None,
            Language::Swedish,
            false,
            WhisperModel::LargeV3Turbo,
        )
        .unwrap();
        assert_eq!(result, WhisperModel::LargeV3Turbo);
    }

    #[test]
    fn resolve_effective_model_explicit_arg_wins_over_auto_select() {
        let result = resolve_effective_model(
            Some("tiny.en"),
            Language::Swedish,
            true,
            WhisperModel::Base,
        )
        .unwrap();
        assert_eq!(result, WhisperModel::TinyEn);
    }

    #[test]
    fn resolve_effective_model_invalid_arg_errors() {
        assert!(
            resolve_effective_model(Some("bogus"), Language::Auto, true, WhisperModel::Base)
                .is_err()
        );
    }
}

// -- diarize_threshold validation --
//
// Exercised via clap's try_parse_from against a small wrapper so the
// value_parser attribute itself (not just the bare parsing function) is
// under test.
#[cfg(all(test, feature = "diarization"))]
mod diarize_threshold_tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(flatten)]
        args: TranscribeArgs,
    }

    fn parse_threshold(value: &str) -> Result<f32, String> {
        TestCli::try_parse_from(["sagascript", "file.wav", "--diarize-threshold", value])
            .map(|cli| cli.args.diarize_threshold)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn rejects_nan() {
        assert!(parse_threshold("nan").is_err());
    }

    #[test]
    fn rejects_negative() {
        assert!(parse_threshold("-1.0").is_err());
    }

    #[test]
    fn rejects_above_range() {
        assert!(parse_threshold("3.0").is_err());
    }

    #[test]
    fn accepts_default_value() {
        assert_eq!(parse_threshold("0.85").unwrap(), 0.85);
    }

    #[test]
    fn accepts_in_range_value() {
        assert_eq!(parse_threshold("1.5").unwrap(), 1.5);
    }

    #[test]
    fn default_applies_when_flag_omitted() {
        let cli = TestCli::try_parse_from(["sagascript", "file.wav"]).unwrap();
        assert_eq!(cli.args.diarize_threshold, 0.85);
    }

    #[test]
    fn accepts_boundary_values() {
        assert_eq!(parse_threshold("0.0").unwrap(), 0.0);
        assert_eq!(parse_threshold("2.0").unwrap(), 2.0);
    }
}
