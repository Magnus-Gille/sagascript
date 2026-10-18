//! `transcribe`'s observable contract, frozen before it is split into steps
//! (#343, sprint K04).
//!
//! The tests run the real `sagascript` binary on synthetic inputs and pin what
//! a caller can see: stdout, stderr (the human lines and the `--progress-json`
//! events), the exit status and the error `code`. They are the proof that
//! moving code between modules, and later reshaping `transcribe_file`, changes
//! nothing outside the process.
//!
//! No model weights are needed:
//!
//! - a diarization cache is a serialized fixture; a valid hit skips decoding,
//!   Whisper and the diarization models, but still runs the planning, progress,
//!   assembly and output paths (`cached_*`);
//! - a sparse file of the right size makes a Whisper model count as downloaded,
//!   so a run gets through planning and decoding and stops at the model's
//!   integrity check (`fresh_*`, `item_failures_*`, `interrupt_*`; Linux and
//!   macOS, where `HOME`/`XDG_DATA_HOME` relocate the model directory);
//! - everything that fails before inference (`preflight_*`, `model_selection_*`)
//!   needs nothing at all.
//!
//! What cannot be reached without weights is a successful Whisper inference.
//! `transcribe_file_tests.rs` drives it in-process with a scripted backend: the
//! plain `--json` result object, `==> file <==` headers, progress events and
//! corrections. A successful Pianissimo run needs only the fake engine host, so
//! `pianissimo_with_the_fake_host` runs it end to end where the platform gate
//! allows it (macOS on Apple Silicon).
//!
//! Dynamic values (times, build revision and version, platform and path
//! details) are normalised explicitly, by `Sandbox::stderr_lines` and
//! `normalize_report`. Settings, `HOME` and every user directory are the
//! sandbox's (ADR 0004); nothing is downloaded. Lines are split with `lines()`
//! so a Windows checkout's line endings do not matter.

// Helpers are shared by modules that are compiled in only for some platforms and
// feature sets (the diarization cache runs, the placeholder-model runs); a build
// that compiles out all of a helper's users would otherwise flag it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use sagascript_testsupport::{isolated_cli_env, TempDir};
use serde_json::{json, Value};

/// A temp directory that is the CLI's whole world: its working directory, its
/// settings (`{}`), its `HOME` and its model directory.
struct Sandbox {
    root: TempDir,
}

/// What a finished run showed: the exit status plus both streams as text.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new();
        std::fs::create_dir_all(root.join("home")).expect("create isolated home");
        std::fs::write(root.join("settings.json"), "{}\n").expect("write settings");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture directory");
        }
        std::fs::write(&path, bytes).expect("write fixture");
        path
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
        isolated_cli_env(&mut command, &self.root.join("settings.json"));
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, self.home());
        }
        command
            .current_dir(self.root.path())
            .args(args)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Run {
        let output = self.command(args).output().expect("run sagascript");
        Run::from(output)
    }

    /// `text` with the sandbox path and Windows path separators made portable.
    fn portable(&self, text: &str) -> String {
        let mut text = text.to_string();
        let root = self.root.path();
        let mut roots = vec![root.display().to_string()];
        if let Ok(canonical) = root.canonicalize() {
            roots.push(canonical.display().to_string());
        }
        roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
        for root in roots {
            text = text.replace(&root, "<SANDBOX>");
        }
        text.replace('\\', "/")
    }

    /// stderr as the contract sees it: log records dropped, dynamic parts
    /// replaced (see [`normalize_line`]).
    fn stderr_lines(&self, run: &Run) -> Vec<String> {
        run.stderr
            .lines()
            .filter(|line| !is_log_record(line))
            .map(|line| normalize_line(&self.portable(line)))
            .collect()
    }
}

impl From<Output> for Run {
    fn from(output: Output) -> Self {
        Self {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("CLI stdout is UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("CLI stderr is UTF-8"),
        }
    }
}

/// A `tracing` record (timestamp, level, target, message), which the CLI's
/// dependencies write to stderr with colour codes. The records are not part of
/// the contract and carry the time.
fn is_log_record(line: &str) -> bool {
    let mut plain = String::new();
    let mut chars = line.chars();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            for escaped in chars.by_ref() {
                if escaped.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    let plain = plain.trim_start();
    plain.starts_with("20")
        && [" WARN ", " ERROR ", " INFO ", " DEBUG ", " TRACE "]
            .iter()
            .any(|level| plain.contains(level))
}

/// Replaces what differs between runs and platforms in one stderr line:
/// elapsed times, speed, and the acceleration line.
fn normalize_line(line: &str) -> String {
    if line.starts_with("Whisper acceleration: ") {
        return "Whisper acceleration: <PLATFORM>".to_string();
    }
    if let Some(rest) = line.strip_prefix("Diarized transcription: ") {
        // "0.00s total (1426.86x realtime), cache_hit=true"
        if let Some((_, tail)) = rest.split_once(" total (") {
            if let Some((_, tail)) = tail.split_once("x realtime)") {
                return format!("Diarized transcription: <TIME> total (<SPEED> realtime){tail}");
            }
        }
    }
    if line.starts_with('{') {
        if let Ok(mut event) = serde_json::from_str::<Value>(line) {
            if event["event"] == "transcription_progress" {
                event["elapsed_ms"] = json!(0);
                return event.to_string();
            }
        }
    }
    line.to_string()
}

#[track_caller]
fn assert_lines(actual: &[String], expected: &[&str], context: &str) {
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(
        actual,
        expected.as_slice(),
        "{context}: stderr lines differ\nactual:\n{}\n",
        actual.join("\n")
    );
}

#[track_caller]
fn assert_failed(run: &Run, context: &str) {
    assert_eq!(
        run.code,
        Some(1),
        "{context}: a failed run exits 1; stderr={}",
        run.stderr
    );
}

fn error_line(sandbox: &Sandbox, run: &Run) -> String {
    sandbox
        .stderr_lines(run)
        .into_iter()
        .rev()
        .find_map(|line| line.strip_prefix("Error: ").map(str::to_string))
        .unwrap_or_else(|| panic!("no `Error:` line in {}", run.stderr))
}

// ---- failures before any inference ------------------------------------------

/// Every way `transcribe` refuses to start, with the message, the `code` of
/// the JSON error object and the exit status. The order in which the checks
/// run is part of the contract: each case has exactly one thing wrong.
#[test]
fn preflight_failures_keep_their_message_code_and_exit_status() {
    let sandbox = Sandbox::new();
    sandbox.write("a.wav", b"RIFF");
    sandbox.write("b.wav", b"RIFF");
    sandbox.write("empty/readme.txt", b"no audio here");
    sandbox.write("tree/nested/c.wav", b"RIFF");

    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["empty"],
            "file_decode_error",
            "File decode error: No supported audio/video files were found in the supplied inputs",
        ),
        (
            &["a.wav", "b.wav", "--clipboard"],
            "settings_error",
            "Settings error: --clipboard is only available when transcribing one file",
        ),
        (
            &["a.wav", "--model", "nope"],
            "settings_error",
            "Settings error: Unknown file model preference 'nope'",
        ),
        (
            &["a.wav", "--model", "auto"],
            "settings_error",
            "Settings error: --model needs a concrete model ID; omit it to use the file default",
        ),
        (
            &["a.wav", "--model", "pianissimo-sv", "--language", "en"],
            "settings_error",
            "Settings error: Pianissimo Original is only available for Swedish file transcription",
        ),
        (
            &["a.wav", "--model", "pianissimo-sv", "--language", "sv", "--vad"],
            "settings_error",
            "Settings error: Pianissimo does not support --vad, --beam, --parallel, --correct-hints, or decoder --prompt/--prompt-file",
        ),
        (
            &["a.wav", "--model", "pianissimo-sv", "--language", "sv", "--beam", "2"],
            "settings_error",
            "Settings error: Pianissimo does not support --vad, --beam, --parallel, --correct-hints, or decoder --prompt/--prompt-file",
        ),
        (
            &["a.wav", "--language", "en", "--correct-hints"],
            "settings_error",
            "Settings error: --correct-hints requires a non-empty --hint/--prompt, saved initial_prompt, or selected profile dictionary",
        ),
        (
            &["a.wav", "--language", "en", "--correct-hints", "--prompt", "Notre Dame"],
            "settings_error",
            "Settings error: --correct-hints requires at least one single-word vocabulary item in the hint",
        ),
        (
            &["a.wav", "--language", "en", "--model", "tiny.en"],
            "transcription_failed",
            "Transcription failed: Model 'Whisper Tiny (EN)' is not downloaded. Run: sagascript download-model tiny.en",
        ),
    ];
    for (flags, code, message) in cases {
        let mut args = vec!["transcribe"];
        args.extend_from_slice(flags);
        let correct_hints = flags.contains(&"--correct-hints");
        for json in [false, true] {
            let mut args = args.clone();
            if json {
                args.push("--json");
            } else if correct_hints {
                // `--correct-hints` is only accepted together with `--json`.
                continue;
            }
            let run = sandbox.run(&args);
            assert_failed(&run, &format!("{args:?}"));
            assert_eq!(error_line(&sandbox, &run), *message, "{args:?}");
            if json {
                let object: Value = serde_json::from_str(&run.stdout)
                    .unwrap_or_else(|error| panic!("{args:?}: stdout is not JSON ({error})"));
                let keys: Vec<&str> = object
                    .as_object()
                    .expect("an error object")
                    .keys()
                    .map(String::as_str)
                    .collect();
                assert_eq!(object["status"], "error", "{args:?}");
                assert_eq!(object["code"], *code, "{args:?}");
                assert_eq!(object["error"], *message, "{args:?}");
                assert_eq!(keys.len(), 3, "{args:?}: {object}");
            } else {
                assert!(
                    run.stdout.is_empty(),
                    "{args:?}: plain output keeps stdout empty, got {:?}",
                    run.stdout
                );
            }
        }
    }
}

/// Usage errors are clap's: exit status 2, nothing on stdout, no error object.
#[test]
fn preflight_usage_errors_exit_two_without_running_anything() {
    let sandbox = Sandbox::new();
    sandbox.write("a.wav", b"RIFF");
    for (flags, first_line) in [
        (
            &["--parallel", "9"][..],
            "error: invalid value '9' for '--parallel <N>': parallel must be between 1 and 4, got 9",
        ),
        (
            &["--vad", "--no-vad"][..],
            "error: the argument '--vad' cannot be used with '--no-vad'",
        ),
        (
            &["--json", "--jsonl"][..],
            "error: the argument '--json' cannot be used with '--jsonl'",
        ),
        (
            &["--correct-hints"][..],
            "error: the following required arguments were not provided:",
        ),
    ] {
        let mut args = vec!["transcribe", "a.wav", "--language", "en"];
        args.extend_from_slice(flags);
        let run = sandbox.run(&args);
        assert_eq!(run.code, Some(2), "{args:?}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{args:?}: {:?}", run.stdout);
        assert_eq!(run.stderr.lines().next(), Some(first_line), "{args:?}");
    }
    let none = sandbox.run(&["transcribe"]);
    assert_eq!(none.code, Some(2));
    assert!(none.stdout.is_empty());
}

/// Which model a run asks for. Each row is a run that reaches the model check
/// and finds no model: the message names the model that was chosen. Rows that
/// could pick Pianissimo on some platforms add `--beam`, which needs Whisper.
#[test]
fn model_selection_names_the_model_each_language_resolves_to() {
    let sandbox = Sandbox::new();
    sandbox.write("a.wav", b"RIFF");
    for (flags, expected) in [
        (&["--language", "en"][..], "Whisper Base (EN)|base.en"),
        (
            &["--language", "en", "--beam", "2"][..],
            "Whisper Base (EN)|base.en",
        ),
        (
            &["--language", "sv", "--beam", "2"][..],
            "KB-Whisper Medium|kb-whisper-medium",
        ),
        (
            &["--language", "no", "--beam", "2"][..],
            "NB-Whisper Base|nb-whisper-base",
        ),
        (
            &["--language", "fi", "--beam", "2"][..],
            "Whisper Base|base",
        ),
        (
            &["--language", "auto", "--beam", "2"][..],
            "Whisper Base|base",
        ),
        (
            &["--language", "sv", "--model", "kb-whisper-base"][..],
            "KB-Whisper Base|kb-whisper-base",
        ),
        (
            &["--language", "sv", "--model", "base"][..],
            "Whisper Base|base",
        ),
        (
            &["--language", "en", "--parallel", "3"][..],
            "Whisper Base (EN)|base.en",
        ),
    ] {
        let mut args = vec!["transcribe", "a.wav"];
        args.extend_from_slice(flags);
        let run = sandbox.run(&args);
        assert_failed(&run, &format!("{args:?}"));
        let (name, id) = expected.split_once('|').expect("name|id");
        assert_eq!(
            error_line(&sandbox, &run),
            format!(
                "Transcription failed: Model '{name}' is not downloaded. Run: sagascript download-model {id}"
            ),
            "{args:?}"
        );
        assert!(run.stdout.is_empty(), "{args:?}");
    }
}

// ---- diarized runs from a cache ----------------------------------------------

#[cfg(feature = "diarization")]
mod cached {
    use super::*;
    use sagascript_testsupport::sha256_hex;

    const SEGMENTATION_SHA256: &str =
        "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079";
    const EMBEDDING_SHA256: &str =
        "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";
    const SOURCE: &[u8] = b"synthetic source bytes; a cache hit must not decode this file\n";

    /// A sandbox holding `synthetic.wav` (never decoded) and a matching
    /// diarization cache for `--language en --model tiny.en --diarize`.
    fn sandbox_with_cache() -> Sandbox {
        let sandbox = Sandbox::new();
        sandbox.write("synthetic.wav", SOURCE);
        sandbox.write("cache.json", &cache_fixture("tiny.en"));
        sandbox
    }

    fn cache_fixture(model: &str) -> Vec<u8> {
        let fixture = json!({
            "identity": {
                "schema_version": 6,
                "input_sha256": sha256_hex(SOURCE),
                "language": "en",
                "model": model,
                "prompt_sha256": sha256_hex(b""),
                "decoder_beam_size": 0,
                "temperature_fallback": true,
                "vad_model_id": null,
            },
            "analysis_identity": {
                "segmentation_sha256": SEGMENTATION_SHA256,
                "embedding_sha256": EMBEDDING_SHA256,
                "min_segment": 0.3,
                "min_gap": 0.5,
                "exclusive_speech_embeddings": false,
            },
            "analysis": {"raw_segments": [[0.0, 0.5, 0]], "embeddings": []},
            "transcript": [[0.1, 0.4, "synthetic cached text"]],
            "coverage_segments": [{
                "start": 0.1,
                "end": 0.4,
                "text": "synthetic cached text",
                "avg_logprob": null,
                "no_speech_prob": 0.0,
            }],
            "coverage_profile": {"audio_samples": 16_000, "speech_frames": vec![false; 50]},
            "detected_language": null,
            "language_regions": null,
        });
        serde_json::to_vec_pretty(&fixture).expect("fixture serializes")
    }

    fn cached_run(sandbox: &Sandbox, extra: &[&str]) -> Run {
        let mut args = vec![
            "transcribe",
            "synthetic.wav",
            "--language",
            "en",
            "--model",
            "tiny.en",
            "--diarize",
            "--diarize-cache",
            "cache.json",
        ];
        args.extend_from_slice(extra);
        sandbox.run(&args)
    }

    /// The stderr of a cache hit, in order: the batch header, the cache note,
    /// the (cached) audio line, the acceleration line, the decoder line, the
    /// clustering lines and the summary.
    const HIT_STDERR: [&str; 9] = [
        "[1/1] synthetic.wav",
        "Reusing diarization cache: cache.json",
        "Audio (cached): 1.0s",
        "Whisper acceleration: <PLATFORM>",
        "Diarization decoder: beam=0, temp_fallback=true, vad=false",
        "  Found 1 speaker(s)",
        "Found 1 speaker segment(s)",
        "Got 1 word/segment(s) for merging",
        "Diarized transcription: <TIME> total (<SPEED> realtime), cache_hit=true",
    ];

    const DECODER_EVENT: &str = r#""decoder":{"beam_size":0,"strategy":"greedy","temperature_fallback":true,"timestamp_method":"dtw_words_with_segment_fallback","vad_enabled":false,"vad_min_silence_duration_ms":200,"vad_samples_overlap":0.10000000149011612,"vad_speech_pad_ms":50,"vad_threshold":0.5}"#;

    fn progress_event(phase: &str) -> String {
        format!(
            r#"{{{DECODER_EVENT},"elapsed_ms":0,"event":"transcription_progress","percent":null,"phase":"{phase}","step":null,"steps_total":null}}"#
        )
    }

    /// Replaces the dynamic parts of a diarized report (legacy `--json` or the
    /// `result` of a `--jsonl` item) and returns the top-level keys in the
    /// order the CLI wrote them.
    fn normalize_report(report: &mut Value) {
        let top = report.as_object_mut().expect("report object");
        let diarization = top
            .get_mut("diarization")
            .and_then(Value::as_object_mut)
            .expect("diarization object");
        diarization.insert("build_revision".into(), json!("<REVISION>"));
        diarization.insert("build_version".into(), json!("<VERSION>"));
        let performance = top
            .get_mut("performance")
            .and_then(Value::as_object_mut)
            .expect("performance object");
        for key in [
            "cache_lookup_seconds",
            "diarization_clustering_seconds",
            "json_assembly_seconds",
            "merge_diagnostics_seconds",
            "total_seconds",
        ] {
            assert!(performance.contains_key(key), "performance.{key}");
            performance.insert(key.into(), json!("<SECONDS>"));
        }
        for key in ["acceleration_backend", "coreml_status"] {
            assert!(performance.contains_key(key), "performance.{key}");
            performance.insert(key.into(), json!("<PLATFORM>"));
        }
    }

    /// The report a cache hit prints for `--json`/`--jsonl`, normalised. The
    /// zero timings are the contract that a hit does no inference work.
    fn expected_report() -> Value {
        json!({
            "coverage_ratio": 1.0,
            "detected_language": null,
            "diarization": {
                "activity": [{"end": 0.5, "speakers": ["SPEAKER_0"], "start": 0.0}],
                "asr_segments": [],
                "attributions": [],
                "build_revision": "<REVISION>",
                "build_version": "<VERSION>",
                "decoder": {
                    "beam_size": 0,
                    "strategy": "greedy",
                    "temperature_fallback": true,
                    "timestamp_method": "dtw_words_with_segment_fallback",
                    "vad_enabled": false,
                    "vad_min_silence_duration_ms": 200,
                    "vad_samples_overlap": 0.100_000_001_490_116_12,
                    "vad_speech_pad_ms": 50,
                    "vad_threshold": 0.5,
                },
                "diagnostics_included": false,
                "duration_seconds": 1.0,
                "embedding_model_sha256": EMBEDDING_SHA256,
                "parameters": {
                    "absorb_max_distance": 0.75,
                    "exclusive_speech_embeddings": false,
                    "hint_merge_max_distance": 0.899_999_976_158_142_1,
                    "min_gap_seconds": 0.5,
                    "min_segment_seconds": 0.3,
                    "min_speaker_seconds": 8.0,
                    "threshold": 0.340_000_003_576_278_7,
                },
                "regions": [],
                "schema_version": 1,
                "segmentation_model_sha256": SEGMENTATION_SHA256,
                "source_sha256": sha256_hex(SOURCE),
                "speaker_hint": null,
                "speaker_hint_outcome": null,
                "transcript_modified": false,
            },
            "duration_seconds": 1.0,
            "file": "synthetic.wav",
            "language": "en",
            "language_redetection_enabled": false,
            "language_regions": null,
            "model": "tiny.en",
            "performance": {
                "acceleration_backend": "<PLATFORM>",
                "cache_hit": true,
                "cache_lookup_seconds": "<SECONDS>",
                "cache_write_seconds": 0.0,
                "coreml_status": "<PLATFORM>",
                "decode_resample_seconds": 0.0,
                "diarization_clustering_seconds": "<SECONDS>",
                "diarization_embeddings_seconds": 0.0,
                "diarization_model_load_seconds": 0.0,
                "diarization_segment_extraction_seconds": 0.0,
                "diarization_segmentation_seconds": 0.0,
                "effective_beam_size": 0,
                "effective_temperature_fallback": true,
                "effective_vad": false,
                "json_assembly_seconds": "<SECONDS>",
                "language_detection_seconds": 0.0,
                "merge_diagnostics_seconds": "<SECONDS>",
                "model_load_seconds": 0.0,
                "parallel_analysis_span_seconds": 0.0,
                "total_seconds": "<SECONDS>",
                "whisper_inference_seconds": 0.0,
                "word_timestamp_attribution_seconds": 0.0,
            },
            "segments": [{
                "end": 0.4,
                "speaker": "SPEAKER_0",
                "start": 0.1,
                "text": "synthetic cached text",
            }],
            "speakers": ["SPEAKER_0"],
            "uncovered_spans": [],
            "vocabulary_corrections": [],
            "warnings": [],
        })
    }

    /// The input recording and the cache are read, never written, by a hit.
    #[track_caller]
    fn assert_inputs_untouched(sandbox: &Sandbox) {
        assert_eq!(
            std::fs::read(sandbox.root.join("synthetic.wav")).expect("source"),
            SOURCE
        );
        assert_eq!(
            std::fs::read(sandbox.root.join("cache.json")).expect("cache"),
            cache_fixture("tiny.en"),
            "a cache hit must not rewrite the cache"
        );
    }

    #[test]
    fn plain_text_prints_one_line_per_speaker_turn_and_the_human_progress_on_stderr() {
        let sandbox = sandbox_with_cache();
        let run = cached_run(&sandbox, &[]);
        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert_eq!(run.stdout, "[SPEAKER_0] synthetic cached text\n");
        assert_lines(&sandbox.stderr_lines(&run), &HIT_STDERR, "text");
        assert_inputs_untouched(&sandbox);
    }

    #[test]
    fn json_prints_one_pretty_report_with_sorted_keys() {
        let sandbox = sandbox_with_cache();
        let run = cached_run(&sandbox, &["--json"]);
        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert!(run.stdout.ends_with("}\n"), "{:?}", run.stdout);
        let mut report: Value = serde_json::from_str(&run.stdout).expect("stdout is one document");
        let keys: Vec<&str> = report
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] < pair[1]),
            "keys are written sorted: {keys:?}"
        );
        // Two-space pretty printing, one key per line, as before.
        assert!(run.stdout.starts_with("{\n  \"coverage_ratio\": 1.0,\n"));
        normalize_report(&mut report);
        assert_eq!(report, expected_report());
        assert_lines(&sandbox.stderr_lines(&run), &HIT_STDERR, "json");
        assert_inputs_untouched(&sandbox);
    }

    #[test]
    fn jsonl_prints_one_compact_item_per_input() {
        let sandbox = sandbox_with_cache();
        let run = cached_run(&sandbox, &["--jsonl"]);
        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert!(run.stdout.ends_with("}\n"), "{:?}", run.stdout);
        let lines: Vec<&str> = run.stdout.lines().collect();
        assert_eq!(lines.len(), 1, "{}", run.stdout);
        assert!(
            lines[0].starts_with(
                r#"{"status":"ok","source":"synthetic.wav","result":{"coverage_ratio":1.0,"#
            ),
            "{}",
            lines[0]
        );
        let mut item: Value = serde_json::from_str(lines[0]).expect("item is JSON");
        assert_eq!(item.as_object().map(|object| object.len()), Some(3));
        assert_eq!(item["status"], "ok");
        assert_eq!(item["source"], "synthetic.wav");
        normalize_report(&mut item["result"]);
        assert_eq!(item["result"], expected_report());
        assert_lines(&sandbox.stderr_lines(&run), &HIT_STDERR, "jsonl");
    }

    #[test]
    fn meeting_json_prints_the_shared_document_and_no_legacy_report() {
        let sandbox = sandbox_with_cache();
        let run = cached_run(&sandbox, &["--meeting-json"]);
        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        // Struct order, not sorted: the document's own field order.
        let top_level: Vec<&str> = run
            .stdout
            .lines()
            .filter_map(|line| line.strip_prefix("  \""))
            .filter_map(|line| line.split_once('"').map(|(key, _)| key))
            .collect();
        assert_eq!(
            top_level,
            [
                "schema_version",
                "source_sha256",
                "language",
                "model",
                "duration_seconds",
                "segments",
                "speakers",
                "diarization"
            ]
        );
        let mut document: Value = serde_json::from_str(&run.stdout).expect("one document");
        assert_eq!(document["schema_version"], 2);
        assert_eq!(document["source_sha256"], sha256_hex(SOURCE));
        assert_eq!(document["language"], "en");
        assert_eq!(document["model"], "tiny.en");
        assert_eq!(document["duration_seconds"], 1.0);
        assert_eq!(
            document["segments"],
            json!([{
                "id": "seg-000001",
                "start": 0.1,
                "end": 0.4,
                "text": "synthetic cached text",
                "speaker": "SPEAKER_0",
            }])
        );
        assert_eq!(
            document["speakers"],
            json!([{"id": "SPEAKER_0", "label": "SPEAKER_0"}])
        );
        assert_eq!(document["diarization"]["decoder"]["strategy"], "greedy");
        assert_eq!(document["diarization"]["source_sha256"], sha256_hex(SOURCE));
        assert!(document["diarization"]["build_revision"].is_string());
        assert!(document["diarization"].get("performance").is_none());
        document["diarization"]["build_revision"] = json!("<REVISION>");
        assert_eq!(document["diarization"]["build_revision"], "<REVISION>");
        assert_lines(&sandbox.stderr_lines(&run), &HIT_STDERR, "meeting-json");
        assert_inputs_untouched(&sandbox);
    }

    /// `--progress-json` events go to stderr between the human lines, in phase
    /// order, with the decoder snapshot on each (a cache hit has no decode,
    /// model or language phases); stdout stays the transcript only.
    #[test]
    fn progress_json_events_interleave_with_the_human_lines_on_stderr() {
        let sandbox = sandbox_with_cache();
        let run = cached_run(&sandbox, &["--progress-json"]);
        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert_eq!(run.stdout, "[SPEAKER_0] synthetic cached text\n");
        let analyzing = progress_event("analyzing");
        let clustering = progress_event("clustering");
        let finalizing = progress_event("finalizing");
        let completed = progress_event("completed");
        assert_lines(
            &sandbox.stderr_lines(&run),
            &[
                "[1/1] synthetic.wav",
                "Reusing diarization cache: cache.json",
                "Audio (cached): 1.0s",
                "Whisper acceleration: <PLATFORM>",
                "Diarization decoder: beam=0, temp_fallback=true, vad=false",
                &analyzing,
                &clustering,
                "  Found 1 speaker(s)",
                "Found 1 speaker segment(s)",
                "Got 1 word/segment(s) for merging",
                &finalizing,
                "Diarized transcription: <TIME> total (<SPEED> realtime), cache_hit=true",
                &completed,
            ],
            "progress",
        );
    }

    /// A cache that does not match the run is a miss, never an error: the run
    /// says why and goes on to compute, which needs the diarization models.
    /// None are installed here, so it ends as an item failure, in every mode,
    /// and the stale cache is left as it was.
    #[test]
    fn a_stale_cache_is_a_miss_that_fails_on_the_missing_models_and_is_not_rewritten() {
        let sandbox = Sandbox::new();
        sandbox.write("synthetic.wav", SOURCE);
        let stale = cache_fixture("base.en");
        sandbox.write("cache.json", &stale);
        let message = "Diarization error: Diarization models not found. Run: sagascript download-model diarization";
        let miss = "Diarization cache miss (input, model, language, prompt, or schema changed); computing analysis.";

        for (extra, stdout_kind) in [
            (&[][..], "empty"),
            (&["--json"][..], "object"),
            (&["--jsonl"][..], "line"),
            (&["--meeting-json"][..], "empty"),
        ] {
            let run = cached_run(&sandbox, extra);
            assert_failed(&run, &format!("{extra:?}"));
            assert_lines(
                &sandbox.stderr_lines(&run),
                &[
                    "[1/1] synthetic.wav",
                    miss,
                    &format!("Error transcribing synthetic.wav: {message}"),
                    "Error: Transcription failed: 1 of 1 batch item(s) failed",
                ],
                &format!("{extra:?}"),
            );
            match stdout_kind {
                "empty" => assert!(run.stdout.is_empty(), "{extra:?}: {:?}", run.stdout),
                "object" => {
                    let object: Value = serde_json::from_str(&run.stdout).expect("one document");
                    assert_eq!(
                        object,
                        json!({
                            "status": "error",
                            "code": "diarization_error",
                            "source": "synthetic.wav",
                            "error": message,
                        })
                    );
                }
                _ => {
                    // Item key order is status, source, error.
                    assert_eq!(
                        run.stdout,
                        format!(
                            "{{\"status\":\"error\",\"source\":\"synthetic.wav\",\"error\":{}}}\n",
                            serde_json::to_string(message).expect("message encodes")
                        )
                    );
                }
            }
            assert_eq!(
                std::fs::read(sandbox.root.join("cache.json")).expect("cache"),
                stale,
                "{extra:?}: the stale cache must be left alone"
            );
        }
    }

    /// With no cache file yet, `--diarize-cache` bypasses the Whisper model
    /// preflight (the cache might make it unnecessary); the miss reason is
    /// "not found" and nothing is created.
    #[test]
    fn a_missing_cache_file_is_a_miss_and_is_not_created_by_a_failed_run() {
        let sandbox = Sandbox::new();
        sandbox.write("synthetic.wav", SOURCE);
        let run = sandbox.run(&[
            "transcribe",
            "synthetic.wav",
            "--language",
            "en",
            "--model",
            "tiny.en",
            "--diarize",
            "--diarize-cache",
            "new-cache.json",
        ]);
        assert_failed(&run, "missing cache");
        assert_lines(
            &sandbox.stderr_lines(&run),
            &[
                "[1/1] synthetic.wav",
                "Diarization cache miss (not found); computing analysis.",
                "Error transcribing synthetic.wav: Diarization error: Diarization models not found. Run: sagascript download-model diarization",
                "Error: Transcription failed: 1 of 1 batch item(s) failed",
            ],
            "missing cache",
        );
        assert!(run.stdout.is_empty());
        assert!(!sandbox.root.join("new-cache.json").exists());
    }

    #[test]
    fn a_cache_that_is_the_input_recording_is_refused_before_it_is_read() {
        let sandbox = sandbox_with_cache();
        let run = sandbox.run(&[
            "transcribe",
            "synthetic.wav",
            "--language",
            "en",
            "--model",
            "tiny.en",
            "--diarize",
            "--diarize-cache",
            "synthetic.wav",
        ]);
        assert_failed(&run, "cache is the input");
        assert_lines(
            &sandbox.stderr_lines(&run),
            &[
                "[1/1] synthetic.wav",
                "Error transcribing synthetic.wav: Settings error: --diarize-cache must not point to the input recording",
                "Error: Transcription failed: 1 of 1 batch item(s) failed",
            ],
            "cache is the input",
        );
        assert_eq!(
            std::fs::read(sandbox.root.join("synthetic.wav")).expect("source"),
            SOURCE
        );
    }

    #[test]
    fn diarization_flags_that_make_no_sense_are_refused_with_their_messages() {
        let sandbox = Sandbox::new();
        sandbox.write("a.wav", b"RIFF");
        sandbox.write("b.wav", b"RIFF");
        sandbox.write("empty/readme.txt", b"no audio here");
        sandbox.write("tree/nested/c.wav", b"RIFF");
        let one = "Settings error: --meeting-json requires one explicit regular input file; directories, --recursive, and multiple inputs are unsupported";
        for (args, message) in [
            (
                &["a.wav", "b.wav", "--diarize"][..],
                "Settings error: --diarize is only available when transcribing one file",
            ),
            (
                &["a.wav", "--speakers", "2"][..],
                "Settings error: --speakers, --min-speakers, --max-speakers and --force-speakers require --diarize",
            ),
            (&["empty", "--diarize", "--meeting-json"][..], one),
            (&["a.wav", "b.wav", "--diarize", "--meeting-json"][..], one),
            (&["tree", "--recursive", "--diarize", "--meeting-json"][..], one),
        ] {
            let mut full = vec!["transcribe", "--language", "en"];
            full.extend_from_slice(args);
            let run = sandbox.run(&full);
            assert_failed(&run, &format!("{full:?}"));
            assert_eq!(error_line(&sandbox, &run), message, "{full:?}");
            assert!(run.stdout.is_empty(), "{full:?}");
        }
        // Usage errors from clap: the threshold range and the dependent flags.
        for (args, first_line) in [
            (
                &["--diarize", "--diarize-threshold", "9"][..],
                "error: invalid value '9' for '--diarize-threshold <THRESHOLD>': diarize-threshold must be between 0.0 and 2.0, got 9",
            ),
            (
                &["--meeting-json"][..],
                "error: the following required arguments were not provided:",
            ),
            (
                &["--diarize", "--meeting-json", "--json"][..],
                "error: the argument '--meeting-json' cannot be used with '--json'",
            ),
        ] {
            let mut full = vec!["transcribe", "a.wav", "--language", "en"];
            full.extend_from_slice(args);
            let run = sandbox.run(&full);
            assert_eq!(run.code, Some(2), "{full:?}: {}", run.stderr);
            assert_eq!(run.stderr.lines().next(), Some(first_line), "{full:?}");
        }
    }
}

// ---- runs that decode, with a placeholder model -------------------------------

/// A Whisper model "downloaded" as a sparse file of the right size: the CLI
/// checks the size before it starts and verifies the content only when it
/// loads the model, after decoding. The model directory follows `dirs` on the
/// platforms where `HOME`/`XDG_DATA_HOME` relocate it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod with_a_placeholder_model {
    use std::path::Path;

    use super::*;
    use sagascript_core::settings::WhisperModel;

    fn models_dir(sandbox: &Sandbox) -> PathBuf {
        #[cfg(target_os = "macos")]
        let data_dir = sandbox.home().join("Library").join("Application Support");
        #[cfg(target_os = "linux")]
        let data_dir = sandbox.home();
        data_dir.join("Sagascript").join("Models")
    }

    fn install_placeholder_model(sandbox: &Sandbox) -> PathBuf {
        let models = models_dir(sandbox);
        std::fs::create_dir_all(&models).expect("create isolated model dir");
        let model = WhisperModel::TinyEn;
        let path = models.join(model.ggml_filename());
        let file = std::fs::File::create(&path).expect("create placeholder model");
        file.set_len(model.download_integrity().size)
            .expect("size placeholder model");
        path
    }

    /// A 16 kHz mono 16-bit PCM WAV of `seconds` of silence.
    fn wav(seconds: usize) -> Vec<u8> {
        let samples = seconds * 16_000;
        let data_len = u32::try_from(samples * 2).expect("fixture fits a WAV");
        let mut bytes = Vec::with_capacity(44 + samples * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&32_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + samples * 2, 0);
        bytes
    }

    fn integrity_error(sandbox: &Sandbox, model_path: &Path) -> String {
        format!(
            "Model download failed: Model integrity check failed for {}: SHA-256 mismatch. Delete and re-download the model.",
            sandbox.portable(&model_path.display().to_string())
        )
    }

    /// One `--progress-json` event: its phase, percent and step.
    type Event = (String, Option<u64>, Option<u64>);

    /// The progress events of a stderr, and the other lines with each event
    /// replaced by a marker that names its phase.
    fn split_progress(lines: Vec<String>) -> (Vec<Event>, Vec<String>) {
        let mut events = Vec::new();
        let mut rest = Vec::new();
        for line in lines {
            match serde_json::from_str::<Value>(&line) {
                Ok(event) if event["event"] == "transcription_progress" => {
                    let phase = event["phase"].as_str().expect("phase").to_string();
                    assert_eq!(event["steps_total"], 3, "{event}");
                    events.push((phase, event["percent"].as_u64(), event["step"].as_u64()));
                    rest.push(format!(
                        "<progress {}>",
                        event["phase"].as_str().unwrap_or("?")
                    ));
                }
                _ => rest.push(line),
            }
        }
        (events, rest)
    }

    /// A fresh (not cached) run: the plan is made, the file is decoded and
    /// resampled with progress, the model is looked up and loaded, and here
    /// the load stops at its integrity check. Nothing reaches stdout in plain
    /// mode and the `--json` run reports the failure as one error object.
    #[test]
    fn fresh_run_decodes_with_progress_then_stops_at_the_model_load() {
        let sandbox = Sandbox::new();
        let model = install_placeholder_model(&sandbox);
        sandbox.write("tone.wav", &wav(1));
        let integrity = integrity_error(&sandbox, &model);

        let run = sandbox.run(&[
            "transcribe",
            "tone.wav",
            "--language",
            "en",
            "--model",
            "tiny.en",
            "--progress-json",
        ]);
        assert_failed(&run, "fresh");
        assert!(run.stdout.is_empty(), "{:?}", run.stdout);
        let (events, rest) = split_progress(sandbox.stderr_lines(&run));
        // Decoding runs 0..=100 and then resampling 0 and 100; the model phase
        // is announced without a percent. Progress never goes backwards.
        let decoding: Vec<u64> = events
            .iter()
            .filter(|(phase, _, _)| phase == "decoding")
            .map(|(_, percent, step)| {
                assert_eq!(*step, Some(1), "decoding is step 1 of 3");
                percent.expect("decode percent")
            })
            .collect();
        assert_eq!(decoding.first(), Some(&0));
        assert_eq!(decoding.last(), Some(&100));
        assert!(
            decoding.windows(2).all(|pair| pair[0] <= pair[1]),
            "{decoding:?}"
        );
        let tail: Vec<(&str, Option<u64>, Option<u64>)> = events
            .iter()
            .skip_while(|(phase, _, _)| phase == "decoding")
            .map(|(phase, percent, step)| (phase.as_str(), *percent, *step))
            .collect();
        assert_eq!(
            tail,
            [
                ("resampling", Some(0), Some(2)),
                ("resampling", Some(100), Some(2)),
                ("loading", None, Some(2)),
            ]
        );
        let mut expected: Vec<String> =
            vec!["[1/1] tone.wav".into(), "Decoding tone.wav...".into()];
        expected.extend(decoding.iter().map(|_| "<progress decoding>".to_string()));
        expected.extend([
            "<progress resampling>".into(),
            "<progress resampling>".into(),
            "Audio: 1.0s, 16000 samples".into(),
            "<progress loading>".into(),
            "Loading model: Whisper Tiny (EN)...".into(),
            format!("Error transcribing tone.wav: {integrity}"),
            "Error: Transcription failed: 1 of 1 batch item(s) failed".into(),
        ]);
        assert_eq!(rest, expected);

        // Without --progress-json the same run has no event lines.
        let run = sandbox.run(&[
            "transcribe",
            "tone.wav",
            "--language",
            "en",
            "--model",
            "tiny.en",
            "--json",
        ]);
        assert_failed(&run, "fresh json");
        assert_lines(
            &sandbox.stderr_lines(&run),
            &[
                "[1/1] tone.wav",
                "Decoding tone.wav...",
                "Audio: 1.0s, 16000 samples",
                "Loading model: Whisper Tiny (EN)...",
                &format!("Error transcribing tone.wav: {integrity}"),
                "Error: Transcription failed: 1 of 1 batch item(s) failed",
            ],
            "fresh json",
        );
        let object: Value = serde_json::from_str(&run.stdout).expect("one error object");
        assert_eq!(object["status"], "error");
        assert_eq!(object["code"], "model_download_failed");
        assert_eq!(object["source"], "tone.wav");
        assert_eq!(
            sandbox.portable(object["error"].as_str().expect("message")),
            integrity
        );
        assert_eq!(object.as_object().map(|map| map.len()), Some(4));
    }

    /// Item-level failures in a batch: every input gets its `[i/n]` header and
    /// its error, in the order given, in all three output modes; `--fail-fast`
    /// stops after the first. The exit status is 1 either way and the summary
    /// line counts the processed items.
    #[test]
    fn item_failures_are_reported_per_input_in_every_output_mode() {
        let sandbox = Sandbox::new();
        install_placeholder_model(&sandbox);
        sandbox.write("bad/a.wav", b"not audio");
        sandbox.write("bad/c.wav", b"not audio");
        let probe = "File decode error: Failed to probe file format: ";
        let open = "File decode error: Failed to open file: ";
        let inputs = ["bad/a.wav", "missing.wav", "bad/c.wav"];

        let run_with = |extra: &[&str]| {
            let mut args = vec!["transcribe", "--language", "en", "--model", "tiny.en"];
            args.extend_from_slice(extra);
            args.extend_from_slice(&inputs);
            sandbox.run(&args)
        };
        let headers = |run: &Run| -> Vec<String> {
            sandbox
                .stderr_lines(run)
                .into_iter()
                .filter(|line| {
                    line.starts_with('[')
                        || line.starts_with("Error")
                        || line.starts_with("Decoding")
                })
                .map(|line| {
                    // The message after the OS-specific part is not the contract.
                    if let Some((head, _)) = line.split_once(probe) {
                        format!("{head}{probe}…")
                    } else if let Some((head, _)) = line.split_once(open) {
                        format!("{head}{open}…")
                    } else {
                        line
                    }
                })
                .collect()
        };
        let all_three = [
            "[1/3] bad/a.wav",
            "Decoding bad/a.wav...",
            "Error transcribing bad/a.wav: File decode error: Failed to probe file format: …",
            "[2/3] missing.wav",
            "Decoding missing.wav...",
            "Error transcribing missing.wav: File decode error: Failed to open file: …",
            "[3/3] bad/c.wav",
            "Decoding bad/c.wav...",
            "Error transcribing bad/c.wav: File decode error: Failed to probe file format: …",
            "Error: Transcription failed: 3 of 3 batch item(s) failed",
        ];

        // plain: nothing on stdout
        let plain = run_with(&[]);
        assert_failed(&plain, "plain");
        assert!(plain.stdout.is_empty(), "{:?}", plain.stdout);
        assert_eq!(headers(&plain), all_three);

        // --jsonl: one compact item per input, written as each input ends
        let jsonl = run_with(&["--jsonl", "--progress-json"]);
        assert_failed(&jsonl, "jsonl");
        // `--progress-json` adds an event line per input; `headers` skips them.
        assert_eq!(headers(&jsonl), all_three);
        let items: Vec<Value> = jsonl
            .stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("item line"))
            .collect();
        assert_eq!(items.len(), 3);
        for (item, source) in items.iter().zip(inputs) {
            assert_eq!(item.as_object().map(|map| map.len()), Some(3), "{item}");
            assert_eq!(item["status"], "error");
            assert_eq!(item["source"], source);
            let message = item["error"].as_str().expect("message");
            assert!(
                message.starts_with(if source == "missing.wav" { open } else { probe }),
                "{message}"
            );
        }
        assert!(jsonl
            .stdout
            .starts_with(r#"{"status":"error","source":"bad/a.wav","error":"#));

        // --json: one document after the batch, an array for several inputs
        let json = run_with(&["--json"]);
        assert_failed(&json, "json");
        assert_eq!(headers(&json), all_three);
        let array: Value = serde_json::from_str(&json.stdout).expect("one document");
        let array = array.as_array().expect("several inputs make an array");
        assert_eq!(array.len(), 3);
        for (item, source) in array.iter().zip(inputs) {
            assert_eq!(item["status"], "error");
            assert_eq!(item["source"], source);
            assert!(
                item.get("code").is_none(),
                "batch items keep three keys: {item}"
            );
        }

        // --fail-fast: the first failure ends the batch; the summary counts one
        let fail_fast = run_with(&["--json", "--fail-fast"]);
        assert_failed(&fail_fast, "fail-fast");
        assert_eq!(
            headers(&fail_fast),
            [
                "[1/3] bad/a.wav",
                "Decoding bad/a.wav...",
                "Error transcribing bad/a.wav: File decode error: Failed to probe file format: …",
                "Error: Transcription failed: 1 of 1 batch item(s) failed",
            ]
        );
        let array: Value = serde_json::from_str(&fail_fast.stdout).expect("one document");
        assert_eq!(array.as_array().map(Vec::len), Some(1), "{array}");
    }

    /// Ctrl-C during decoding: the run unwinds instead of dying, reports a
    /// final `cancelled` progress event and `Cancelled`, prints nothing but
    /// the error object on stdout, and exits 130. The input is a FIFO, so the
    /// decoder is parked in `open` until the test lets it go; the signal is
    /// sent while it waits and the audio is delivered after the CLI has
    /// acknowledged the signal, so the cancellation is in place before the
    /// decode could finish. Unix only (signals, `mkfifo`).
    #[cfg(unix)]
    #[test]
    fn interrupt_during_decoding_cancels_cleanly_and_exits_130() {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        // A watchdog only: it turns a hang into a failure, never decides an outcome.
        const WATCHDOG: Duration = Duration::from_secs(60);

        for json in [false, true] {
            let sandbox = Sandbox::new();
            install_placeholder_model(&sandbox);
            let fifo = sandbox.root.join("pipe.wav");
            let made = Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("run mkfifo");
            assert!(made.success(), "mkfifo failed");

            let mut args = vec![
                "transcribe",
                "pipe.wav",
                "--language",
                "en",
                "--model",
                "tiny.en",
                "--progress-json",
            ];
            if json {
                args.push("--json");
            }
            let mut child = sandbox
                .command(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn sagascript");
            let pid = child.id();
            let mut stdout = child.stdout.take().expect("stdout pipe");
            let stderr = child.stderr.take().expect("stderr pipe");

            let (lines_tx, lines_rx) = mpsc::channel::<String>();
            let stderr_reader = std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    if lines_tx.send(line).is_err() {
                        break;
                    }
                }
            });
            let stdout_reader = std::thread::spawn(move || {
                let mut text = String::new();
                std::io::Read::read_to_string(&mut stdout, &mut text).expect("read stdout");
                text
            });

            let started = Instant::now();
            let mut seen: Vec<String> = Vec::new();
            let wait_for = |needle: &str, seen: &mut Vec<String>| loop {
                let remaining = WATCHDOG.saturating_sub(started.elapsed());
                let line = lines_rx
                    .recv_timeout(remaining)
                    .unwrap_or_else(|_| panic!("no {needle:?} on stderr; saw {seen:?}"));
                let found = line.contains(needle);
                seen.push(line);
                if found {
                    return;
                }
            };

            // The first decode event is written before the file is opened.
            wait_for(r#""phase":"decoding""#, &mut seen);
            let signalled = Command::new("kill")
                .args(["-INT", &pid.to_string()])
                .status()
                .expect("run kill");
            assert!(signalled.success(), "kill -INT failed");
            wait_for("Cancelling...", &mut seen);

            // Let the decoder open the FIFO and feed it enough audio that a
            // finished decode would be a long way off.
            let mut writer = std::fs::OpenOptions::new()
                .write(true)
                .open(&fifo)
                .expect("open the FIFO for writing");
            let audio = wav(120);
            // The CLI stops reading once it has noticed the cancellation; a
            // closed pipe is expected, not a failure.
            let _ = writer.write_all(&audio);
            drop(writer);

            let status = loop {
                match child.try_wait().expect("poll child") {
                    Some(status) => break status,
                    None if started.elapsed() > WATCHDOG => {
                        let _ = child.kill();
                        panic!("the CLI did not exit after the interrupt; saw {seen:?}");
                    }
                    None => std::thread::sleep(Duration::from_millis(10)),
                }
            };
            stderr_reader.join().expect("stderr reader");
            seen.extend(lines_rx.try_iter());
            let stdout_text = stdout_reader.join().expect("stdout reader");

            assert_eq!(status.code(), Some(130), "json={json}; saw {seen:?}");
            let run = Run {
                code: status.code(),
                stdout: stdout_text,
                stderr: seen.join("\n"),
            };
            let lines = sandbox.stderr_lines(&run);
            let cancelled_event = lines
                .iter()
                .position(|line| line.contains(r#""phase":"cancelled""#))
                .unwrap_or_else(|| panic!("no cancelled event in {lines:?}"));
            assert_eq!(
                lines[cancelled_event],
                r#"{"elapsed_ms":0,"event":"transcription_progress","percent":null,"phase":"cancelled","step":null,"steps_total":3}"#
            );
            assert_eq!(
                lines.last().map(String::as_str),
                Some("Cancelled"),
                "{lines:?}"
            );
            assert_eq!(
                cancelled_event + 2,
                lines.len(),
                "`cancelled` is the last event: {lines:?}"
            );
            assert!(
                lines.iter().all(|line| !line.contains("Loading model")),
                "a cancelled run must not go on to load the model: {lines:?}"
            );
            if json {
                let object: Value = serde_json::from_str(&run.stdout).expect("one error object");
                assert_eq!(
                    object,
                    json!({"status": "error", "code": "cancelled", "error": "Cancelled"})
                );
            } else {
                assert!(run.stdout.is_empty(), "{:?}", run.stdout);
            }
        }
    }
}

// ---- the Rust paths other crates import -----------------------------------------

/// `sagascript_cli::transcribe::*` is imported by the app (the meeting jobs)
/// and by the CLI's own modules. Taking each item as a typed function pointer
/// makes a moved or re-signatured export a compile error here, in the one place
/// that lists them, instead of in a distant importer.
#[test]
#[allow(
    clippy::type_complexity,
    reason = "the fn-pointer types are the point: they pin each exported signature"
)]
fn the_public_transcribe_paths_stay_importable() {
    use sagascript_cli::output::Out;
    use sagascript_cli::transcribe::{
        copy_to_clipboard, model_id_string, parse_language, parse_model, resolve_effective_model,
        resolve_file_model, resolve_file_model_for_run, run, TranscribeArgs,
    };
    use sagascript_core::error::DictationError;
    use sagascript_core::settings::{FileModel, Language, Settings, WhisperModel};

    let _: fn(TranscribeArgs, &Out) -> Result<(), DictationError> = run;
    let _: fn(&str) -> Result<Language, DictationError> = parse_language;
    let _: fn(&str) -> Result<WhisperModel, DictationError> = parse_model;
    let _: fn(WhisperModel) -> &'static str = model_id_string;
    let _: fn(Option<&str>, Language, bool, WhisperModel) -> Result<WhisperModel, DictationError> =
        resolve_effective_model;
    let _: fn(Option<&str>, Language, &Settings) -> Result<FileModel, DictationError> =
        resolve_file_model;
    let _: fn(Option<&str>, Language, &Settings, bool) -> Result<FileModel, DictationError> =
        resolve_file_model_for_run;
    let _: fn(&str) -> Result<(), DictationError> = copy_to_clipboard;

    #[cfg(feature = "diarization")]
    {
        use std::path::Path;

        use sagascript_cli::transcribe::{
            transcribe_meeting_file, transcribe_meeting_file_full,
            transcribe_meeting_file_with_control, MeetingControl, MeetingPhase,
        };
        use sagascript_core::meeting::MeetingTranscript;
        use sagascript_core::speaker_hint::SpeakerCountHint;
        use sagascript_core::transcription::{Glossary, WhisperBackend};

        let _: fn(
            &Path,
            &Settings,
            Language,
            WhisperModel,
            &Glossary,
            &WhisperBackend,
        ) -> Result<MeetingTranscript, DictationError> = transcribe_meeting_file;
        let _: fn(
            &Path,
            &Settings,
            Language,
            WhisperModel,
            &Glossary,
            &WhisperBackend,
            &MeetingControl<'_>,
        ) -> Result<MeetingTranscript, DictationError> = transcribe_meeting_file_with_control;
        let _: fn(
            &Path,
            &Settings,
            Language,
            WhisperModel,
            &Glossary,
            &WhisperBackend,
            Option<&MeetingControl<'_>>,
            f32,
            Option<SpeakerCountHint>,
            Option<&Path>,
        ) -> Result<MeetingTranscript, DictationError> = transcribe_meeting_file_full;
        // The phases are what the app maps to its job states, by serialized name.
        let names: Vec<String> = [
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ]
        .iter()
        .map(|phase| serde_json::to_string(phase).expect("phase serializes"))
        .collect();
        assert_eq!(
            names,
            [
                "\"preparing\"",
                "\"decoding\"",
                "\"loading_model\"",
                "\"analyzing\"",
                "\"clustering\"",
                "\"finalizing\""
            ]
        );
    }
}

// ---- a Pianissimo run that succeeds, against the fake engine host ---------------

/// `--model pianissimo-sv` end to end: the real binary starts
/// `sagascript-fake-engine-host` as its engine host, decodes a WAV, has the
/// host "transcribe" it and writes the result. No model or network is involved;
/// the fake host derives its tokens from the audio (see its header).
///
/// Pianissimo runs only where the platform gate lets it (macOS 14+ on Apple
/// Silicon, Windows on ARM), so this module is compiled for macOS on Apple
/// Silicon, where CI's macOS lane runs it. The engine host and the model
/// directory come from the developer overrides, which only a debug build or a
/// build with the `dev-overrides` feature honours
/// (`sagascript_core::transcription::dev_overrides::ENABLED`); a release test
/// build ignores both variables, so the module is left out of it.
#[cfg(all(
    target_os = "macos",
    target_arch = "aarch64",
    any(debug_assertions, feature = "dev-overrides")
))]
mod pianissimo_with_the_fake_host {
    use std::path::Path;

    use super::*;

    /// The fake host built by `sagascript-core`: next to the CLI binary when
    /// the workspace was built as a whole, otherwise built now (as the engine
    /// tests in `src/engine.rs` do).
    fn fake_host() -> PathBuf {
        let cli = PathBuf::from(env!("CARGO_BIN_EXE_sagascript"));
        let name = format!(
            "sagascript-fake-engine-host{}",
            std::env::consts::EXE_SUFFIX
        );
        let beside = cli.with_file_name(&name);
        if beside.is_file() {
            return beside;
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args([
                "build",
                "-p",
                "sagascript-core",
                "--bin",
                "sagascript-fake-engine-host",
                "--message-format=json",
            ])
            .stderr(Stdio::inherit())
            .output()
            .expect("run cargo build for the fake engine host");
        assert!(
            output.status.success(),
            "building the fake engine host failed"
        );
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|message| {
                message["reason"] == "compiler-artifact"
                    && message["target"]["name"] == "sagascript-fake-engine-host"
                    && message["target"]["kind"]
                        .as_array()
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            })
            .and_then(|message| message["executable"].as_str().map(PathBuf::from))
            .expect("cargo reported the fake engine host")
    }

    /// A model directory the developer override accepts: every item the engine
    /// host would load, empty.
    fn model_dir(sandbox: &Sandbox) -> PathBuf {
        let dir = sandbox.root.join("pianissimo-model");
        std::fs::create_dir_all(&dir).expect("create the model directory");
        for item in sagascript_core::transcription::pianissimo_model::active().required_items {
            std::fs::write(dir.join(item), b"").expect("write a model item");
        }
        dir
    }

    /// A 16 kHz mono 16-bit PCM WAV of `seconds` of silence.
    fn wav(seconds: usize) -> Vec<u8> {
        let samples = seconds * 16_000;
        let data_len = u32::try_from(samples * 2).expect("fixture fits a WAV");
        let mut bytes = Vec::with_capacity(44 + samples * 2);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1_u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&16_000_u32.to_le_bytes());
        bytes.extend_from_slice(&32_000_u32.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(44 + samples * 2, 0);
        bytes
    }

    fn run(sandbox: &Sandbox, host: &Path, args: &[&str]) -> Run {
        let mut command = sandbox.command(args);
        command
            .env("SAGASCRIPT_ENGINE_HOST", host)
            .env("SAGASCRIPT_PIANISSIMO_MODEL_DIR", model_dir(sandbox));
        Run::from(command.output().expect("run sagascript"))
    }

    /// The words the fake host "hears" in 3 s of silence: one token per half
    /// second, all the same.
    const HEARD: &str = "t0 t0 t0 t0 t0 t0";

    const ARGS: [&str; 6] = [
        "transcribe",
        "tone.wav",
        "--model",
        "pianissimo-sv",
        "--language",
        "sv",
    ];

    /// `ARGS` and then `extra`.
    fn with<'a>(extra: &[&'a str]) -> Vec<&'a str> {
        let mut args: Vec<&str> = ARGS.to_vec();
        args.extend_from_slice(extra);
        args
    }

    #[test]
    fn plain_text_is_the_transcript_alone_on_stdout() {
        let sandbox = Sandbox::new();
        sandbox.write("tone.wav", &wav(3));
        let run = run(&sandbox, &fake_host(), &ARGS);

        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert_eq!(run.stdout, format!("{HEARD}\n"));
        assert_lines(&sandbox.stderr_lines(&run), &["[1/1] tone.wav"], "plain");
    }

    #[test]
    fn json_is_the_per_file_object_with_engine_performance() {
        let sandbox = Sandbox::new();
        sandbox.write("tone.wav", &wav(3));
        let run = run(&sandbox, &fake_host(), &with(&["--json"]));

        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        let mut result: Value = serde_json::from_str(&run.stdout).expect("--json is one document");
        let performance = result["performance"].take();
        let words: Vec<Value> = (0..6)
            .map(|index| {
                json!({
                    "start": index as f64 * 0.5,
                    "end": index as f64 * 0.5 + 0.4,
                    "text": "t0",
                    "avg_logprob": null,
                    "no_speech_prob": null,
                    "quarantined": false,
                })
            })
            .collect();
        result["performance"] = Value::Null;
        assert_eq!(
            result,
            json!({
                "text": HEARD,
                "segments": words,
                "language": "sv",
                "model": "pianissimo-sv",
                "file": "tone.wav",
                "duration_seconds": 3.0,
                "coverage_ratio": null,
                "uncovered_spans": [],
                "repetition_spans": [],
                "detected_language": null,
                "language_redetection_enabled": false,
                "language_regions": null,
                "warnings": [],
                "vocabulary_corrections": [],
                "performance": null,
            })
        );
        // Times vary; the rest of `performance` is the engine's report.
        for key in [
            "model_load_seconds",
            "model_verification_seconds",
            "decode_resample_seconds",
            "total_seconds",
        ] {
            assert!(performance[key].as_f64().is_some(), "{key}: {performance}");
        }
        assert_eq!(performance["engine"], "fake");
        assert_eq!(performance["engine_warm"], false);
        let windows = performance["windows"].as_array().expect("windows");
        assert_eq!(windows.len(), 1, "{performance}");
        assert_eq!(windows[0]["index"], 0);
        assert_eq!(windows[0]["tokens"], 6);
        assert_eq!(windows[0]["audio_seconds"], 3.0);
    }

    #[test]
    fn a_profile_dictionarys_replacements_apply_after_transcription() {
        let sandbox = Sandbox::new();
        sandbox.write("tone.wav", &wav(3));
        sandbox.write(
            "settings.json",
            br#"{"language":"sv","profile_glossary_migrated":true,
                "hotkey_profiles":[
                  {"id":"swedish","name":"Swedish","shortcut":"Super+S","language":"sv"}],
                "profile_glossaries":{"swedish":"Tzero = t0"}}"#,
        );
        let run = run(
            &sandbox,
            &fake_host(),
            &[
                "transcribe",
                "tone.wav",
                "--model",
                "pianissimo-sv",
                "--profile",
                "swedish",
                "--json",
            ],
        );

        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        let result: Value = serde_json::from_str(&run.stdout).expect("--json is one document");
        assert_eq!(result["text"], "Tzero Tzero Tzero Tzero Tzero Tzero");
        let corrections = result["vocabulary_corrections"]
            .as_array()
            .expect("corrections");
        assert_eq!(corrections.len(), 6, "{result}");
        assert_eq!(corrections[0]["original"], "t0");
        assert_eq!(corrections[0]["replacement"], "Tzero");
        assert_lines(
            &sandbox.stderr_lines(&run),
            &[
                "Pianissimo does not use Whisper decoder hints; saved glossary replacements still apply after transcription.",
                "[1/1] tone.wav",
            ],
            "glossary",
        );
    }

    #[test]
    fn progress_events_run_decode_resample_transcribe_complete() {
        let sandbox = Sandbox::new();
        sandbox.write("tone.wav", &wav(3));
        let run = run(&sandbox, &fake_host(), &with(&["--progress-json"]));

        assert_eq!(run.code, Some(0), "stderr={}", run.stderr);
        assert_eq!(run.stdout, format!("{HEARD}\n"));
        let mut events: Vec<Value> = Vec::new();
        let rest: Vec<String> = sandbox
            .stderr_lines(&run)
            .into_iter()
            .filter(|line| match serde_json::from_str::<Value>(line) {
                Ok(event) if event["event"] == "transcription_progress" => {
                    events.push(event);
                    false
                }
                _ => true,
            })
            .collect();
        let mut phases: Vec<(String, u64)> = Vec::new();
        for event in &events {
            assert_eq!(event["steps_total"], 3, "{event}");
            let entry = (
                event["phase"].as_str().expect("phase").to_string(),
                event["step"].as_u64().expect("step"),
            );
            if phases.last() != Some(&entry) {
                phases.push(entry);
            }
        }
        let transcribing: Vec<u64> = events
            .iter()
            .filter(|event| event["phase"] == "transcribing")
            .map(|event| event["percent"].as_u64().expect("percent"))
            .collect();
        assert_eq!(rest, ["[1/1] tone.wav"]);
        assert_eq!(
            phases,
            [
                ("decoding".to_string(), 1),
                ("resampling".to_string(), 2),
                ("transcribing".to_string(), 3),
                ("completed".to_string(), 3),
            ]
        );
        assert_eq!(transcribing.first(), Some(&0));
        assert_eq!(transcribing.last(), Some(&100));
        assert!(
            transcribing.windows(2).all(|pair| pair[0] <= pair[1]),
            "{transcribing:?}"
        );
        let last = events.last().expect("events were written");
        assert_eq!(last["phase"], "completed");
        assert_eq!(last["percent"], 100);
    }

    #[test]
    fn a_batch_starts_the_engine_once_and_writes_a_header_per_file() {
        let sandbox = Sandbox::new();
        sandbox.write("a.wav", &wav(3));
        sandbox.write("b.wav", &wav(3));
        let host = fake_host();
        let base = ["--model", "pianissimo-sv", "--language", "sv"];

        let plain = run(
            &sandbox,
            &host,
            &[&["transcribe", "a.wav", "b.wav"][..], &base[..]].concat(),
        );
        assert_eq!(plain.code, Some(0), "stderr={}", plain.stderr);
        assert_eq!(
            plain.stdout,
            format!("==> a.wav <==\n{HEARD}\n==> b.wav <==\n{HEARD}\n")
        );
        assert_lines(
            &sandbox.stderr_lines(&plain),
            &["[1/2] a.wav", "[2/2] b.wav"],
            "batch",
        );

        // The engine is started and the model loaded for the first file only.
        let jsonl = run(
            &sandbox,
            &host,
            &[&["transcribe", "a.wav", "b.wav", "--jsonl"][..], &base[..]].concat(),
        );
        assert_eq!(jsonl.code, Some(0), "stderr={}", jsonl.stderr);
        let items: Vec<Value> = jsonl
            .stdout
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSONL item"))
            .collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["source"], "a.wav");
        assert_eq!(items[1]["source"], "b.wav");
        for item in &items {
            assert_eq!(item["status"], "ok");
            assert_eq!(item["result"]["text"], HEARD);
        }
        assert_eq!(items[0]["result"]["performance"]["engine_warm"], false);
        assert_eq!(items[1]["result"]["performance"]["engine_warm"], true);
        assert_eq!(items[1]["result"]["performance"]["model_load_seconds"], 0.0);
        assert_eq!(
            items[1]["result"]["performance"]["model_verification_seconds"],
            0.0
        );
    }
}
