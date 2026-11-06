//! Recorder mode of `sagascript record` (SAGA-564, design §4.7): the microphone
//! becomes a recording folder (FLAC tracks and a manifest) instead of audio in
//! memory.
//!
//! Recording is never silent: a start banner, a status line every ten seconds
//! and a closing summary go to stderr. stdout carries the result only: the
//! folder path, one JSON object with `--json`, or, with `--transcribe`, the
//! text (the folder is then on stderr and in the JSON object).
//!
//! The source is the default microphone, or, in a development build, the
//! synthetic source `SAGASCRIPT_TEST_SOURCE=synthetic` asks for (no hardware;
//! `capture::test_source_requested`). Paths are printed but never logged
//! through `tracing`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sagascript_core::audio::permission::{microphone_permission, MicrophonePermissionStatus};
use sagascript_core::error::DictationError;
use sagascript_core::recording::capture::{
    test_source_requested, CaptureSource, MicConfig, MicSource, SyntheticConfig,
};
use sagascript_core::recording::{
    decode_tracks, summarize_folder, Client, Quality, RecordingSession, RecordingStore,
    SessionConfig, SessionOutcome, SessionStatus, SessionWarning, SourceSpec, SyntheticSource,
};
use sagascript_core::settings::{RecordingQuality, Settings};

use crate::output::{errln, outln, Out};
use crate::record::{
    ctrlc_handler, prepare, transcribe_samples, write_plain_record_output, Prepared, RecordArgs,
};
use crate::recording_text::{
    ended_early_text, ended_reason_id, format_bytes, format_hms, format_level, quality_id,
    warning_text,
};

/// Shown in the banner of every recording.
pub(crate) const CONSENT_REMINDER: &str =
    "Recording other people may need their consent; a microphone picks up the whole room.";

/// The title of a recording made without `--name`.
pub(crate) const DEFAULT_TITLE: &str = "Recording";

/// How often the status line is printed.
pub(crate) const STATUS_INTERVAL: Duration = Duration::from_secs(10);

/// What a recorder-mode run was asked for, checked before anything is opened.
#[derive(Debug, PartialEq)]
pub(crate) struct RecorderOptions {
    pub quality: RecordingQuality,
    pub dir: PathBuf,
    pub title: String,
    pub duration: Option<Duration>,
    pub transcribe: bool,
    pub json: bool,
}

impl RecorderOptions {
    pub(crate) fn from_args(args: &RecordArgs, stored: &Settings) -> Result<Self, DictationError> {
        let quality = match args.quality.as_deref() {
            Some(id) => RecordingQuality::parse_id(id).map_err(DictationError::from)?,
            None => stored.recording_quality,
        };
        let title = match args.name.as_deref().map(str::trim) {
            Some("") => {
                return Err(DictationError::SettingsError(
                    "--name must not be empty".into(),
                ))
            }
            Some(name) => name.to_string(),
            None => DEFAULT_TITLE.to_string(),
        };
        let duration = match args.duration {
            None => None,
            Some(seconds) => Some(
                Duration::try_from_secs_f64(seconds)
                    .ok()
                    .filter(|duration| !duration.is_zero())
                    .ok_or_else(|| {
                        DictationError::SettingsError(
                            "--duration must be a positive number of seconds".into(),
                        )
                    })?,
            ),
        };
        if !args.transcribe {
            let transcription_flag = [
                (args.language.is_some(), "--language"),
                (args.profile.is_some(), "--profile"),
                (args.model.is_some(), "--model"),
                (args.prompt.is_some(), "--prompt/--hint"),
                (args.prompt_file.is_some(), "--prompt-file"),
            ]
            .into_iter()
            .find_map(|(given, flag)| given.then_some(flag));
            if let Some(flag) = transcription_flag {
                return Err(DictationError::SettingsError(format!(
                    "{flag} only applies with --transcribe"
                )));
            }
        }
        Ok(Self {
            quality,
            dir: args.dir.clone().unwrap_or_else(|| stored.recordings_dir()),
            title,
            duration,
            transcribe: args.transcribe,
            json: args.json,
        })
    }
}

/// The source to record: the synthetic one when a development build was asked
/// for it, else whatever `open_mic` opens. `open_mic` is never called when a
/// synthetic source is requested, so a test environment cannot reach a device.
pub(crate) fn choose_source(
    requested: Option<SyntheticConfig>,
    open_mic: impl FnOnce() -> Result<Box<dyn CaptureSource>, DictationError>,
) -> Result<Box<dyn CaptureSource>, DictationError> {
    match requested {
        Some(config) => Ok(Box::new(SyntheticSource::new(config))),
        None => open_mic(),
    }
}

fn open_mic(channel_pair: (u16, u16), out: &Out) -> Result<Box<dyn CaptureSource>, DictationError> {
    match microphone_permission().status {
        status if status.capture_denied() => return Err(DictationError::MicrophonePermissionDenied),
        MicrophonePermissionStatus::NotDetermined => errln!(
            out,
            "Microphone permission has not been decided yet; macOS may ask now. If recording does not start, allow it and run the command again."
        ),
        _ => {}
    }
    let source = MicSource::open(MicConfig { channel_pair })?;
    Ok(Box::new(source))
}

/// The client that wrote the recording (manifest `client`).
fn client() -> Client {
    Client {
        kind: "cli".into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        build: crate::GIT_HASH.into(),
        extra: Default::default(),
    }
}

/// RFC 3339 with the local offset and milliseconds.
fn started_at_now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

/// The stored shape of the microphone track, in words.
pub(crate) fn describe_tracks(
    quality: Quality,
    sample_rate: u32,
    channels: u8,
    pair: (u16, u16),
) -> String {
    let layout = match (channels, pair) {
        (2, (a, b)) => format!("stereo, device channels {a}-{b}"),
        (_, (a, b)) if a == b => format!("mono, device channel {a}"),
        _ => "mono".to_string(),
    };
    match quality {
        Quality::Lossless => {
            format!("{layout}, {sample_rate} Hz, 24-bit FLAC (as the device delivers it)")
        }
        Quality::Compact => {
            format!("{layout}, 16000 Hz, 16-bit FLAC (resampled from {sample_rate} Hz)")
        }
    }
}

/// The lines printed when recording starts.
pub(crate) fn banner_lines(
    folder: &Path,
    quality: Quality,
    device: Option<&str>,
    tracks: &str,
    duration: Option<Duration>,
) -> Vec<String> {
    let mut lines = vec![
        format!("Recording to {}", folder.display()),
        format!("Quality: {} ({tracks})", quality_id(quality)),
        format!("Device: {}", device.unwrap_or("unknown")),
    ];
    lines.push(match duration {
        Some(duration) => format!(
            "Recording for {} (press Ctrl+C to stop early).",
            format_hms(duration.as_secs_f64())
        ),
        None => "Press Ctrl+C to stop.".to_string(),
    });
    lines.push(CONSENT_REMINDER.to_string());
    lines
}

/// The periodic status line: elapsed time, level, gaps, free disk.
pub(crate) fn status_line(status: &SessionStatus) -> String {
    let peak = status
        .tracks
        .iter()
        .map(|track| track.peak)
        .fold(0.0_f32, f32::max);
    let gaps: u32 = status.tracks.iter().map(|track| track.gap_count).sum();
    let free = status
        .free_disk_bytes
        .map_or_else(|| "unknown".to_string(), format_bytes);
    let mut line = format!(
        "[{}] level {}, gaps {gaps}, free disk {free}, written {}",
        format_hms(status.elapsed.as_secs_f64()),
        format_level(peak),
        format_bytes(status.bytes_written),
    );
    if status.tracks.iter().any(|track| track.device_lost) {
        line.push_str(", input device lost");
    } else if status.tracks.iter().any(|track| track.stalled) {
        line.push_str(", input stalled");
    }
    line
}

/// The one JSON object of a finished recording (`--json`).
pub(crate) fn result_json(
    summary: &sagascript_core::recording::RecordingSummary,
    folder: &Path,
    ended_early: Option<&sagascript_core::recording::EndedReason>,
    transcript: Option<&str>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "recording_id": summary.recording_id,
        "folder": folder,
        "title": summary.title,
        "quality": summary.quality,
        "duration_seconds": summary.duration_seconds,
        "bytes": summary.bytes,
        "track_count": summary.track_count,
        "gap_count": summary.gap_count,
        "ended_early": ended_early.map(ended_reason_id),
        "recovered": false,
    });
    if let (Some(transcript), Some(object)) = (transcript, value.as_object_mut()) {
        object.insert("transcript".into(), transcript.into());
    }
    value
}

/// The closing summary on stderr.
pub(crate) fn summary_line(
    summary: &sagascript_core::recording::RecordingSummary,
    folder: &Path,
) -> String {
    format!(
        "Saved {} of {} audio ({}, {} gap{}) to {}",
        format_hms(summary.duration_seconds),
        quality_id(summary.quality),
        format_bytes(summary.bytes),
        summary.gap_count,
        if summary.gap_count == 1 { "" } else { "s" },
        folder.display()
    )
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn run(args: RecordArgs, out: &Out) -> Result<(), DictationError> {
    let stored = sagascript_core::machine::load();
    let options = RecorderOptions::from_args(&args, &stored)?;
    // Fail on a missing model before the microphone is touched.
    let prepared = if options.transcribe {
        Some(prepare(&args, &stored, false)?)
    } else {
        None
    };

    let pair = stored.recording_channel_pair_tuple();
    let source = choose_source(test_source_requested(), || open_mic(pair, out))?;
    let device = source.device_name();
    let format = source.format();

    let quality = options.quality.to_quality();
    let config = SessionConfig::new(&options.title, started_at_now(), client(), quality);
    let session = RecordingSession::new(RecordingStore::new(&options.dir), config);
    errln!(out, "Opening the microphone...");
    session.start(vec![SourceSpec {
        id: "mic".into(),
        role: "mic".into(),
        source,
    }])?;

    let folder = session.folder().map(|folder| absolute(&folder));
    if let Some(folder) = &folder {
        let tracks = describe_tracks(quality, format.sample_rate, format.channels, pair);
        for line in banner_lines(
            folder,
            quality,
            device.as_deref(),
            &tracks,
            options.duration,
        ) {
            errln!(out, "{line}");
        }
    }

    // Ctrl+C and SIGTERM stop and finish the recording; they do not cancel it.
    let running = Arc::new(AtomicBool::new(true));
    ctrlc_handler(running.clone());
    wait_for_stop(&session, &running, options.duration, out);

    errln!(out, "Stopping and finishing the recording...");
    let outcome = session.stop().map_err(|error| {
        if let Some(folder) = &folder {
            errln!(
                out,
                "The recording in {} was not finished; run 'sagascript recordings recover' to keep what was saved.",
                folder.display()
            );
        }
        DictationError::from(error)
    })?;
    finish(&options, prepared, outcome, out)
}

/// Print the status line and new warnings until the user or `--duration` stops
/// the recording, or the session ends by itself.
fn wait_for_stop(
    session: &RecordingSession,
    running: &AtomicBool,
    duration: Option<Duration>,
    out: &Out,
) {
    let began = Instant::now();
    let mut next_status = began + STATUS_INTERVAL;
    let mut warned: Vec<SessionWarning> = Vec::new();
    let mut print_new_warnings = |status: &SessionStatus| {
        for warning in &status.warnings {
            if !warned.contains(warning) {
                errln!(out, "Warning: {}", warning_text(warning));
                warned.push(warning.clone());
            }
        }
    };
    print_new_warnings(&session.status());
    loop {
        std::thread::sleep(Duration::from_millis(50));
        if !running.load(Ordering::Relaxed) {
            break;
        }
        if duration.is_some_and(|limit| began.elapsed() >= limit) {
            break;
        }
        let status = session.status();
        if status.state.is_terminal() {
            break;
        }
        print_new_warnings(&status);
        if Instant::now() >= next_status {
            errln!(out, "{}", status_line(&status));
            next_status += STATUS_INTERVAL;
        }
    }
}

fn finish(
    options: &RecorderOptions,
    prepared: Option<Prepared>,
    outcome: SessionOutcome,
    out: &Out,
) -> Result<(), DictationError> {
    let folder = absolute(&outcome.folder);
    let summary = summarize_folder(&outcome.folder)?;
    errln!(out, "{}", summary_line(&summary, &folder));
    if let Some(reason) = &outcome.ended_early {
        errln!(
            out,
            "The recording ended early: {}. What was recorded is saved.",
            ended_early_text(reason)
        );
    }

    let transcript = match prepared {
        Some(prepared) => Some(transcribe_recording(
            &outcome.folder,
            prepared,
            &folder,
            out,
        )?),
        None => None,
    };
    let text = transcript
        .as_ref()
        .map(|transcript| transcript.text.as_str());

    if options.json {
        out.json(&result_json(
            &summary,
            &folder,
            outcome.ended_early.as_ref(),
            text,
        ))?;
    } else if let Some(text) = text {
        // The text is the result of a transcribing run; the folder is above.
        if !write_plain_record_output(out, text)? {
            errln!(out, "No speech was recognised.");
        }
    } else {
        outln!(out, "{}", folder.display())?;
    }
    Ok(())
}

/// Hand the finished recording to the file transcription: the microphone track
/// is decoded to 16 kHz in chunks and joined (a bounded `Vec`, as the
/// transcription entry points take a slice) and goes through the engine that
/// was checked before capture started.
fn transcribe_recording(
    folder: &Path,
    prepared: Prepared,
    display_folder: &Path,
    out: &Out,
) -> Result<crate::record::Transcript, DictationError> {
    let saved = |error: DictationError| {
        errln!(
            out,
            "The recording is saved in {}; transcribe it later.",
            display_folder.display()
        );
        error
    };
    let model = prepared
        .model
        .expect("the transcription model is resolved before capture");
    let samples = mic_samples(folder).map_err(saved)?;
    if samples.is_empty() {
        return Err(saved(DictationError::NoAudioCaptured));
    }
    let seconds =
        samples.len() as f64 / f64::from(sagascript_core::audio::resample::TARGET_SAMPLE_RATE);
    transcribe_samples(
        &samples,
        seconds,
        model,
        prepared.language,
        &prepared.glossary,
        out,
    )
    .map_err(saved)
}

/// The microphone track as one 16 kHz mono buffer, chunks placed by their
/// `start_sample` (a hole before a chunk is silence).
fn mic_samples(folder: &Path) -> Result<Vec<f32>, DictationError> {
    let mut samples: Vec<f32> = Vec::new();
    for chunk in decode_tracks(folder)? {
        let chunk = chunk?;
        if chunk.role != "mic" {
            continue;
        }
        place_chunk(&mut samples, chunk.start_sample, &chunk.samples)?;
    }
    Ok(samples)
}

/// Put `chunk` at `start` in `samples`, padding a hole with silence.
pub(crate) fn place_chunk(
    samples: &mut Vec<f32>,
    start: u64,
    chunk: &[f32],
) -> Result<(), DictationError> {
    let start = usize::try_from(start).map_err(|_| {
        DictationError::FileDecodeError("the recording is too long to transcribe".into())
    })?;
    if start < samples.len() {
        return Err(DictationError::FileDecodeError(
            "the recording's chunks overlap".into(),
        ));
    }
    samples.resize(start, 0.0);
    samples.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cli;
    use clap::Parser;
    use sagascript_core::recording::{SessionState, TrackStatus};

    fn record_args(extra: &[&str]) -> RecordArgs {
        let mut argv = vec!["sagascript", "record"];
        argv.extend_from_slice(extra);
        match Cli::try_parse_from(argv).unwrap().command.unwrap() {
            crate::Command::Record(args) => args,
            _ => panic!("expected record"),
        }
    }

    fn options(extra: &[&str]) -> Result<RecorderOptions, DictationError> {
        RecorderOptions::from_args(&record_args(extra), &Settings::default())
    }

    #[test]
    fn defaults_come_from_the_settings() {
        let settings = Settings {
            recording_quality: RecordingQuality::Compact,
            recording_directory: "/tmp/takes".into(),
            ..Settings::default()
        };
        let options =
            RecorderOptions::from_args(&record_args(&["--name", "x"]), &settings).unwrap();
        assert_eq!(options.quality, RecordingQuality::Compact);
        assert_eq!(options.dir, PathBuf::from("/tmp/takes"));
        assert_eq!(options.title, "x");
    }

    #[test]
    fn flags_override_the_settings_and_the_title_defaults() {
        let options =
            options(&["--quality", "compact", "--dir", "rec", "--duration", "2.5"]).unwrap();
        assert_eq!(options.quality, RecordingQuality::Compact);
        assert_eq!(options.dir, PathBuf::from("rec"));
        assert_eq!(options.title, DEFAULT_TITLE);
        assert_eq!(options.duration, Some(Duration::from_millis(2500)));
        assert!(!options.transcribe && !options.json);
    }

    #[test]
    fn a_bad_duration_or_name_is_refused_with_the_flag_named() {
        for bad in ["0", "-1", "nan", "inf", "1e30"] {
            let flag = format!("--duration={bad}");
            let error = options(&["--name", "x", flag.as_str()]).unwrap_err();
            assert_eq!(error.code(), "settings_error", "{bad}");
            assert!(error.to_string().contains("--duration"), "{bad}");
        }
        let error = options(&["--name", "   "]).unwrap_err();
        assert!(error.to_string().contains("--name"));
    }

    #[test]
    fn transcription_flags_need_transcribe_in_recorder_mode() {
        for flags in [
            &["--language", "sv"][..],
            &["--model", "base.en"],
            &["--profile", "default"],
            &["--prompt", "x"],
        ] {
            let mut argv = vec!["--name", "x"];
            argv.extend_from_slice(flags);
            let error = options(&argv).unwrap_err();
            assert!(
                error.to_string().contains("only applies with --transcribe"),
                "{flags:?}: {error}"
            );
        }
        assert!(options(&["--transcribe", "--language", "sv"]).is_ok());
    }

    #[test]
    fn a_synthetic_request_never_opens_the_microphone() {
        let source = choose_source(Some(SyntheticConfig::default()), || {
            panic!("the microphone must not be opened when a synthetic source is requested")
        })
        .unwrap();
        assert_eq!(
            source.device_name().as_deref(),
            Some(sagascript_core::recording::SYNTHETIC_DEVICE_NAME)
        );
    }

    #[test]
    fn without_a_request_the_microphone_opener_runs_and_its_error_is_kept() {
        let result = choose_source(None, || {
            Err(DictationError::Recording(
                sagascript_core::recording::RecordingError::SourceUnavailable {
                    reason: "no input device",
                },
            ))
        });
        assert_eq!(
            result.err().map(|error| error.code().to_string()),
            Some("recording_source_unavailable".to_string())
        );
    }

    fn track(peak: f32, gaps: u32) -> TrackStatus {
        TrackStatus {
            id: "mic".into(),
            role: "mic".into(),
            peak,
            channel_peaks: [peak, 0.0],
            frames: 0,
            frames_received: 0,
            gap_count: gaps,
            gap_frames: 0,
            dropped_frames: 0,
            stalled: false,
            device_lost: false,
            ended: None,
            queued_blocks: 0,
        }
    }

    fn status(tracks: Vec<TrackStatus>) -> SessionStatus {
        SessionStatus {
            state: SessionState::Recording,
            elapsed: Duration::from_millis(3_723_400),
            tracks,
            bytes_written: 2_500_000,
            free_disk_bytes: Some(85_200_000_000),
            warnings: Vec::new(),
            error_code: None,
            ended_early: None,
        }
    }

    #[test]
    fn the_status_line_shows_elapsed_level_gaps_and_free_disk() {
        assert_eq!(
            status_line(&status(vec![track(0.5, 2)])),
            "[1:02:03] level -6 dB, gaps 2, free disk 85.2 GB, written 2.5 MB"
        );
        let mut unknown = status(vec![track(0.0, 0)]);
        unknown.free_disk_bytes = None;
        unknown.tracks[0].stalled = true;
        assert_eq!(
            status_line(&unknown),
            "[1:02:03] level -inf dB, gaps 0, free disk unknown, written 2.5 MB, input stalled"
        );
        unknown.tracks[0].device_lost = true;
        assert!(status_line(&unknown).ends_with("input device lost"));
    }

    #[test]
    fn the_banner_names_folder_quality_device_and_the_consent_reminder() {
        let lines = banner_lines(
            Path::new("/rec/2026 Talk"),
            Quality::Compact,
            Some("Mic"),
            &describe_tracks(Quality::Compact, 48_000, 1, (1, 2)),
            Some(Duration::from_secs(90)),
        );
        let text = lines.join("\n");
        assert!(text.contains("Recording to /rec/2026 Talk"), "{text}");
        assert!(text
            .contains("Quality: compact (mono, 16000 Hz, 16-bit FLAC (resampled from 48000 Hz))"));
        assert!(text.contains("Device: Mic"));
        assert!(text.contains("Recording for 0:01:30"));
        assert!(text.contains(CONSENT_REMINDER));
        let open_ended = banner_lines(Path::new("/r"), Quality::Lossless, None, "t", None);
        assert!(open_ended.iter().any(|l| l == "Press Ctrl+C to stop."));
        assert!(open_ended.iter().any(|l| l == "Device: unknown"));
    }

    #[test]
    fn channel_pairs_are_described() {
        assert!(describe_tracks(Quality::Lossless, 48_000, 2, (3, 4))
            .starts_with("stereo, device channels 3-4, 48000 Hz, 24-bit FLAC"));
        assert!(describe_tracks(Quality::Lossless, 44_100, 1, (3, 3))
            .starts_with("mono, device channel 3, 44100 Hz"));
        assert!(describe_tracks(Quality::Lossless, 44_100, 1, (1, 2)).starts_with("mono, 44100 Hz"));
    }

    fn summary() -> sagascript_core::recording::RecordingSummary {
        sagascript_core::recording::RecordingSummary {
            folder_name: "2026-11-05 14.02 Talk [a1b2c3]".into(),
            recording_id: "a1b2c3d4-0000-4000-8000-000000000000".into(),
            title: "Talk".into(),
            state: sagascript_core::recording::RecordingState::Complete,
            recovered: false,
            started_at: "2026-11-05T14:02:11.482+01:00".into(),
            quality: Quality::Lossless,
            duration_seconds: 61.5,
            bytes: 4_000_000,
            track_count: 1,
            gap_count: 1,
            gap_seconds: 0.2,
            ended_reason: Some("stopped".into()),
            codec: "flac".into(),
            sample_rate: 48_000,
            channels: 1,
            needs_recovery: false,
            playable: true,
        }
    }

    #[test]
    fn the_json_object_has_the_documented_keys() {
        let value = result_json(&summary(), Path::new("/rec/Talk"), None, None);
        let object = value.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "bytes",
                "duration_seconds",
                "ended_early",
                "folder",
                "gap_count",
                "quality",
                "recording_id",
                "recovered",
                "title",
                "track_count"
            ]
        );
        assert_eq!(value["folder"], "/rec/Talk");
        assert_eq!(value["quality"], "lossless");
        assert_eq!(value["recovered"], false);
        assert!(value["ended_early"].is_null());

        let with = result_json(
            &summary(),
            Path::new("/rec/Talk"),
            Some(&sagascript_core::recording::EndedReason::DeviceChanged),
            Some("hello"),
        );
        assert_eq!(with["transcript"], "hello");
        assert_eq!(with["ended_early"], "device_changed");
    }

    #[test]
    fn the_summary_line_pluralises_gaps() {
        let line = summary_line(&summary(), Path::new("/rec/Talk"));
        assert_eq!(
            line,
            "Saved 0:01:01 of lossless audio (4.0 MB, 1 gap) to /rec/Talk"
        );
        let mut many = summary();
        many.gap_count = 3;
        assert!(summary_line(&many, Path::new("/r")).contains("3 gaps"));
    }

    /// What `--transcribe` hands to the engine: the saved microphone track,
    /// decoded to 16 kHz in chunks and joined.
    #[test]
    fn the_saved_microphone_track_is_decoded_to_sixteen_khz_samples() {
        use sagascript_core::recording::{EndedReason, NewRecording, NewTrack};
        let temp = sagascript_testsupport::TempDir::new();
        let mut writer = RecordingStore::new(temp.path())
            .create(NewRecording {
                title: "t".into(),
                started_at: "2026-11-05T14:02:11.482+01:00".into(),
                client: client(),
                quality: Quality::Lossless,
                tracks: vec![NewTrack {
                    id: "mic".into(),
                    role: "mic".into(),
                    sample_rate: 48_000,
                    channels: 1,
                    bits: 24,
                    clock_domain: "test".into(),
                    start_ns: 1,
                    resampled_from: None,
                    device: None,
                }],
            })
            .unwrap();
        let tone: Vec<f32> = (0..96_000)
            .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        writer.push("mic", &tone).unwrap();
        let folder = writer.folder().to_path_buf();
        writer.finish(EndedReason::Stopped).unwrap();

        let samples = mic_samples(&folder).unwrap();
        // Two seconds at 16 kHz, give or take the resampler's edge.
        assert!(
            (31_000..=33_000).contains(&samples.len()),
            "{}",
            samples.len()
        );
        let peak = samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!((0.4..=0.6).contains(&peak), "peak {peak}");
    }

    #[test]
    fn a_recording_that_is_not_finished_is_not_transcribed() {
        let temp = sagascript_testsupport::TempDir::new();
        let error = mic_samples(temp.path()).unwrap_err();
        assert!(error.code().starts_with("recording_"), "{error}");
    }

    #[test]
    fn chunks_are_placed_by_start_sample_and_holes_are_silence() {
        let mut samples = Vec::new();
        place_chunk(&mut samples, 0, &[1.0, 1.0]).unwrap();
        place_chunk(&mut samples, 2, &[2.0]).unwrap();
        place_chunk(&mut samples, 5, &[3.0]).unwrap();
        assert_eq!(samples, [1.0, 1.0, 2.0, 0.0, 0.0, 3.0]);
        assert!(place_chunk(&mut samples, 3, &[9.0]).is_err());
    }
}
