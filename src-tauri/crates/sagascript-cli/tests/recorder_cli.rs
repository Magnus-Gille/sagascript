//! `sagascript record` in recorder mode and `sagascript recordings` through the
//! real binary (SAGA-564).
//!
//! No test opens a real microphone: every spawn goes through [`cli`], which
//! sets `SAGASCRIPT_TEST_SOURCE=synthetic` (the development seam of
//! `sagascript_core::recording::capture::test_source_requested`) and refuses to
//! run in a build that would ignore it. Settings live in a temporary file and
//! recordings in a temporary `--dir` (ADR 0004).

#![cfg(feature = "record")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use sagascript_core::recording::capture::TEST_SOURCE_ENV;
use sagascript_core::recording::{
    Client, NewRecording, NewTrack, Quality, RecordingStore, RecordingWriter,
};
use sagascript_testsupport::{fake_settings, isolated_cli_env, FakeSettings, TempDir};
use serde_json::Value;

/// The synthetic source is honoured only by a development build; in any other
/// build the same command would open the real microphone.
fn seam_is_honoured() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "dev-overrides")
}

/// A `sagascript` command that can only record from the synthetic source.
fn cli(settings: &FakeSettings, args: &[&str]) -> Command {
    assert!(
        seam_is_honoured(),
        "the synthetic source seam is off in this build; refusing to risk the real microphone"
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    isolated_cli_env(&mut command, settings.path());
    command.env(TEST_SOURCE_ENV, "synthetic");
    let seam = command
        .get_envs()
        .find(|(name, _)| *name == TEST_SOURCE_ENV)
        .and_then(|(_, value)| value);
    assert_eq!(seam, Some(std::ffi::OsStr::new("synthetic")));
    command.args(args);
    command
}

fn run(settings: &FakeSettings, args: &[&str]) -> Output {
    cli(settings, args).output().expect("run sagascript")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn stdout(output: &Output) -> String {
    text(&output.stdout)
}

fn stderr(output: &Output) -> String {
    text(&output.stderr)
}

fn ok(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout is not JSON ({error}): {}", stdout(output)))
}

fn s(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}

fn manifest(folder: &Path) -> Value {
    let bytes = std::fs::read(folder.join("recording.json")).expect("read the manifest");
    serde_json::from_slice(&bytes).expect("the manifest is JSON")
}

/// Record one second and return the folder (from the plain stdout).
fn record_one(settings: &FakeSettings, dir: &Path, extra: &[&str]) -> PathBuf {
    let mut args = vec!["record", "--duration", "1", "--dir", s(dir)];
    args.extend_from_slice(extra);
    let output = run(settings, &args);
    ok(&output);
    PathBuf::from(stdout(&output).trim())
}

// -- record ------------------------------------------------------------------

#[test]
fn a_lossless_recording_is_a_complete_folder_with_banner_and_summary() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let output = run(
        &settings,
        &[
            "record",
            "--quality",
            "lossless",
            "--dir",
            s(&dir),
            "--duration",
            "1",
            "--name",
            "Team meeting",
        ],
    );
    ok(&output);

    let folder = PathBuf::from(stdout(&output).trim());
    assert!(folder.is_absolute(), "{folder:?}");
    assert!(folder.is_dir());
    assert_eq!(
        folder.parent().map(Path::to_path_buf),
        Some(dir.path().to_path_buf())
    );
    assert_eq!(
        stdout(&output).lines().count(),
        1,
        "stdout is the path alone"
    );
    assert!(folder.join("mic.flac").is_file());

    let manifest = manifest(&folder);
    assert_eq!(manifest["state"], "complete");
    assert_eq!(manifest["title"], "Team meeting");
    assert_eq!(manifest["quality"], "lossless");
    assert_eq!(manifest["client"]["kind"], "cli");
    let tracks = manifest["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0]["id"], "mic");
    assert_eq!(tracks[0]["codec"], "flac");
    assert_eq!(tracks[0]["sample_rate"], 48_000);
    assert_eq!(tracks[0]["bits"], 24);
    let frames = tracks[0]["frames"].as_u64().unwrap();
    assert!((40_000..=70_000).contains(&frames), "{frames} frames");

    let err = stderr(&output);
    assert!(err.contains("Recording to "), "{err}");
    assert!(err.contains("Quality: lossless"), "{err}");
    assert!(err.contains("Device: Synthetic microphone"), "{err}");
    assert!(
        err.contains(
            "Recording other people may need their consent; a microphone picks up the whole room."
        ),
        "{err}"
    );
    assert!(err.contains("Saved 0:00:0"), "{err}");
    assert!(err.contains("of lossless audio"), "{err}");
}

#[test]
fn a_compact_recording_reports_one_json_object() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let output = run(
        &settings,
        &[
            "record",
            "--quality",
            "compact",
            "--dir",
            s(&dir),
            "--duration",
            "1",
            "--json",
        ],
    );
    ok(&output);
    let report = json(&output);
    let folder = PathBuf::from(report["folder"].as_str().unwrap());
    assert!(folder.is_absolute() && folder.is_dir());

    let mut keys: Vec<&str> = report
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
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
    assert_eq!(report["quality"], "compact");
    assert_eq!(report["title"], "Recording");
    assert_eq!(report["track_count"], 1);
    assert_eq!(report["gap_count"], 0);
    assert_eq!(report["recovered"], false);
    assert!(report["ended_early"].is_null());
    assert!(report["bytes"].as_u64().unwrap() > 0);
    let seconds = report["duration_seconds"].as_f64().unwrap();
    assert!((0.8..=2.0).contains(&seconds), "{seconds}");

    let manifest = manifest(&folder);
    assert_eq!(manifest["recording_id"], report["recording_id"]);
    assert_eq!(manifest["tracks"][0]["sample_rate"], 16_000);
    assert_eq!(manifest["tracks"][0]["bits"], 16);
    assert_eq!(manifest["tracks"][0]["resampled_from"], 48_000);
    assert!(stderr(&output).contains("Quality: compact"));
}

#[test]
fn the_name_is_a_title_and_a_sanitised_folder_name() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let folder = record_one(&settings, &dir, &["--name", "a/b:c*d?e \"f\"<g>|h"]);
    let name = folder.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(folder.parent().unwrap(), dir.path());
    for forbidden in ['/', '\\', ':', '*', '?', '"', '<', '>', '|'] {
        assert!(!name.contains(forbidden), "{name:?} holds {forbidden:?}");
    }
    assert_eq!(manifest(&folder)["title"], "a/b:c*d?e \"f\"<g>|h");

    let default = record_one(&settings, &dir, &["--quality", "lossless"]);
    assert_eq!(manifest(&default)["title"], "Recording");
    assert_ne!(default, folder);
}

#[test]
fn the_settings_supply_quality_and_folder() {
    let settings = fake_settings();
    let dir = TempDir::new();
    for (key, value) in [
        ("recording_quality", "compact"),
        ("recording_directory", s(&dir)),
    ] {
        ok(&run(&settings, &["config", "set", key, value]));
    }
    let output = run(
        &settings,
        &["record", "--duration", "1", "--name", "Configured"],
    );
    ok(&output);
    let folder = PathBuf::from(stdout(&output).trim());
    assert_eq!(folder.parent().unwrap(), dir.path());
    assert_eq!(manifest(&folder)["quality"], "compact");
}

#[cfg(unix)]
#[test]
fn sigterm_stops_and_finishes_the_recording() {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::time::Duration;

    let settings = fake_settings();
    let dir = TempDir::new();
    let mut child = cli(&settings, &["record", "--dir", s(&dir), "--name", "Signal"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sagascript");
    let pid = child.id().to_string();
    let stderr_pipe = child.stderr.take().unwrap();
    let (lines_tx, lines_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        for line in BufReader::new(stderr_pipe).lines().map_while(Result::ok) {
            all.push_str(&line);
            all.push('\n');
            let _ = lines_tx.send(line);
        }
        all
    });

    // The banner is printed once the recording runs.
    let mut seen_banner = false;
    while let Ok(line) = lines_rx.recv_timeout(Duration::from_secs(20)) {
        if line.contains("Press Ctrl+C to stop.") {
            seen_banner = true;
            break;
        }
    }
    if !seen_banner {
        let _ = child.kill();
        panic!("no start banner on stderr");
    }
    std::thread::sleep(Duration::from_millis(1200));
    let kill = Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .expect("run kill");
    assert!(kill.success());

    let output = child.wait_with_output().expect("wait for sagascript");
    let err = reader.join().unwrap();
    assert_eq!(output.status.code(), Some(0), "stderr: {err}");
    assert!(
        err.contains("Stopping and finishing the recording"),
        "{err}"
    );
    assert!(err.contains("Saved 0:00:0"), "{err}");
    let folder = PathBuf::from(stdout(&output).trim());
    assert_eq!(manifest(&folder)["state"], "complete");
    assert_eq!(manifest(&folder)["tracks"][0]["ended_reason"], "stopped");
}

#[test]
fn bad_recorder_arguments_fail_before_anything_is_recorded() {
    let settings = fake_settings();
    let dir = TempDir::new();

    // clap: recorder flags and the old outputs exclude each other.
    for conflicting in [
        ["--dir", s(&dir), "-o", "x.wav"],
        ["--name", "x", "--clipboard", ""],
    ] {
        let args: Vec<&str> = ["record"]
            .into_iter()
            .chain(conflicting.into_iter().filter(|a| !a.is_empty()))
            .collect();
        let output = run(&settings, &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            stderr(&output)
        );
    }

    // A duration of zero is refused; with --json the error object is the only stdout.
    let output = run(
        &settings,
        &["record", "--dir", s(&dir), "--duration=0", "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    let error = json(&output);
    assert_eq!(error["status"], "error");
    assert_eq!(error["code"], "settings_error");

    // A transcription flag without --transcribe is not silently ignored.
    let output = run(
        &settings,
        &[
            "record",
            "--dir",
            s(&dir),
            "--name",
            "x",
            "--language",
            "sv",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("only applies with --transcribe"));

    // An unknown model stops --transcribe before the recording starts.
    let output = run(
        &settings,
        &[
            "record",
            "--dir",
            s(&dir),
            "--transcribe",
            "--model",
            "not-a-model",
            "--duration",
            "1",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Unknown model"),
        "{}",
        stderr(&output)
    );
    assert!(!stderr(&output).contains("Recording to"));

    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "nothing was created"
    );
}

#[test]
fn a_recording_failure_keeps_its_recording_code_in_json() {
    let settings = fake_settings();
    let scratch = TempDir::new();
    // A file where the recordings folder should be.
    let file = scratch.path().join("not-a-folder");
    std::fs::write(&file, b"x").unwrap();
    let output = run(
        &settings,
        &["record", "--dir", s(&file), "--duration", "1", "--json"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let error = json(&output);
    assert_eq!(error["status"], "error");
    assert!(
        error["code"].as_str().unwrap().starts_with("recording_"),
        "{error}"
    );
    assert!(
        !stderr(&output).contains(s(scratch.path())),
        "no path in the error"
    );
}

// -- recordings --------------------------------------------------------------

#[test]
fn list_show_and_export_work_on_a_recording() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let folder = record_one(&settings, &dir, &["--name", "Interview"]);
    let id = manifest(&folder)["recording_id"]
        .as_str()
        .unwrap()
        .to_string();
    let prefix = &id[..8];

    // list
    let output = run(&settings, &["recordings", "list", "--dir", s(&dir)]);
    ok(&output);
    let table = stdout(&output);
    let mut lines = table.lines();
    let header = lines.next().unwrap();
    for column in ["ID", "WHEN", "TITLE", "LENGTH", "QUALITY", "SIZE", "STATE"] {
        assert!(header.contains(column), "{header}");
    }
    let row = lines.next().unwrap();
    assert!(
        row.starts_with(prefix)
            && row.contains("Interview")
            && row.contains("lossless")
            && row.ends_with("complete"),
        "{row}"
    );
    assert!(lines.next().is_none());

    let output = run(
        &settings,
        &["recordings", "list", "--dir", s(&dir), "--json"],
    );
    ok(&output);
    let list = json(&output);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["recording_id"], id.as_str());
    assert_eq!(list[0]["state"], "complete");
    assert_eq!(list[0]["playable"], true);

    // show: by id prefix, by folder name and by path
    let name = folder.file_name().unwrap().to_str().unwrap().to_string();
    for target in [prefix, name.as_str(), s(&folder)] {
        let output = run(
            &settings,
            &["recordings", "show", target, "--dir", s(&dir), "--json"],
        );
        ok(&output);
        let shown = json(&output);
        assert_eq!(shown["recording_id"], id.as_str(), "{target}");
        assert_eq!(shown["folder"], s(&folder), "{target}");
        assert_eq!(shown["tracks"][0]["codec"], "flac");
        assert_eq!(shown["tracks"][0]["sample_rate"], 48_000);
        assert_eq!(shown["tracks"][0]["channels"], 1);
        assert!(
            !stdout(&output).contains("Synthetic microphone"),
            "the device name stays out of --json"
        );
    }
    let output = run(&settings, &["recordings", "show", prefix, "--dir", s(&dir)]);
    ok(&output);
    assert!(stdout(&output).contains("Title:    Interview"));
    assert!(stdout(&output).contains("device: Synthetic microphone"));

    // export tracks: FLAC is a byte copy, WAV is decoded
    let out_flac = TempDir::new().path().join("flac");
    let output = run(
        &settings,
        &[
            "recordings",
            "export",
            prefix,
            "--dir",
            s(&dir),
            "-o",
            s(&out_flac),
            "--json",
        ],
    );
    ok(&output);
    let report = json(&output);
    assert_eq!(report["derived"], false);
    assert!(report["note"].is_null());
    let exported = out_flac.join("mic.flac");
    assert_eq!(
        std::fs::read(&exported).unwrap(),
        std::fs::read(folder.join("mic.flac")).unwrap()
    );
    assert_eq!(report["files"][0], s(&exported));

    let wav_dir = TempDir::new();
    let output = run(
        &settings,
        &[
            "recordings",
            "export",
            prefix,
            "--dir",
            s(&dir),
            "--format",
            "wav",
            "-o",
            s(wav_dir.path()),
        ],
    );
    ok(&output);
    let wav = std::fs::read(wav_dir.path().join("mic.wav")).unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert!(wav.len() > 100_000, "{} bytes", wav.len());
    assert_eq!(stdout(&output).trim(), s(&wav_dir.path().join("mic.wav")));

    // export stereo and mixdown: one derived file each, said on stderr and in JSON
    for (layout, format, file) in [
        ("stereo", "flac", "stereo.flac"),
        ("mixdown", "wav", "mixdown.wav"),
        ("mixdown", "flac", "mixdown.flac"),
    ] {
        let target_dir = TempDir::new();
        let target = target_dir.path().join(file);
        let output = run(
            &settings,
            &[
                "recordings",
                "export",
                prefix,
                "--dir",
                s(&dir),
                "--layout",
                layout,
                "--format",
                format,
                "-o",
                s(&target),
                "--json",
            ],
        );
        ok(&output);
        assert!(target.is_file(), "{layout} {format}");
        let report = json(&output);
        assert_eq!(report["derived"], true, "{layout}");
        let note = report["note"].as_str().unwrap();
        assert!(!note.is_empty());
        assert!(
            stderr(&output).contains(&format!("Note: {note}")),
            "{}",
            stderr(&output)
        );
        assert_eq!(report["files"][0], s(&target));

        // an existing output is never overwritten
        let again = run(
            &settings,
            &[
                "recordings",
                "export",
                prefix,
                "--dir",
                s(&dir),
                "--layout",
                layout,
                "--format",
                format,
                "-o",
                s(&target),
            ],
        );
        assert_eq!(again.status.code(), Some(1));
        assert!(
            stderr(&again).contains("already exists"),
            "{}",
            stderr(&again)
        );
    }
}

#[test]
fn an_empty_folder_lists_nothing_and_says_so() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let output = run(&settings, &["recordings", "list", "--dir", s(&dir)]);
    ok(&output);
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("No recordings in"));
    let output = run(
        &settings,
        &["recordings", "list", "--dir", s(&dir), "--json"],
    );
    ok(&output);
    assert_eq!(json(&output), serde_json::json!([]));

    let output = run(
        &settings,
        &["recordings", "show", "nothing", "--dir", s(&dir), "--json"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(json(&output)["code"]
        .as_str()
        .unwrap()
        .starts_with("recording_"));
}

/// A recording a process left in the `recording` state: one second of sine
/// pushed straight into the store (no session, no device). The writer holds the
/// folder's lock until it is dropped.
fn interrupted_recording(root: &Path) -> RecordingWriter {
    let request = NewRecording {
        title: "Interrupted".into(),
        started_at: "2026-11-05T14:02:11.482+01:00".into(),
        client: Client {
            kind: "cli".into(),
            app_version: "test".into(),
            build: "test".into(),
            extra: Default::default(),
        },
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
    };
    let mut writer = RecordingStore::new(root)
        .create(request)
        .expect("create the recording");
    let samples: Vec<f32> = (0..48_000)
        .map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin())
        .collect();
    writer.push("mic", &samples).expect("push audio");
    writer.sync().expect("sync");
    writer
}

#[test]
fn recover_finishes_an_interrupted_recording_and_never_touches_a_live_one() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let writer = interrupted_recording(dir.path());
    let folder = writer.folder().to_path_buf();
    let name = folder.file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(manifest(&folder)["state"], "recording");

    // While this process holds the lock the recording is another process's.
    let output = run(
        &settings,
        &["recordings", "recover", "--dir", s(&dir), "--json"],
    );
    ok(&output);
    let notes = json(&output);
    assert_eq!(notes[0]["outcome"], "skipped_locked");
    assert_eq!(manifest(&folder)["state"], "recording");
    let output = run(
        &settings,
        &["recordings", "delete", &name, "--dir", s(&dir), "--yes"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(folder.is_dir(), "a live recording is not deleted");

    // Nothing owns it any more: list says so and recover finishes it.
    drop(writer);
    let output = run(&settings, &["recordings", "list", "--dir", s(&dir)]);
    ok(&output);
    assert!(
        stdout(&output).contains("needs recovery"),
        "{}",
        stdout(&output)
    );
    assert!(stderr(&output).contains("recordings recover"));

    let output = run(
        &settings,
        &["recordings", "recover", "--dir", s(&dir), "--json"],
    );
    ok(&output);
    let notes = json(&output);
    assert_eq!(notes.as_array().unwrap().len(), 1);
    assert_eq!(notes[0]["outcome"], "recovered");
    assert_eq!(notes[0]["title"], "Interrupted");
    assert!(notes[0]["frames_kept"].as_u64().unwrap() > 0);
    assert!(notes[0]["tail_bytes"].is_u64());

    let manifest = manifest(&folder);
    assert_eq!(manifest["state"], "complete");
    assert_eq!(manifest["recovered"], true);
    let output = run(&settings, &["recordings", "list", "--dir", s(&dir)]);
    assert!(stdout(&output).contains("recovered"));

    // Recovered audio can be exported; recovering again finds nothing to do.
    let target = TempDir::new();
    ok(&run(
        &settings,
        &[
            "recordings",
            "export",
            &name,
            "--dir",
            s(&dir),
            "-o",
            s(target.path()),
        ],
    ));
    let output = run(
        &settings,
        &["recordings", "recover", &name, "--dir", s(&dir), "--json"],
    );
    ok(&output);
    assert_eq!(json(&output)[0]["outcome"], "already_complete");
    let output = run(&settings, &["recordings", "recover", "--dir", s(&dir)]);
    ok(&output);
    assert!(stderr(&output).contains("Nothing to recover."));
}

#[test]
fn delete_needs_yes_and_then_removes_the_folder() {
    let settings = fake_settings();
    let dir = TempDir::new();
    let folder = record_one(&settings, &dir, &["--name", "Doomed"]);
    let id = manifest(&folder)["recording_id"]
        .as_str()
        .unwrap()
        .to_string();

    let output = run(
        &settings,
        &["recordings", "delete", &id[..8], "--dir", s(&dir)],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("--yes"), "{}", stderr(&output));
    assert!(folder.is_dir(), "nothing is deleted without --yes");

    let output = run(
        &settings,
        &[
            "recordings",
            "delete",
            &id[..8],
            "--dir",
            s(&dir),
            "--yes",
            "--json",
        ],
    );
    ok(&output);
    let report = json(&output);
    assert_eq!(report["deleted"], true);
    assert_eq!(report["recording_id"], id.as_str());
    assert!(!folder.exists());
    assert!(stderr(&output).contains("Deleted "));

    // Only plain names inside the recordings folder: a path elsewhere is not a recording.
    let other = TempDir::new();
    let output = run(
        &settings,
        &["recordings", "delete", s(other.path()), "--yes"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(other.path().is_dir());
}

#[test]
fn permissions_status_reports_the_recording_microphone() {
    let settings = fake_settings();
    let output = run(&settings, &["permissions", "status", "--json"]);
    ok(&output);
    let report = json(&output);
    let word = report["recording"]["microphone"].as_str().unwrap();
    assert!(
        [
            "granted",
            "denied",
            "not_determined",
            "not_applicable",
            "unknown"
        ]
        .contains(&word),
        "{word}"
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(word, "not_applicable");
    let output = run(&settings, &["permissions", "status"]);
    ok(&output);
    assert!(stdout(&output).contains("Recording (microphone): "));
}
