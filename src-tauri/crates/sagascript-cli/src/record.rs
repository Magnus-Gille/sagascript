use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::resample::TARGET_SAMPLE_RATE;
use sagascript_core::audio::system::recorder::{SystemRecorder, SystemTrack};
use sagascript_core::audio::system::twotrack::{
    mix_tracks, pad_front, track_start_padding, write_two_track_wav,
};
use sagascript_core::audio::system::{AppSelector, AudioSource, CaptureTarget};
use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{HotkeyProfile, Language, Settings, WhisperModel};
use sagascript_core::transcription::live_dictation::LiveDictationBackend;
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::{Glossary, TranscribeOptions, WhisperBackend};

use super::transcribe::{
    copy_to_clipboard, effective_glossary, model_id_string, parse_language,
    resolve_effective_model, resolve_one_run_prompt, resolve_profile,
};

#[derive(Args)]
pub struct RecordArgs {
    /// Language for transcription [possible values: en, sv, no, fi, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language, model and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// Whisper model ID or pianissimo-sv (Swedish only) [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Max recording duration in seconds (default: record until Ctrl+C)
    #[arg(short, long, value_name = "SECONDS")]
    pub duration: Option<f64>,

    /// Audio source: the microphone, the computer's system audio, or both as two tracks
    #[arg(long, value_name = "SOURCE", default_value = "mic", value_parser = ["mic", "system", "both"])]
    pub source: String,

    /// Limit system capture to one app: macOS bundle id, pid, or executable name
    #[arg(long, value_name = "APP")]
    pub app: Option<String>,

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

/// Parse `--source`/`--app` into a validated capture plan.
pub fn resolve_source(source: &str, app: Option<&str>) -> Result<(AudioSource, CaptureTarget), DictationError> {
    let source: AudioSource = source.parse()?;
    let target = match app {
        None => CaptureTarget::All,
        Some(_) if !source.needs_system() => {
            return Err(DictationError::SettingsError(
                "--app only applies to system audio; use --source system or --source both".into(),
            ))
        }
        Some(app) => CaptureTarget::App(AppSelector::parse(app)?),
    };
    Ok((source, target))
}

/// The microphone service keeps at most this many seconds in memory and then
/// silently stops appending. `--source both` must not let that truncate the mic
/// channel while the system track keeps growing, so it is limited to this length.
fn max_record_secs(source: AudioSource) -> Option<f64> {
    (source == AudioSource::Both).then_some(sagascript_core::audio::capture::MAX_BUFFER_SECONDS as f64)
}

fn validate_duration(source: AudioSource, duration: Option<f64>) -> Result<(), DictationError> {
    match (max_record_secs(source), duration) {
        (Some(max), Some(d)) if d > max => Err(DictationError::SettingsError(format!(
            "--source both is limited to {} minutes (the microphone buffer is in memory); \
             use --source system for longer recordings, or record in parts",
            (max / 60.0) as u32
        ))),
        _ => Ok(()),
    }
}

const SYSTEM_AUDIO_BANNER: &str = "\
== RECORDING SYSTEM AUDIO ==  Everything the computer plays is being captured and stored locally.\n\
   Recording other people may require their consent. Press Ctrl+C to stop.";

pub fn run(args: RecordArgs) -> Result<(), DictationError> {
    let (source, target) = resolve_source(&args.source, args.app.as_deref())?;
    validate_duration(source, args.duration)?;
    let stored = sagascript_core::settings::store::load();
    let profile = match args.profile.as_deref() {
        Some(profile_id) => resolve_profile(&stored, profile_id)?,
        None => stored.resolved_hotkey_profiles().into_iter()
            .find(|candidate| candidate.id == "default")
            .or_else(|| stored.resolved_hotkey_profiles().into_iter().next())
            .expect("settings always resolve a default profile"),
    };
    let language = args.language.as_deref().map(parse_language).transpose()?.unwrap_or(profile.language);
    let save_only = args.output.is_some();
    // Saving audio does not select or require a transcription model. Resolve
    // and validate the engine before capture for every transcription run.
    let selected_model = if save_only {
        None
    } else {
        let mut selected = resolve_record_model_for_profile(&stored, &profile, language, args.model.as_deref())?;
        // Pianissimo has no decoder hints. When it was only implied by
        // "recommended" (no --model, profile on Auto or a language override),
        // decode with the recommended Whisper model instead of failing.
        let implicit_pianissimo = args.model.is_none()
            && (language != profile.language
                || stored.profile_models.get(&profile.id).is_none_or(|p| *p == sagascript_core::settings::FileModelPreference::Auto));
        if selected == RecordModel::Pianissimo
            && implicit_pianissimo
            && (args.prompt.is_some() || args.prompt_file.is_some())
        {
            selected = RecordModel::Whisper(WhisperModel::recommended(language));
        }
        if selected == RecordModel::Pianissimo {
            validate_pianissimo_record_options(args.prompt.is_some(), args.prompt_file.is_some())?;
        }
        Some(selected)
    };
    // Save-only output does not transcribe, so it intentionally does not read
    // a hint file. Transcription keeps the normal scoped glossary behavior.
    let glossary = if save_only {
        Glossary::parse("")
    } else if language == profile.language {
        effective_glossary(&stored, Some(&profile.id), args.prompt.as_deref(), args.prompt_file.as_deref())?
    } else {
        let hint = resolve_one_run_prompt(args.prompt.as_deref(), args.prompt_file.as_deref())?;
        Glossary::parse(hint.as_deref().unwrap_or(""))
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
                if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os() {
                    return Err(DictationError::TranscriptionFailed(
                        sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE.into(),
                    ));
                }
                if !pianissimo_model::is_downloaded() {
                    return Err(DictationError::TranscriptionFailed(
                        "Pianissimo is not downloaded. Run: sagascript download-model pianissimo-sv".into(),
                    ));
                }
            }
        }
    }

    // Set up Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc_handler(r);

    // Start recording. The banner is the visible indicator and consent reminder
    // and must appear before any system-audio capture begins.
    if source.needs_system() {
        eprintln!("{SYSTEM_AUDIO_BANNER}");
    }
    let mut capture = AudioCaptureService::new();
    let mic_requested = std::time::Instant::now();
    if source.needs_mic() {
        capture.start_capture()?;
    }
    let system = if source.needs_system() {
        let started = std::time::Instant::now();
        match SystemRecorder::start(&target) {
            Ok(recorder) => Some((recorder, started)),
            Err(error) => {
                if source.needs_mic() {
                    let _ = capture.stop_capture();
                }
                return Err(error);
            }
        }
    } else {
        None
    };

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
        if let Some(max) = max_record_secs(source) {
            if start.elapsed().as_secs_f64() >= max {
                eprintln!("Stopping: --source both is limited to {} minutes.", (max / 60.0) as u32);
                break;
            }
        }
    }

    let mic_first_callback_ms = capture.metrics().first_callback_ms.unwrap_or(0);
    // Stop and join BOTH paths before propagating either error, so a microphone
    // failure cannot leave the system recorder (and its spool file) behind.
    let mic_result = if source.needs_mic() { Some(capture.stop_capture()) } else { None };
    let system_result = system.map(|(recorder, started)| {
        let offset_ms = started.duration_since(mic_requested).as_millis() as u64;
        (recorder.stop(), offset_ms)
    });
    let mic_audio = mic_result.transpose()?;
    let system_track: Option<(SystemTrack, u64)> =
        system_result.map(|(track, offset_ms)| track.map(|t| (t, offset_ms))).transpose()?;
    if let Some((track, _)) = &system_track {
        if track.silent {
            eprintln!("Warning: the system-audio track is silent. Nothing was playing, or permission was denied (run `sagascript doctor`).");
        }
    }
    let (audio, two_track) = combine_tracks(source, mic_audio, system_track, mic_first_callback_ms);
    let duration = audio.len() as f64 / TARGET_SAMPLE_RATE as f64;
    if let Some((mic, system)) = &two_track {
        eprintln!(
            "Track lengths: mic {:.1}s, system {:.1}s (drift {} ms)",
            mic.len() as f64 / TARGET_SAMPLE_RATE as f64,
            system.len() as f64 / TARGET_SAMPLE_RATE as f64,
            sagascript_core::audio::system::convert::drift_ms(mic.len(), system.len())
        );
    }
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
        if let Some((mic, system)) = &two_track {
            write_two_track_wav(std::path::Path::new(output_path), mic, system)
                .map_err(|e| DictationError::FileDecodeError(format!("Failed to write WAV: {e}")))?;
            eprintln!("Two-track WAV: left = microphone, right = system audio.");
        } else {
            // Same 0600 writer as the two-track path: meeting audio must not be
            // created with the default umask.
            let write = || -> std::io::Result<()> {
                sagascript_core::audio::system::twotrack::check_wav_capacity(audio.len() as u64, 1)?;
                let mut w = sagascript_core::audio::system::twotrack::TwoTrackWriter::create(
                    std::path::Path::new(output_path),
                    1,
                )?;
                w.write_interleaved(&audio)?;
                w.finish().map(|_| ())
            };
            write().map_err(|e| DictationError::FileDecodeError(format!("Failed to write WAV: {e}")))?;
        }
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
            eprintln!("Loading model: Pianissimo...");
            let text = transcribe_pianissimo_record(&audio, duration)?;
            (text, "pianissimo-sv")
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
            "source": args.source,
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

/// Aligned (microphone, system) tracks of a `both` recording.
type AlignedTracks = (Vec<f32>, Vec<f32>);

/// Returns the mono audio to transcribe and, for `both`, the aligned
/// (microphone, system) tracks for a two-track file.
fn combine_tracks(
    source: AudioSource,
    mic: Option<Vec<f32>>,
    system: Option<(SystemTrack, u64)>,
    mic_first_callback_ms: u64,
) -> (Vec<f32>, Option<AlignedTracks>) {
    match (source, mic, system) {
        (AudioSource::Both, Some(mic), Some((system, system_offset_ms))) => {
            // Sample 0 of the mic is its first callback; sample 0 of the system
            // track is the moment its capture was requested.
            let (mic_pad, system_pad) = track_start_padding(mic_first_callback_ms, system_offset_ms);
            let mic = pad_front(&mic, mic_pad);
            let system = pad_front(&system.samples, system_pad);
            (mix_tracks(&mic, &system), Some((mic, system)))
        }
        (_, Some(mic), None) => (mic, None),
        (_, None, Some((system, _))) => (system.samples, None),
        _ => (Vec::new(), None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordModel {
    Whisper(WhisperModel),
    Pianissimo,
}

fn resolve_record_model_for_profile(
    stored: &Settings,
    profile: &HotkeyProfile,
    language: Language,
    explicit: Option<&str>,
) -> Result<RecordModel, DictationError> {
    if explicit.is_none() && language == profile.language {
        let (whisper, pianissimo) = stored.live_model_for_profile(&profile.id)
            .map_err(DictationError::SettingsError)?;
        return Ok(if pianissimo { RecordModel::Pianissimo } else { RecordModel::Whisper(whisper) });
    }
    resolve_record_model(
        explicit,
        language,
        true,
        WhisperModel::recommended(language),
        // "Recommended for Swedish" is Pianissimo wherever it runs.
        sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os(),
    )
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
                    "Pianissimo supports Swedish dictation only; use --language sv or choose a Whisper model".into(),
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
            "Pianissimo does not support Whisper decoder hints (--hint/--prompt/--prompt-file); remove that option, add replacement aliases to a profile with `sagascript glossary add TERM --alias ALIAS --profile ID` and select it with --profile ID, or choose a Whisper model with --model MODEL_ID".into(),
        ));
    }
    Ok(())
}

fn transcribe_pianissimo_record(audio: &[f32], duration: f64) -> Result<String, DictationError> {
    let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true);
    transcribe_pianissimo_with_progress(duration, |progress| {
        progress(0);
        let result = backend.transcribe(
            WhisperModel::Base,
            audio,
            Language::Swedish,
            &TranscribeOptions::default(),
            &mut Default::default(),
        );
        if result.is_ok() {
            progress(100);
        }
        result
    })
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
        combine_tracks, resolve_source, SystemTrack,
        resolve_record_model, resolve_record_model_for_profile, validate_pianissimo_record_options, write_plain_record_output,
        RecordModel,
    };
    use sagascript_core::settings::{FileModelPreference, HotkeyProfile, Language, Settings, WhisperModel};

    const FALLBACK: WhisperModel = WhisperModel::KbWhisperLarge;

    #[test]
    fn two_profiles_keep_independent_record_engines_and_explicit_override() {
        let mut settings = Settings::default();
        let swedish = HotkeyProfile::legacy_default("Super+S".into(), Language::Swedish);
        let mut english = HotkeyProfile::legacy_default("Super+E".into(), Language::English);
        english.id = "english".into();
        english.name = "English".into();
        settings.hotkey_profiles = vec![swedish.clone(), english.clone()];
        settings.profile_models.insert("default".into(), FileModelPreference::PianissimoOriginal);
        settings.profile_models.insert("english".into(), FileModelPreference::Whisper(WhisperModel::SmallEn));
        assert_eq!(resolve_record_model_for_profile(&settings, &swedish, Language::Swedish, None).unwrap(), RecordModel::Pianissimo);
        assert_eq!(resolve_record_model_for_profile(&settings, &english, Language::English, None).unwrap(), RecordModel::Whisper(WhisperModel::SmallEn));
        assert_eq!(resolve_record_model_for_profile(&settings, &swedish, Language::Swedish, Some("kb-whisper-base")).unwrap(), RecordModel::Whisper(WhisperModel::KbWhisperBase));
    }

    #[test]
    fn pianissimo_silence_skips_native_model_in_record() {
        for audio in [vec![0.0; 1600], vec![0.00001; 1600]] {
            assert_eq!(
                super::transcribe_pianissimo_record(&audio, 0.1).unwrap(),
                ""
            );
        }
    }

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

    #[test]
    fn both_is_limited_to_the_microphone_buffer_length() {
        use sagascript_core::audio::system::AudioSource;
        assert!(super::validate_duration(AudioSource::Both, Some(900.0)).is_ok());
        assert!(super::validate_duration(AudioSource::Both, Some(901.0)).is_err());
        assert!(super::validate_duration(AudioSource::Both, None).is_ok());
        assert!(super::validate_duration(AudioSource::System, Some(7200.0)).is_ok());
        assert_eq!(super::max_record_secs(AudioSource::System), None);
    }

    #[test]
    fn source_defaults_to_mic_and_validates_app() {
        use sagascript_core::audio::system::{AppSelector, AudioSource, CaptureTarget};
        assert_eq!(resolve_source("mic", None).unwrap(), (AudioSource::Mic, CaptureTarget::All));
        assert_eq!(
            resolve_source("both", Some("us.zoom.xos")).unwrap(),
            (AudioSource::Both, CaptureTarget::App(AppSelector::BundleId("us.zoom.xos".into())))
        );
        assert!(resolve_source("mic", Some("1234")).is_err());
        assert!(resolve_source("speakers", None).is_err());
    }

    #[test]
    fn both_aligns_and_mixes_tracks() {
        use sagascript_core::audio::system::{AudioSource, NativeFormat};
        let system = SystemTrack {
            samples: vec![1.0; 16],
            native: NativeFormat { sample_rate: 48_000, channels: 2 },
            silent: false,
        };
        // System started 1 ms (16 samples) after the mic's first callback.
        let (mixed, two) = combine_tracks(AudioSource::Both, Some(vec![1.0; 32]), Some((system, 1)), 0);
        let (mic, sys) = two.unwrap();
        assert_eq!((mic.len(), sys.len()), (32, 32));
        assert_eq!(sys[0], 0.0);
        assert_eq!(sys[16], 1.0);
        assert_eq!(mixed[0], 0.5);
        assert_eq!(mixed[20], 1.0);
    }

    #[test]
    fn system_only_uses_system_samples() {
        use sagascript_core::audio::system::{AudioSource, NativeFormat};
        let system = SystemTrack {
            samples: vec![0.25; 4],
            native: NativeFormat { sample_rate: 16_000, channels: 1 },
            silent: false,
        };
        let (audio, two) = combine_tracks(AudioSource::System, None, Some((system, 0)), 0);
        assert_eq!(audio, vec![0.25; 4]);
        assert!(two.is_none());
    }
}
