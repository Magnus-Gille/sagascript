//! `sagascript engine ...`: inspect and exercise the Pianissimo engine host.
//!
//! `status` reports what would run, `doctor` runs the full chain (resolve host,
//! hello, load, transcribe one second of silence) and fails loudly, `warm`
//! loads the model and holds it (benchmarks, signed-build smoke tests).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use sagascript_core::error::DictationError;
use sagascript_core::transcription::engine_host::{CancelToken, EngineHostClient, EngineHostConfig};
use sagascript_core::transcription::{pianissimo_backend, pianissimo_model};
use serde_json::{json, Value};

#[derive(Args)]
pub struct EngineArgs {
    #[command(subcommand)]
    pub action: EngineAction,
}

#[derive(Subcommand)]
pub enum EngineAction {
    /// Show the engine host, protocol and Pianissimo model state
    #[command(
        long_about = "\
Show which engine host would run Pianissimo (path and where it was found), the \
host's own build identity from its protocol handshake, the protocol version, and \
whether the model is installed and verified. The host is started only to \
read its identity and is stopped again; no model is loaded.

Pianissimo requires macOS 14+ on Apple Silicon or Windows on ARM (Snapdragon); on other systems the \
report says so and exits zero.

The host is found in the copy bundled with the app (Sagascript.app, or engine-host\\ beside the Windows executable). Development builds \
(debug, or built with the `dev-overrides` feature) also honor \
SAGASCRIPT_ENGINE_HOST and SAGASCRIPT_PIANISSIMO_MODEL_DIR (an existing model \
directory instead of the downloaded one); release builds ignore both.",
        after_long_help = "EXAMPLES:\n  sagascript engine status\n  sagascript engine status --json"
    )]
    Status {
        /// Machine-readable JSON on stdout
        #[arg(long)]
        json: bool,
    },
    /// Check that the engine works end to end
    #[command(
        long_about = "\
Run the full engine chain and report each step: platform gate, host lookup, \
model presence and verification, host start and handshake, model load, and a \
one-second silence transcription. Exits non-zero on the first failing step \
(after printing the report).

--verify-model ignores cached verification stamps and re-hashes every model \
file (slow on first run of a fresh install; use it after suspected corruption).",
        after_long_help = "EXAMPLES:\n  sagascript engine doctor\n  sagascript engine doctor --json --verify-model"
    )]
    Doctor {
        /// Machine-readable JSON on stdout
        #[arg(long)]
        json: bool,
        /// Re-hash all model files instead of trusting verification stamps
        #[arg(long)]
        verify_model: bool,
    },
    /// Load the model and hold the engine warm
    #[command(
        long_about = "\
Start the engine host, load the Pianissimo model and keep it loaded until \
Ctrl+C or --seconds elapse. Idle unloading is disabled while holding. Useful for \
benchmarking warm latency or smoke-testing a signed build.",
        after_long_help = "EXAMPLES:\n  sagascript engine warm --seconds 30"
    )]
    Warm {
        /// Stop after this many seconds (default: until Ctrl+C)
        #[arg(long, value_name = "N")]
        seconds: Option<u64>,
    },
}

pub fn run(args: EngineArgs) -> Result<(), DictationError> {
    match args.action {
        EngineAction::Status { json } => {
            let probe = Probe::detect();
            let report = status_report(&probe);
            print_status(&report, json);
            Ok(())
        }
        EngineAction::Doctor { json, verify_model } => {
            let probe = Probe::detect();
            let report = doctor_report(&probe, verify_model);
            print_doctor(&report, json);
            if report.ok {
                Ok(())
            } else {
                Err(DictationError::TranscriptionFailed(format!(
                    "engine doctor failed at step '{}'",
                    report.failed_step.as_deref().unwrap_or("unknown")
                )))
            }
        }
        EngineAction::Warm { seconds } => warm(seconds),
    }
}

// ---- inputs (injectable for tests) ----------------------------------------

/// Everything `status` and `doctor` learn from the environment, so tests can
/// substitute a fake host and skip the platform gate.
pub struct Probe {
    pub supported: bool,
    pub os: String,
    pub arch: String,
    pub host: Option<(PathBuf, &'static str)>,
    pub model_id: String,
    pub model_dir: PathBuf,
    pub model_overridden: bool,
    pub model_installed: bool,
    /// Config used to start the host (`None` when the prechecks fail).
    pub config: Option<EngineHostConfig>,
    pub verify: fn(force: bool) -> Result<(), DictationError>,
}

impl Probe {
    pub fn detect() -> Self {
        let host = pianissimo_backend::resolve_host().map(|path| {
            let source = if std::env::var_os(pianissimo_backend::ENGINE_HOST_ENV)
                .is_some_and(|value| !value.is_empty())
            {
                "env"
            } else {
                "bundled"
            };
            (path, source)
        });
        let config = host.as_ref().map(|(path, _)| {
            pianissimo_backend::config_for_host(path.clone(), &sagascript_core::settings::store::load())
        });
        Self {
            supported: pianissimo_backend::runtime_supported_on_this_os(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            host,
            model_id: pianissimo_model::model_id(),
            model_dir: pianissimo_model::model_dir(),
            model_overridden: pianissimo_model::is_overridden(),
            model_installed: pianissimo_model::is_downloaded(),
            config,
            verify: |force| {
                if force {
                    pianissimo_model::verify_downloaded_forced()
                } else {
                    pianissimo_model::verify_downloaded()
                }
            },
        }
    }
}

fn build_identity() -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "git": crate::GIT_HASH,
        "built": env!("SAGASCRIPT_CLI_BUILD_DATE"),
    })
}

fn quiet(config: &EngineHostConfig) -> EngineHostClient {
    let mut config = config.clone();
    config.idle_unload = None;
    config.idle_shutdown = None;
    EngineHostClient::new(config)
}

// ---- status ---------------------------------------------------------------

pub fn status_report(probe: &Probe) -> Value {
    let requirement = pianissimo_backend::UNSUPPORTED_MESSAGE;
    let mut host = json!({
        "path": probe.host.as_ref().map(|(p, _)| p.display().to_string()),
        "source": probe.host.as_ref().map_or("none", |(_, s)| *s),
        "exists": probe.host.as_ref().is_some_and(|(p, _)| p.is_file()),
        "hello": Value::Null,
        "error": Value::Null,
    });
    if probe.supported {
        match (&probe.host, &probe.config) {
            (Some((path, _)), Some(config)) if path.is_file() => {
                let client = quiet(config);
                match client.connect() {
                    Ok(snapshot) => {
                        host["hello"] = json!({
                            "name": snapshot.host.as_ref().map(|h| h.name.clone()),
                            "version": snapshot.host.as_ref().map(|h| h.version.clone()),
                            "git_sha": snapshot.host.as_ref().and_then(|h| h.git_sha.clone()),
                            "engine": snapshot.host.as_ref().map(|h| h.engine.clone()),
                            "engine_version": snapshot.host.as_ref().map(|h| h.engine_version.clone()),
                            "pid": snapshot.pid,
                            "capabilities": snapshot.capabilities.as_ref().map(|c| json!({
                                "sample_rate": c.sample_rate,
                                "max_window_s": c.max_window_s,
                                "preferred_window_s": c.preferred_window_s,
                                "preferred_overlap_s": c.preferred_overlap_s,
                                "max_in_flight": c.max_in_flight,
                                "compute_units": c.compute_units,
                                "min_macos": c.min_macos,
                            })),
                        });
                    }
                    Err(error) => host["error"] = json!(error.to_string()),
                }
                client.shutdown();
            }
            (Some((path, _)), _) if !path.is_file() => {
                host["error"] = json!(format!("host binary not found at {}", path.display()));
            }
            (None, _) => {
                host["error"] = json!(format!(
                    "no engine host: not bundled and {} is not set",
                    pianissimo_backend::ENGINE_HOST_ENV
                ));
            }
            _ => {}
        }
    }

    let verified: Value = if probe.model_installed {
        match (probe.verify)(false) {
            Ok(()) => json!(true),
            Err(error) => json!({ "error": error.to_string() }),
        }
    } else {
        Value::Null
    };
    json!({
        "supported": probe.supported,
        "requirement": requirement,
        "os": probe.os,
        "arch": probe.arch,
        "cli_build": build_identity(),
        "protocol": sagascript_core::transcription::engine_host::protocol::PROTOCOL_VERSION,
        "host": host,
        "model": {
            "id": probe.model_id,
            "dir": probe.model_dir.display().to_string(),
            "overridden": probe.model_overridden,
            "installed": probe.model_installed,
            "verified": verified,
        },
    })
}

fn print_status(report: &Value, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(report).unwrap());
        return;
    }
    let text = |v: &Value| v.as_str().unwrap_or("-").to_string();
    println!(
        "Engine (Pianissimo, {})",
        if cfg!(windows) { "ONNX Runtime" } else { "Core ML" }
    );
    if report["supported"] == true {
        println!("  Platform:    supported ({} {})", text(&report["os"]), text(&report["arch"]));
    } else {
        println!(
            "  Platform:    NOT supported on {} {} ({})",
            text(&report["os"]),
            text(&report["arch"]),
            text(&report["requirement"])
        );
    }
    let build = &report["cli_build"];
    println!(
        "  CLI build:   {} (git {}, built {})",
        text(&build["version"]),
        text(&build["git"]),
        text(&build["built"])
    );
    println!("  Protocol:    {}", report["protocol"]);
    let host = &report["host"];
    println!("  Host:        {} (source: {})", text(&host["path"]), text(&host["source"]));
    if let Some(hello) = host["hello"].as_object().filter(|h| !h.is_empty()) {
        println!(
            "  Host build:  {} {} git {} engine {} {}",
            hello["name"].as_str().unwrap_or("-"),
            hello["version"].as_str().unwrap_or("-"),
            hello["git_sha"].as_str().unwrap_or("-"),
            hello["engine"].as_str().unwrap_or("-"),
            hello["engine_version"].as_str().unwrap_or("")
        );
    }
    if let Some(error) = host["error"].as_str() {
        println!("  Host error:  {error}");
    }
    let model = &report["model"];
    let state = match (&model["installed"], &model["verified"]) {
        (Value::Bool(false), _) => "not installed".to_string(),
        (_, Value::Bool(true)) => "installed, verified".to_string(),
        (_, other) => format!("installed, verification FAILED: {other}"),
    };
    println!("  Model:       {} {}", text(&model["id"]), state);
    println!("  Model dir:   {}", text(&model["dir"]));
}

// ---- doctor ---------------------------------------------------------------

pub struct DoctorReport {
    pub ok: bool,
    pub failed_step: Option<String>,
    pub steps: Vec<Value>,
}

impl DoctorReport {
    fn to_json(&self) -> Value {
        json!({ "ok": self.ok, "failed_step": self.failed_step, "steps": self.steps })
    }
}

pub fn doctor_report(probe: &Probe, verify_model: bool) -> DoctorReport {
    let mut steps: Vec<Value> = Vec::new();
    let mut failed: Option<String> = None;
    let mut step = |name: &str, started: Instant, result: Result<String, String>| {
        let (ok, detail) = match result {
            Ok(detail) => (true, detail),
            Err(detail) => (false, detail),
        };
        steps.push(json!({
            "step": name,
            "ok": ok,
            "detail": detail,
            "ms": started.elapsed().as_millis() as u64,
        }));
        if !ok && failed.is_none() {
            failed = Some(name.to_string());
        }
        ok
    };

    let t = Instant::now();
    let supported = probe.supported;
    if !step(
        "platform",
        t,
        if supported {
            Ok(format!("{} {}", probe.os, probe.arch))
        } else {
            Err(format!(
                "{} ({} {})",
                pianissimo_backend::UNSUPPORTED_MESSAGE,
                probe.os,
                probe.arch
            ))
        },
    ) {
        return finish(steps, failed);
    }

    let t = Instant::now();
    let host_ok = match &probe.host {
        Some((path, source)) if path.is_file() => Ok(format!("{} ({source})", path.display())),
        Some((path, _)) => Err(format!("host binary not found at {}", path.display())),
        None => Err(format!(
            "no engine host: not bundled and {} is not set",
            pianissimo_backend::ENGINE_HOST_ENV
        )),
    };
    if !step("host", t, host_ok) {
        return finish(steps, failed);
    }

    let t = Instant::now();
    let installed = if probe.model_installed {
        Ok(probe.model_dir.display().to_string())
    } else {
        Err(format!(
            "model {} is not installed. Run: sagascript download-model pianissimo-sv",
            probe.model_id
        ))
    };
    if !step("model_installed", t, installed) {
        return finish(steps, failed);
    }

    let t = Instant::now();
    let verified = (probe.verify)(verify_model)
        .map(|()| if verify_model { "all files re-hashed".into() } else { "verified".to_string() })
        .map_err(|e| e.to_string());
    if !step("model_verified", t, verified) {
        return finish(steps, failed);
    }

    let Some(config) = &probe.config else {
        let t = Instant::now();
        step("host_start", t, Err("engine configuration is unavailable".into()));
        return finish(steps, failed);
    };
    let client = quiet(config);

    let t = Instant::now();
    let started = client.connect();
    let started_detail = started
        .as_ref()
        .map(|snapshot| {
            let host = snapshot.host.as_ref();
            format!(
                "{} {} git {} engine {}",
                host.map_or("-", |h| h.name.as_str()),
                host.map_or("-", |h| h.version.as_str()),
                host.and_then(|h| h.git_sha.as_deref()).unwrap_or("-"),
                host.map_or("-", |h| h.engine.as_str()),
            )
        })
        .map_err(|e| e.to_string());
    if !step("host_start", t, started_detail) {
        client.shutdown();
        return finish(steps, failed);
    }

    let t = Instant::now();
    let loaded = client
        .warm()
        .map(|_| {
            client
                .snapshot()
                .last_load
                .map_or("loaded".to_string(), |l| {
                    format!("load {} ms (compiled: {})", l.load_ms, l.compiled)
                })
        })
        .map_err(|e| e.to_string());
    if !step("model_load", t, loaded) {
        client.shutdown();
        return finish(steps, failed);
    }

    let t = Instant::now();
    let silence = vec![0.0_f32; 16_000];
    let transcribed = client
        .transcribe_file_samples(&silence, &CancelToken::new(), None)
        .map(|r| format!("1 s of silence -> {} chars, {} window(s)", r.text.chars().count(), r.windows.len()))
        .map_err(|e| e.to_string());
    step("transcribe", t, transcribed);

    client.shutdown();
    finish(steps, failed)
}

fn finish(steps: Vec<Value>, failed: Option<String>) -> DoctorReport {
    DoctorReport {
        ok: failed.is_none(),
        failed_step: failed,
        steps,
    }
}

fn print_doctor(report: &DoctorReport, json: bool) {
    if json {
        println!("{}", serde_json::to_string_pretty(&report.to_json()).unwrap());
        return;
    }
    for step in &report.steps {
        println!(
            "[{}] {:<15} {} ({} ms)",
            if step["ok"] == true { " ok " } else { "FAIL" },
            step["step"].as_str().unwrap_or("?"),
            step["detail"].as_str().unwrap_or(""),
            step["ms"]
        );
    }
    println!("{}", if report.ok { "Engine OK." } else { "Engine check FAILED." });
}

// ---- warm -----------------------------------------------------------------

fn warm(seconds: Option<u64>) -> Result<(), DictationError> {
    let config = pianissimo_backend::engine_config()?;
    pianissimo_model::verify_downloaded()?;
    let client = quiet(&config);
    let started = Instant::now();
    client.warm().map_err(|e| DictationError::TranscriptionFailed(e.to_string()))?;
    let snapshot = client.snapshot();
    eprintln!(
        "Engine warm in {:.2} s (host pid {}). {}",
        started.elapsed().as_secs_f64(),
        snapshot.pid.map_or("?".into(), |p| p.to_string()),
        match seconds {
            Some(n) => format!("Holding for {n} s..."),
            None => "Holding until Ctrl+C...".to_string(),
        }
    );
    let deadline = seconds.map(|n| Instant::now() + Duration::from_secs(n));
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        // Detect a host that died while holding; Ctrl+C ends the process and the
        // host exits on stdin EOF.
        if !client.snapshot().running {
            client.shutdown();
            return Err(DictationError::TranscriptionFailed("engine host exited while warm".into()));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    client.shutdown();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_core::transcription::engine_host::LoadSpec;
    use std::path::Path;

    /// The fake host lives in sagascript-core's bins; it is built by core's own
    /// tests, so build it on demand when running this crate alone.
    fn fake_host() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
        let name = format!("sagascript-fake-engine-host{}", std::env::consts::EXE_SUFFIX);
        let path = profile_dir.join(&name);
        if !path.is_file() {
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = std::process::Command::new(cargo)
                .args(["build", "-p", "sagascript-core", "--bin", "sagascript-fake-engine-host"])
                .status()
                .expect("run cargo build for the fake engine host");
            assert!(status.success(), "building the fake engine host failed");
        }
        assert!(path.is_file(), "fake host missing at {}", path.display());
        path
    }

    fn probe(host: PathBuf, supported: bool, installed: bool) -> Probe {
        let config = EngineHostConfig::new(
            host.clone(),
            LoadSpec {
                model_dir: "/models/ok".into(),
                model_id: "pianissimo-sv-coreml-test".into(),
                compute_units: "cpu".into(),
            },
        );
        Probe {
            supported,
            os: "testos".into(),
            arch: "testarch".into(),
            host: Some((host, "env")),
            model_id: "pianissimo-sv-coreml-test".into(),
            model_dir: "/models/ok".into(),
            model_overridden: false,
            model_installed: installed,
            config: Some(config),
            verify: |force| {
                if force && std::env::var_os("ENGINE_TEST_FORCE_FAILS").is_some() {
                    Err(DictationError::ModelDownloadFailed("hash mismatch".into()))
                } else {
                    Ok(())
                }
            },
        }
    }

    #[test]
    fn status_json_reports_host_identity_protocol_and_model_state() {
        let report = status_report(&probe(fake_host(), true, true));

        assert_eq!(report["supported"], true);
        assert_eq!(report["protocol"], 1);
        assert_eq!(report["host"]["source"], "env");
        assert_eq!(report["host"]["exists"], true);
        assert_eq!(report["host"]["hello"]["engine"], "fake");
        assert_eq!(
            report["host"]["hello"]["git_sha"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(report["host"]["error"], Value::Null);
        assert_eq!(report["model"]["installed"], true);
        assert_eq!(report["model"]["verified"], true);
        assert_eq!(report["cli_build"]["version"], env!("CARGO_PKG_VERSION"));
        assert!(!report["cli_build"]["git"].as_str().unwrap().is_empty());
    }

    #[test]
    fn status_on_unsupported_system_says_so_without_starting_a_host() {
        let report = status_report(&probe(PathBuf::from("/nonexistent/host"), false, false));
        assert_eq!(report["supported"], false);
        assert!(report["requirement"].as_str().unwrap().contains("macOS 14"));
        assert_eq!(report["host"]["hello"], Value::Null);
        assert_eq!(report["model"]["installed"], false);
        assert_eq!(report["model"]["verified"], Value::Null);
    }

    #[test]
    fn status_reports_a_missing_host_binary() {
        let report = status_report(&probe(PathBuf::from("/nonexistent/host"), true, true));
        assert_eq!(report["host"]["exists"], false);
        assert!(report["host"]["error"].as_str().unwrap().contains("not found"));
    }

    #[test]
    fn doctor_passes_every_step_against_the_fake_host() {
        let report = doctor_report(&probe(fake_host(), true, true), false);

        assert!(report.ok, "{:?}", report.steps);
        let names: Vec<_> = report.steps.iter().map(|s| s["step"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["platform", "host", "model_installed", "model_verified", "host_start", "model_load", "transcribe"]
        );
        assert!(report.steps.iter().all(|s| s["ok"] == true));
        let json = report.to_json();
        assert_eq!(json["ok"], true);
        assert_eq!(json["failed_step"], Value::Null);
    }

    #[test]
    fn doctor_stops_at_the_first_failing_step() {
        let report = doctor_report(&probe(fake_host(), true, false), false);
        assert!(!report.ok);
        assert_eq!(report.failed_step.as_deref(), Some("model_installed"));
        assert_eq!(report.steps.len(), 3);

        let report = doctor_report(&probe(fake_host(), false, true), false);
        assert_eq!(report.failed_step.as_deref(), Some("platform"));
        assert_eq!(report.steps.len(), 1);

        let report = doctor_report(&probe(PathBuf::from("/nonexistent/host"), true, true), false);
        assert_eq!(report.failed_step.as_deref(), Some("host"));
    }

    #[test]
    fn doctor_reports_a_host_that_cannot_start() {
        // A regular file that is not executable as a host.
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("host");
        std::fs::write(&bogus, b"not a program").unwrap();
        let report = doctor_report(&probe(bogus, true, true), false);
        assert_eq!(report.failed_step.as_deref(), Some("host_start"));
    }

    #[test]
    fn verify_model_flag_reaches_the_forced_verifier() {
        // The probe's verifier fails only for forced verification.
        std::env::set_var("ENGINE_TEST_FORCE_FAILS", "1");
        let normal = doctor_report(&probe(fake_host(), true, true), false);
        let forced = doctor_report(&probe(fake_host(), true, true), true);
        std::env::remove_var("ENGINE_TEST_FORCE_FAILS");
        assert!(normal.ok, "{:?}", normal.steps);
        assert_eq!(forced.failed_step.as_deref(), Some("model_verified"));
    }
}
