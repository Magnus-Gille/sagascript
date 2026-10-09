//! End-to-end cache-hit coverage for diarization progress decoder provenance.
//!
//! The fixture is deliberately a serialized cache rather than audio: a valid hit must skip
//! decoding, Whisper, VAD, and the diarization models while still exercising the CLI output
//! paths that users consume. Settings are isolated with the supported override; model
//! directory discovery retains the ordinary OS profile. A valid cache hit bypasses model
//! presence checks and loading, rather than requiring a provisioned model directory.

#![cfg(feature = "diarization")]

use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const SEGMENTATION_SHA256: &str =
    "220ad67ca923bef2fa91f2390c786097bf305bceb5e261d4af67b38e938e1079";
const EMBEDDING_SHA256: &str = "7bb2f06e9df17cdf1ef14ee8a15ab08ed28e8d0ef5054ee135741560df2ec068";
const VAD_MODEL_ID: &str =
    "ggml-silero-v5.1.2.bin@29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf;threshold=0.5;min_silence_ms=200;speech_pad_ms=50;samples_overlap=0.1";

#[derive(Clone, Copy, Debug)]
struct DecoderCase {
    name: &'static str,
    beam_size: u32,
    vad: bool,
    cli_beam: Option<&'static str>,
}

const DECODER_CASES: &[DecoderCase] = &[
    DecoderCase {
        name: "greedy-default",
        beam_size: 0,
        vad: false,
        cli_beam: None,
    },
    DecoderCase {
        name: "explicit-beam",
        beam_size: 4,
        vad: false,
        cli_beam: Some("4"),
    },
    DecoderCase {
        name: "vad",
        beam_size: 0,
        vad: true,
        cli_beam: None,
    },
];

#[derive(Clone, Copy, Debug)]
enum OutputKind {
    Json,
    MeetingJson,
}

impl OutputKind {
    fn flag(self) -> &'static str {
        match self {
            Self::Json => "--json",
            Self::MeetingJson => "--meeting-json",
        }
    }
}

#[test]
fn cached_json_progress_decoder_matches_final_report_for_greedy_beam_and_vad() {
    for &decoder in DECODER_CASES {
        run_cache_hit_case(OutputKind::Json, decoder);
    }
}

#[test]
fn cached_meeting_progress_decoder_matches_final_report_for_greedy_beam_and_vad() {
    for &decoder in DECODER_CASES {
        run_cache_hit_case(OutputKind::MeetingJson, decoder);
    }
}

fn run_cache_hit_case(output_kind: OutputKind, decoder: DecoderCase) {
    let root = tempdir().expect("create fixture directory");
    let source = root.path().join("synthetic.wav");
    let cache = root.path().join("synthetic-diarization-cache.json");
    let settings = root.path().join("isolated-settings.json");
    let source_bytes = b"synthetic source bytes; cache hit must not decode this file\n";
    std::fs::write(&source, source_bytes).expect("write synthetic source");
    std::fs::write(&settings, b"{}\n").expect("write isolated settings");
    write_cache(&cache, source_bytes, decoder);
    let cache_before = std::fs::read(&cache).expect("read fixture cache");

    let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
    command.env_clear();
    // Retain ordinary platform paths required by the Windows loader and directory
    // lookup while excluding inherited provider and application credentials.
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "PATH",
        "LANG",
        "TMPDIR",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.env("SAGASCRIPT_SETTINGS_PATH", &settings).args([
        "transcribe",
        source.to_str().expect("source is UTF-8"),
        "--language",
        "en",
        "--model",
        "tiny.en",
        "--diarize",
        "--diarize-cache",
        cache.to_str().expect("cache is UTF-8"),
        "--progress-json",
        output_kind.flag(),
    ]);
    if let Some(beam) = decoder.cli_beam {
        command.args(["--beam", beam]);
    }
    if decoder.vad {
        command.arg("--vad");
    }

    let output = command.output().unwrap_or_else(|error| {
        panic!(
            "{} {} failed to start for {}: {error}",
            output_kind.flag(),
            decoder.name,
            env!("CARGO_BIN_EXE_sagascript")
        )
    });
    let stdout = String::from_utf8(output.stdout).expect("CLI stdout is UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("CLI stderr is UTF-8");
    assert!(
        output.status.success(),
        "{} {} failed: status={}\nstdout={stdout}\nstderr={stderr}",
        output_kind.flag(),
        decoder.name,
        output.status
    );

    let final_output: Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
        panic!(
            "{} {} emitted invalid JSON: {error}\nstdout={stdout}\nstderr={stderr}",
            output_kind.flag(),
            decoder.name
        )
    });
    let final_decoder = final_output
        .get("diarization")
        .and_then(|value| value.get("decoder"))
        .unwrap_or_else(|| {
            panic!(
                "{} {} final report has no diarization.decoder: {stdout}",
                output_kind.flag(),
                decoder.name
            )
        });
    assert_eq!(final_decoder["beam_size"], json!(decoder.beam_size));
    assert_eq!(final_decoder["vad_enabled"], json!(decoder.vad));
    assert_eq!(
        final_decoder["strategy"],
        json!(if decoder.beam_size >= 2 {
            "beam_search"
        } else {
            "greedy"
        })
    );

    let progress_events: Vec<Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event.get("event") == Some(&json!("transcription_progress")))
        .collect();
    let phases: Vec<&str> = progress_events
        .iter()
        .filter_map(|event| event.get("phase").and_then(Value::as_str))
        .collect();
    assert_eq!(
        phases,
        ["analyzing", "clustering", "finalizing", "completed"],
        "{} {} progress phases; stderr={stderr}",
        output_kind.flag(),
        decoder.name
    );
    assert_eq!(progress_events.len(), 4);
    for event in &progress_events {
        assert_eq!(
            event.get("decoder"),
            Some(final_decoder),
            "{} {} progress decoder differs from final report at phase {:?}; stderr={stderr}",
            output_kind.flag(),
            decoder.name,
            event.get("phase")
        );
    }

    assert!(
        stderr.contains("cache_hit=true"),
        "{} {} did not report a cache hit; stderr={stderr}",
        output_kind.flag(),
        decoder.name
    );
    assert!(stderr.contains("Reusing diarization cache"));
    for forbidden in [
        "Decoding ",
        "Loading model:",
        "Verifying Silero VAD model",
        "Running speaker diarization",
    ] {
        assert!(
            !stderr.contains(forbidden),
            "{} {} unexpectedly performed {forbidden:?}; stderr={stderr}",
            output_kind.flag(),
            decoder.name
        );
    }

    let decoder_text = serde_json::to_string(final_decoder).expect("decoder serializes");
    for forbidden in ["model", "path", "eta"] {
        assert!(
            !decoder_text.to_ascii_lowercase().contains(forbidden),
            "{} {} decoder contains forbidden {forbidden:?}: {decoder_text}",
            output_kind.flag(),
            decoder.name
        );
    }

    if matches!(output_kind, OutputKind::Json) {
        let performance = final_output
            .get("performance")
            .expect("legacy JSON includes performance");
        assert_eq!(performance["cache_hit"], json!(true));
        for timing in [
            "model_load_seconds",
            "decode_resample_seconds",
            "language_detection_seconds",
            "diarization_model_load_seconds",
            "diarization_segmentation_seconds",
            "diarization_segment_extraction_seconds",
            "diarization_embeddings_seconds",
            "whisper_inference_seconds",
            "word_timestamp_attribution_seconds",
            "parallel_analysis_span_seconds",
            "cache_write_seconds",
        ] {
            assert_eq!(
                performance[timing],
                json!(0.0),
                "{} {} cache hit should have no {timing}",
                output_kind.flag(),
                decoder.name
            );
        }
    }

    assert_eq!(
        std::fs::read(&source).expect("source remains readable"),
        source_bytes,
        "source bytes changed during cache hit"
    );
    assert_eq!(
        std::fs::read(&cache).expect("cache remains readable"),
        cache_before,
        "cache bytes changed during cache hit"
    );
}

fn write_cache(path: &std::path::Path, source: &[u8], decoder: DecoderCase) {
    let source_sha256 = sha256_hex(source);
    let empty_prompt_sha256 = sha256_hex(b"");
    let speech_frames = vec![false; 50];
    let fixture = json!({
        "identity": {
            "schema_version": 6,
            "input_sha256": source_sha256,
            "language": "en",
            "model": "tiny.en",
            "prompt_sha256": empty_prompt_sha256,
            "decoder_beam_size": decoder.beam_size,
            "temperature_fallback": true,
            "vad_model_id": decoder.vad.then_some(VAD_MODEL_ID),
        },
        "analysis_identity": {
            "segmentation_sha256": SEGMENTATION_SHA256,
            "embedding_sha256": EMBEDDING_SHA256,
            "min_segment": 0.3,
            "min_gap": 0.5,
            "exclusive_speech_embeddings": false,
        },
        "analysis": {
            "raw_segments": [[0.0, 0.5, 0]],
            "embeddings": [],
        },
        "transcript": [[0.1, 0.4, "synthetic cached text"]],
        "coverage_segments": [{
            "start": 0.1,
            "end": 0.4,
            "text": "synthetic cached text",
            "avg_logprob": null,
            "no_speech_prob": 0.0,
        }],
        "coverage_profile": {
            "audio_samples": 16_000,
            "speech_frames": speech_frames,
        },
        "detected_language": null,
        "language_regions": null,
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&fixture).expect("fixture serializes"),
    )
    .expect("write cache fixture");
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
