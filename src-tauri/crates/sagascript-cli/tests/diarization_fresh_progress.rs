//! Fresh diarization progress provenance regression.
//!
//! This test intentionally runs only when explicitly requested because it needs a real audio
//! public fixture and locally installed Whisper, Silero VAD, and diarization models. Before
//! starting the CLI it verifies the pinned VAD artifact, refusing a missing/corrupt file.
//! Cargo offline mode alone does not constrain the spawned CLI. The test does not use a
//! cache, so it exercises the decode, model loading, language detection, analysis, clustering,
//! and finalization phases.
//!
//! Run it with the repository's offline target and the provisioned local artifacts:
//!
//! ```text
//! SAGASCRIPT_PROGRESS_TEST_FILE=/path/to/public-fixture.wav \
//! SAGASCRIPT_PROGRESS_TEST_MODEL=tiny.en \
//! SAGASCRIPT_PROGRESS_TEST_LANGUAGE=en \
//! cargo test --offline -p sagascript-cli --test diarization_fresh_progress -- \
//!   --ignored --exact fresh_progress::fresh_diarization_progress_decoder_matches_final_report
//! ```

#[cfg(feature = "diarization")]
mod fresh_progress {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    const REQUIRED_PHASES: [&str; 8] = [
        "decoding",
        "resampling",
        "loading",
        "language_detection",
        "analyzing",
        "clustering",
        "finalizing",
        "completed",
    ];

    // Keep ordinary OS paths for provisioned local models and the Windows loader;
    // exclude provider/application credentials. Only settings are redirected.
    const ORDINARY_OS_ENV: [&str; 14] = [
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
        "XDG_DATA_HOME",
    ];

    #[test]
    #[ignore = "requires a provisioned audio fixture and local inference models"]
    fn fresh_diarization_progress_decoder_matches_final_report() {
        let fixture = required_path("SAGASCRIPT_PROGRESS_TEST_FILE");
        let model = optional_nonempty("SAGASCRIPT_PROGRESS_TEST_MODEL", "tiny.en");
        let language = optional_nonempty("SAGASCRIPT_PROGRESS_TEST_LANGUAGE", "en");
        let source_before = std::fs::read(&fixture)
            .unwrap_or_else(|error| panic!("cannot read SAGASCRIPT_PROGRESS_TEST_FILE: {error}"));
        let source_sha256 = sha256_hex(&source_before);
        let vad = sagascript_core::transcription::model::vad_model_path();
        sagascript_core::transcription::model::verify_vad_model(&vad)
            .expect("fresh regression requires the pinned local VAD model before CLI start");

        let root = tempdir().expect("create isolated settings directory");
        let settings = root.path().join("settings.json");
        std::fs::write(&settings, b"{}\n").expect("write isolated settings");

        for output_kind in [OutputKind::Json, OutputKind::MeetingJson] {
            let output = run_cli(&fixture, &settings, &model, &language, output_kind);
            assert_fresh_result(
                &output,
                output_kind,
                &model,
                &source_sha256,
                &source_before,
                &fixture,
            );
        }
    }

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

    fn required_path(name: &str) -> PathBuf {
        let value = std::env::var_os(name)
            .unwrap_or_else(|| panic!("{name} is required when running this ignored test"));
        assert!(!value.is_empty(), "{name} must not be empty");
        PathBuf::from(value)
    }

    fn optional_nonempty(name: &str, default: &str) -> String {
        match std::env::var_os(name) {
            None => default.to_owned(),
            Some(value) => {
                assert!(!value.is_empty(), "{name} must not be empty when supplied");
                value.to_string_lossy().into_owned()
            }
        }
    }

    fn run_cli(
        fixture: &Path,
        settings: &Path,
        model: &str,
        language: &str,
        output_kind: OutputKind,
    ) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sagascript"));
        command.env_clear();
        for name in ORDINARY_OS_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("SAGASCRIPT_SETTINGS_PATH", settings)
            .args([
                "transcribe",
                fixture.to_str().expect("fixture path is UTF-8"),
                "--language",
                language,
                "--model",
                model,
                "--diarize",
                "--progress-json",
                "--beam",
                "2",
                "--vad",
                output_kind.flag(),
            ])
            .output()
            .unwrap_or_else(|error| panic!("failed to start fresh CLI run: {error}"))
    }

    fn assert_fresh_result(
        output: &Output,
        output_kind: OutputKind,
        model: &str,
        source_sha256: &str,
        source_before: &[u8],
        fixture: &Path,
    ) {
        let output_summary = format!(
            "status={} stdout_bytes={} stdout_sha256={} stderr_bytes={} stderr_sha256={}",
            output.status,
            output.stdout.len(),
            sha256_hex(&output.stdout),
            output.stderr.len(),
            sha256_hex(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{} fresh run failed for model {model}: {output_summary}",
            output_kind.flag()
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{} fresh run emitted invalid JSON: {error}; {output_summary}",
                output_kind.flag()
            )
        });
        let final_decoder = report
            .get("diarization")
            .and_then(|value| value.get("decoder"))
            .unwrap_or_else(|| {
                panic!(
                    "{} fresh report has no diarization.decoder; {output_summary}",
                    output_kind.flag()
                )
            });
        assert_eq!(report["diarization"]["source_sha256"], json!(source_sha256));
        assert_eq!(final_decoder["beam_size"], json!(2));
        assert_eq!(final_decoder["strategy"], json!("beam_search"));
        assert_eq!(final_decoder["vad_enabled"], json!(true));
        assert_eq!(
            final_decoder["timestamp_method"],
            json!("source_mapped_segments")
        );

        let progress_events: Vec<Value> = stderr
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|event| event.get("event") == Some(&json!("transcription_progress")))
            .collect();
        let expected_keys: BTreeSet<&str> = [
            "event",
            "phase",
            "step",
            "steps_total",
            "percent",
            "elapsed_ms",
            "decoder",
        ]
        .into_iter()
        .collect();
        let mut phases = Vec::new();
        for event in &progress_events {
            let keys: BTreeSet<&str> = event
                .as_object()
                .expect("progress object")
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, expected_keys, "unexpected progress field names");
            let phase = event
                .get("phase")
                .and_then(Value::as_str)
                .expect("progress event has a phase");
            if phases.last().copied() != Some(phase) {
                phases.push(phase);
            }
            assert!(
                event.get("decoder") == Some(final_decoder),
                "{} fresh progress decoder differs from final report at {phase}; {output_summary}",
                output_kind.flag()
            );
            if matches!(
                phase,
                "analyzing" | "clustering" | "finalizing" | "completed"
            ) {
                assert!(
                    event["percent"].is_null()
                        && event["step"].is_null()
                        && event["steps_total"].is_null(),
                    "meeting phase invented overall completion"
                );
            }
        }
        assert_eq!(
            phases,
            REQUIRED_PHASES,
            "{} fresh progress phases; {output_summary}",
            output_kind.flag()
        );
        assert!(
            progress_events.len() >= REQUIRED_PHASES.len(),
            "{} fresh run emitted too few progress events; {output_summary}",
            output_kind.flag()
        );

        for key in final_decoder.as_object().expect("decoder object").keys() {
            let key = key.to_ascii_lowercase();
            assert!(
                !["model", "path", "eta"].contains(&key.as_str())
                    && !key.ends_with("_path")
                    && !key.starts_with("eta_"),
                "decoder contains forbidden field {key:?}"
            );
        }

        let source_after = std::fs::read(fixture).expect("fixture remains readable");
        assert!(
            source_after == source_before,
            "fixture bytes changed during run"
        );
        assert_eq!(sha256_hex(&source_after), source_sha256);
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
}
