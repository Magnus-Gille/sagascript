use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::Args;
use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::audio::resample::TARGET_SAMPLE_RATE;
use sagascript_core::audio::AudioCaptureService;
use sagascript_core::error::DictationError;
use sagascript_core::settings::{HotkeyProfile, Language, PianissimoModel, Settings, WhisperModel};
use sagascript_core::transcription::live_dictation::LiveDictationBackend;
use sagascript_core::transcription::model;
use sagascript_core::transcription::pianissimo_model;
use sagascript_core::transcription::{
    Glossary, TranscribeOptions, TranscribeRequest, WhisperBackend,
};

use crate::output::{errln, outln, Out, WriteError};
use crate::run_request::RunRequest;

use super::transcribe::{copy_to_clipboard, model_id_string, resolve_effective_model};

#[derive(Args)]
pub struct RecordArgs {
    /// Language for transcription [possible values: en, sv, no, fi, da, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,

    /// Use this dictation profile's language, model and personal dictionary
    #[arg(long, value_name = "ID", conflicts_with = "language")]
    pub profile: Option<String>,

    /// Whisper model ID, or pianissimo-sv (Swedish) / pianissimo-da (Danish) [see: sagascript list-models]
    #[arg(short, long, value_name = "MODEL_ID")]
    pub model: Option<String>,

    /// Max recording duration in seconds (default: record until Ctrl+C)
    #[arg(short, long, value_name = "SECONDS")]
    pub duration: Option<f64>,

    /// Save audio to WAV file instead of transcribing
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<String>,

    /// Output result as JSON (includes text, language, model, duration; in recorder mode the saved recording)
    #[arg(long)]
    pub json: bool,

    /// Copy transcription result to clipboard
    #[arg(long)]
    pub clipboard: bool,

    /// Recorder mode: keep the recording as a folder of FLAC files. lossless stores the
    /// microphone as it delivers it (24-bit, native rate); compact stores 16 kHz, 16-bit
    /// (default: the recording_quality setting)
    #[arg(
        long,
        value_name = "QUALITY",
        value_parser = clap::builder::PossibleValuesParser::new(["lossless", "compact"]),
        conflicts_with_all = ["output", "clipboard"]
    )]
    pub quality: Option<String>,

    /// Recorder mode: folder that holds the recordings (default: the recording_directory
    /// setting, else Sagascript in your Music folder)
    #[arg(long, value_name = "DIR", conflicts_with_all = ["output", "clipboard"])]
    pub dir: Option<PathBuf>,

    /// Recorder mode: title of the recording (default: Recording); part of the folder name
    #[arg(long, value_name = "TITLE", conflicts_with_all = ["output", "clipboard"])]
    pub name: Option<String>,

    /// Recorder mode: transcribe the recording once it is saved and print the text
    #[arg(long, conflicts_with_all = ["output", "clipboard"])]
    pub transcribe: bool,

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

impl<'a> From<&'a RecordArgs> for RunRequest<'a> {
    fn from(args: &'a RecordArgs) -> Self {
        Self {
            profile: args.profile.as_deref(),
            language: args.language.as_deref(),
            prompt: args.prompt.as_deref(),
            prompt_file: args.prompt_file.as_deref(),
        }
    }
}

/// What `sagascript record` was asked to do. The old behaviour (capture to
/// memory, then transcribe or save a WAV) stays exactly as it was unless one of
/// the recorder flags is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordMode {
    /// `record`, `record -o file.wav`, `record --clipboard`, ...
    Dictation,
    /// `--quality`, `--dir`, `--name` or `--transcribe`: a recording folder.
    Recorder,
}

impl RecordArgs {
    pub fn mode(&self) -> RecordMode {
        if self.quality.is_some() || self.dir.is_some() || self.name.is_some() || self.transcribe {
            RecordMode::Recorder
        } else {
            RecordMode::Dictation
        }
    }
}

/// The language, the engine and the vocabulary a transcription run uses,
/// resolved and validated before any audio is captured.
pub(crate) struct Prepared {
    pub language: Language,
    /// `None` when the run only saves audio.
    pub model: Option<RecordModel>,
    pub glossary: Glossary,
}

/// Resolve the profile, language, engine and glossary, and check that the
/// engine is installed. `save_only` skips the engine and the hint file: saving
/// audio selects and requires no transcription model.
pub(crate) fn prepare(
    args: &RecordArgs,
    stored: &Settings,
    save_only: bool,
) -> Result<Prepared, DictationError> {
    let request = RunRequest::from(args);
    let selection = request.select(stored)?;
    let profile = &selection.profile;
    let language = selection.language;
    // Saving audio does not select or require a transcription model. Resolve
    // and validate the engine before capture for every transcription run.
    let selected_model = if save_only {
        None
    } else {
        let mut selected =
            resolve_record_model_for_profile(stored, profile, language, args.model.as_deref())?;
        // Pianissimo has no decoder hints. When it was only implied by
        // "recommended" (no --model, profile on Auto or a language override),
        // decode with the recommended Whisper model instead of failing.
        let implicit_pianissimo = args.model.is_none()
            && (selection.language_overridden()
                || stored
                    .profile_models
                    .get(&profile.id)
                    .is_none_or(|p| *p == sagascript_core::settings::FileModelPreference::Auto));
        if matches!(selected, RecordModel::Pianissimo(_))
            && implicit_pianissimo
            && (args.prompt.is_some() || args.prompt_file.is_some())
        {
            selected = RecordModel::Whisper(WhisperModel::recommended(language));
        }
        if matches!(selected, RecordModel::Pianissimo(_)) {
            validate_pianissimo_record_options(args.prompt.is_some(), args.prompt_file.is_some())?;
        }
        Some(selected)
    };
    // Save-only output does not transcribe, so it intentionally does not read
    // a hint file. Transcription keeps the normal scoped glossary behavior.
    let glossary = if save_only {
        Glossary::parse("")
    } else {
        request.finish(stored, selection)?.glossary
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
            RecordModel::Pianissimo(pianissimo) => {
                if !sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os(
                ) {
                    return Err(DictationError::TranscriptionFailed(
                        sagascript_core::transcription::pianissimo_backend::UNSUPPORTED_MESSAGE
                            .into(),
                    ));
                }
                if !pianissimo_model::is_downloaded(pianissimo) {
                    return Err(DictationError::TranscriptionFailed(format!(
                        "Pianissimo is not downloaded. Run: sagascript download-model {}",
                        pianissimo.id()
                    )));
                }
            }
        }
    }
    Ok(Prepared {
        language,
        model: selected_model,
        glossary,
    })
}

/// A finished transcription of recorded audio.
pub(crate) struct Transcript {
    pub text: String,
    pub model_id: &'static str,
    pub vocabulary_corrections: Vec<sagascript_core::glossary::GlossaryCorrection>,
}

/// Transcribe 16 kHz mono `audio` of `duration` seconds with the engine that
/// [`prepare`] chose and apply the vocabulary corrections. Shared by the
/// dictation mode and by the recorder's `--transcribe`.
pub(crate) fn transcribe_samples(
    audio: &[f32],
    duration: f64,
    model: RecordModel,
    language: Language,
    glossary: &Glossary,
    out: &Out,
) -> Result<Transcript, DictationError> {
    let (text, model_id) = match model {
        RecordModel::Whisper(model) => {
            errln!(out, "Loading model: {}...", model.display_name());
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
                let text = backend.transcribe_text(
                    TranscribeRequest::new(audio, language, &opts)
                        .live()
                        .on_progress(move |pct| {
                            crate::set_transcription_progress(&pb_cb, pct);
                        }),
                )?;
                pb.finish_and_clear();
                text
            } else {
                errln!(out, "Transcribing...");
                backend.transcribe_text(TranscribeRequest::new(audio, language, &opts).live())?
            };
            (text, model_id_string(model))
        }
        RecordModel::Pianissimo(pianissimo) => {
            if glossary.decoder_prompt().is_some() {
                errln!(out, "Pianissimo does not use Whisper decoder hints; aliases in the selected profile still correct the transcript after inference.");
            }
            errln!(out, "Loading model: Pianissimo...");
            let text = transcribe_pianissimo_record(audio, duration, pianissimo, out)?;
            (text, pianissimo.id())
        }
    };
    let (text, vocabulary_corrections) = glossary.correct_text(&text);
    Ok(Transcript {
        text,
        model_id,
        vocabulary_corrections,
    })
}

pub fn run(args: RecordArgs, out: &Out) -> Result<(), DictationError> {
    if args.mode() == RecordMode::Recorder {
        return crate::recorder::run(args, out);
    }
    let stored = sagascript_core::machine::load();
    let save_only = args.output.is_some();
    let Prepared {
        language,
        model: selected_model,
        glossary,
    } = prepare(&args, &stored, save_only)?;

    // Set up Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    install_stop_handler(running.clone())?;

    // Start recording
    let mut capture = AudioCaptureService::new();
    capture.start_capture()?;

    if let Some(secs) = args.duration {
        errln!(out, "Recording for {secs}s... (press Ctrl+C to stop early)");
    } else {
        errln!(out, "Recording... press Ctrl+C to stop");
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
    errln!(
        out,
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
        errln!(out, "Saved to {output_path}");
        return Ok(());
    }

    // Transcribe
    let selected_model = selected_model.expect("transcription model resolved before capture");
    let Transcript {
        text,
        model_id,
        vocabulary_corrections,
    } = transcribe_samples(&audio, duration, selected_model, language, &glossary, out)?;
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
        out.json(&json)?;
    } else {
        write_plain_record_output(out, &text)?;
    }

    if args.clipboard && has_text {
        copy_to_clipboard(&text)?;
        errln!(out, "Copied to clipboard.");
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordModel {
    Whisper(WhisperModel),
    Pianissimo(PianissimoModel),
}

fn resolve_record_model_for_profile(
    stored: &Settings,
    profile: &HotkeyProfile,
    language: Language,
    explicit: Option<&str>,
) -> Result<RecordModel, DictationError> {
    if explicit.is_none() && language == profile.language {
        let (whisper, pianissimo) = stored
            .live_model_for_profile(&profile.id)
            .map_err(DictationError::from)?;
        // A profile is on Pianissimo only in its model's language.
        return Ok(match PianissimoModel::for_language(profile.language) {
            Some(model) if pianissimo => RecordModel::Pianissimo(model),
            _ => RecordModel::Whisper(whisper),
        });
    }
    resolve_record_model(
        explicit,
        language,
        true,
        WhisperModel::recommended(language),
        // "Recommended" is the language's Pianissimo model wherever it runs.
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
    if let Some(model) = model_arg.and_then(PianissimoModel::from_id) {
        let own = model.language();
        if language != own {
            return Err(DictationError::SettingsError(format!(
                "Pianissimo supports {} dictation only; use --language {} or choose a Whisper model",
                own.display_name(),
                own.id()
            )));
        }
        return Ok(RecordModel::Pianissimo(model));
    }
    let saved_pianissimo = PianissimoModel::for_language(language).filter(|_| use_saved_pianissimo);
    match (model_arg, saved_pianissimo) {
        (None, Some(model)) => Ok(RecordModel::Pianissimo(model)),
        (model_arg, _) => {
            resolve_effective_model(model_arg, language, auto_select_model, whisper_fallback)
                .map(RecordModel::Whisper)
        }
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

fn transcribe_pianissimo_record(
    audio: &[f32],
    duration: f64,
    model: PianissimoModel,
    out: &Out,
) -> Result<String, DictationError> {
    let backend = LiveDictationBackend::new(
        Arc::new(WhisperBackend::new()),
        true,
        sagascript_core::machine::load,
    );
    transcribe_pianissimo_with_progress(duration, out, |progress| {
        progress(0);
        let result = backend.transcribe(
            WhisperModel::Base,
            audio,
            model.language(),
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
    out: &Out,
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
        errln!(out, "Transcribing...");
        transcribe(Box::new(|_| {}))
    }
}

pub(crate) fn write_plain_record_output(out: &Out, text: &str) -> Result<bool, WriteError> {
    if text.trim().is_empty() {
        return Ok(false);
    }

    outln!(out, "{text}")?;
    Ok(true)
}

/// Make Ctrl+C and SIGTERM clear `running` instead of ending the process, so a
/// recording is stopped and finished, not cut off. Install it **before** the
/// microphone is opened, and fail if it cannot be installed: without it a
/// signal would kill the process with an unfinished recording.
pub(crate) fn install_stop_handler(running: Arc<AtomicBool>) -> Result<(), DictationError> {
    ctrlc::set_handler(move || {
        running.store(false, Ordering::Relaxed);
    })
    .map_err(|error| {
        DictationError::AudioCaptureError(format!(
            "Could not install the handler for Ctrl+C and SIGTERM: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_record_model, resolve_record_model_for_profile, validate_pianissimo_record_options,
        write_plain_record_output, RecordModel,
    };
    use crate::output::Out;
    use clap::Parser;
    use sagascript_core::settings::{
        FileModelPreference, HotkeyProfile, Language, PianissimoModel, Settings, WhisperModel,
    };

    const FALLBACK: WhisperModel = WhisperModel::KbWhisperLarge;

    /// The stop handler is process-wide: a second installation is refused by
    /// the library, and that refusal is an error, not something ignored.
    #[test]
    fn a_stop_handler_that_cannot_be_installed_is_an_error() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let running = Arc::new(AtomicBool::new(true));
        super::install_stop_handler(running.clone()).expect("the first installation works");
        let error = super::install_stop_handler(running).expect_err("a second one is refused");
        assert!(error.to_string().contains("Ctrl+C"), "{error}");
    }

    fn parsed(extra: &[&str]) -> Result<super::RecordArgs, clap::Error> {
        let mut argv = vec!["sagascript", "record"];
        argv.extend_from_slice(extra);
        crate::Cli::try_parse_from(argv).map(|cli| match cli.command {
            Some(crate::Command::Record(args)) => args,
            _ => panic!("expected record"),
        })
    }

    /// D5: `record` without a recorder flag, and `record -o file.wav`, are the
    /// old command (capture to memory, then transcribe or save a WAV). The
    /// branch is a pure function of the arguments, so this needs no device.
    #[test]
    fn the_old_record_invocations_stay_in_dictation_mode() {
        for argv in [
            &[][..],
            &["-o", "capture.wav"],
            &["--output", "capture.wav", "--duration", "5"],
            &["--clipboard"],
            &["--json", "--duration", "5", "--language", "no"],
            &["--model", "base.en", "--hint", "Sara"],
            &["--profile", "default", "--prompt-file", "terms.txt"],
        ] {
            let args = parsed(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            assert_eq!(args.mode(), super::RecordMode::Dictation, "{argv:?}");
            assert!(args.quality.is_none() && args.dir.is_none() && args.name.is_none());
            assert!(!args.transcribe);
        }
        let args = parsed(&["-o", "capture.wav"]).unwrap();
        assert_eq!(args.output.as_deref(), Some("capture.wav"));
    }

    #[test]
    fn any_recorder_flag_selects_recorder_mode() {
        for argv in [
            &["--quality", "lossless"][..],
            &["--quality", "compact"],
            &["--dir", "takes"],
            &["--name", "Standup"],
            &["--transcribe"],
            &["--duration", "5", "--json", "--name", "x"],
        ] {
            let args = parsed(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
            assert_eq!(args.mode(), super::RecordMode::Recorder, "{argv:?}");
        }
    }

    #[test]
    fn recorder_flags_conflict_with_output_and_clipboard() {
        for recorder in [
            &["--quality", "lossless"][..],
            &["--dir", "d"],
            &["--name", "n"],
            &["--transcribe"],
        ] {
            for legacy in [&["-o", "x.wav"][..], &["--clipboard"]] {
                let mut argv = recorder.to_vec();
                argv.extend_from_slice(legacy);
                let error = parsed(&argv).err().unwrap_or_else(|| panic!("{argv:?}"));
                assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
            }
        }
        assert!(parsed(&["--quality", "mp3"]).is_err());
    }

    #[test]
    fn two_profiles_keep_independent_record_engines_and_explicit_override() {
        let mut settings = Settings::default();
        let swedish = HotkeyProfile::legacy_default("Super+S".into(), Language::Swedish);
        let mut english = HotkeyProfile::legacy_default("Super+E".into(), Language::English);
        english.id = "english".into();
        english.name = "English".into();
        settings.hotkey_profiles = vec![swedish.clone(), english.clone()];
        settings.profile_models.insert(
            "default".into(),
            FileModelPreference::Pianissimo(PianissimoModel::Swedish),
        );
        settings.profile_models.insert(
            "english".into(),
            FileModelPreference::Whisper(WhisperModel::SmallEn),
        );
        assert_eq!(
            resolve_record_model_for_profile(&settings, &swedish, Language::Swedish, None).unwrap(),
            RecordModel::Pianissimo(PianissimoModel::Swedish)
        );
        assert_eq!(
            resolve_record_model_for_profile(&settings, &english, Language::English, None).unwrap(),
            RecordModel::Whisper(WhisperModel::SmallEn)
        );
        assert_eq!(
            resolve_record_model_for_profile(
                &settings,
                &swedish,
                Language::Swedish,
                Some("kb-whisper-base")
            )
            .unwrap(),
            RecordModel::Whisper(WhisperModel::KbWhisperBase)
        );
    }

    #[test]
    fn pianissimo_silence_skips_native_model_in_record() {
        for audio in [vec![0.0; 1600], vec![0.00001; 1600]] {
            assert_eq!(
                super::transcribe_pianissimo_record(
                    &audio,
                    0.1,
                    PianissimoModel::Swedish,
                    &Out::capture().0
                )
                .unwrap(),
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
            RecordModel::Pianissimo(PianissimoModel::Swedish)
        );
    }

    #[test]
    fn explicit_pianissimo_rejects_non_swedish_language() {
        for language in [
            Language::English,
            Language::Norwegian,
            Language::Finnish,
            Language::Danish,
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
            RecordModel::Pianissimo(PianissimoModel::Swedish)
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
            let (out, captured) = Out::capture();
            assert!(!write_plain_record_output(&out, text).unwrap());
            assert!(captured.stdout().is_empty());
            assert!(!out.stdout_used());
        }
    }

    #[test]
    fn nonempty_plain_record_output_keeps_text_and_newline() {
        let (out, captured) = Out::capture();
        assert!(write_plain_record_output(&out, "hello").unwrap());
        assert_eq!(captured.stdout(), "hello\n");
    }
}
