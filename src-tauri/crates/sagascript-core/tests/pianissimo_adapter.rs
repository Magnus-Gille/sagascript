//! `PianissimoBackend` / `LiveDictationBackend` over the fake engine host:
//! progress, cancellation and warm reuse (no real model, no network).

use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::engine_host::{
    EngineHostClient, EngineHostConfig, LoadSpec, Timeouts,
};
use sagascript_core::transcription::live_dictation::LiveDictationBackend;
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;
use sagascript_core::transcription::whisper_backend::DictationTimings;
use sagascript_core::transcription::{TranscribeOptions, WhisperBackend};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FAKE: &str = env!("CARGO_BIN_EXE_sagascript-fake-engine-host");
const SR: usize = 16_000;

fn client(env: &[(&str, &str)], tmp: &Path) -> EngineHostClient {
    let mut cfg = EngineHostConfig::new(
        FAKE,
        LoadSpec {
            model_dir: "/models/ok".into(),
            model_id: "fake-model".into(),
            compute_units: "cpu".into(),
        },
    );
    cfg.env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    cfg.timeouts = Timeouts {
        hello: Duration::from_secs(5),
        load: Duration::from_secs(5),
        ping: Duration::from_millis(500),
        status: Duration::from_secs(2),
        unload: Duration::from_secs(2),
        transcribe_min: Duration::from_secs(10),
        transcribe_factor: 0.0,
        shutdown_grace: Duration::from_millis(500),
    };
    cfg.idle_unload = None;
    cfg.idle_shutdown = None;
    cfg.temp_dir = Some(tmp.to_path_buf());
    EngineHostClient::new(cfg)
}

/// The fake host derives token ids from the sample value (= absolute index).
fn ramp(seconds: usize) -> Vec<f32> {
    (0..seconds * SR).map(|i| i as f32).collect()
}

fn spawn_count(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with("hello "))
        .count()
}

#[test]
fn file_transcription_reports_real_progress_and_words() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = PianissimoBackend::with_client(client(&[("FAKE_WINDOW_S", "30")], tmp.path()));
    let seen = Mutex::new(Vec::new());

    let result = backend
        .transcribe(&ramp(100), |p| seen.lock().unwrap().push(p))
        .unwrap();

    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.first(), Some(&0));
    assert_eq!(seen.last(), Some(&100));
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "monotonic: {seen:?}");
    assert!(
        seen.iter().any(|p| *p > 0 && *p < 100),
        "real intermediate progress, not just 0/100: {seen:?}"
    );
    assert!(!result.text.is_empty());
    assert!(!result.words.is_empty());
    assert!(result.words.windows(2).all(|w| w[0].start <= w[1].start));
}

#[test]
fn caller_cancel_flag_stops_a_running_file_job_quickly() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = Arc::new(PianissimoBackend::with_client(client(
        &[("FAKE_WINDOW_S", "30"), ("FAKE_WINDOW_DELAY_MS", "2000")],
        tmp.path(),
    )));
    let cancelled = Arc::new(AtomicBool::new(false));
    let job = {
        let backend = backend.clone();
        let cancelled = cancelled.clone();
        std::thread::spawn(move || backend.transcribe_with_cancel(&ramp(100), |_| {}, &cancelled))
    };
    std::thread::sleep(Duration::from_millis(400));
    let at = Instant::now();
    cancelled.store(true, Ordering::SeqCst);

    let error = job.join().unwrap().unwrap_err();

    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
}

#[test]
fn request_abort_cancels_the_active_job() {
    let tmp = tempfile::tempdir().unwrap();
    let backend = Arc::new(PianissimoBackend::with_client(client(
        &[("FAKE_WINDOW_S", "30"), ("FAKE_WINDOW_DELAY_MS", "2000")],
        tmp.path(),
    )));
    let job = {
        let backend = backend.clone();
        std::thread::spawn(move || backend.transcribe(&ramp(100), |_| {}))
    };
    std::thread::sleep(Duration::from_millis(400));
    let at = Instant::now();
    backend.request_abort();

    let error = job.join().unwrap().unwrap_err();

    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(at.elapsed() < Duration::from_secs(2));
}

#[test]
fn cancellation_before_start_never_spawns_the_host() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("log");
    let backend = PianissimoBackend::with_client(client(
        &[("FAKE_LOG", log.to_str().unwrap())],
        tmp.path(),
    ));
    let error = backend
        .transcribe_with_cancel(&ramp(2), |_| {}, &AtomicBool::new(true))
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(spawn_count(&log), 0);
}

fn live(client: &EngineHostClient) -> LiveDictationBackend {
    LiveDictationBackend::with_engine(Arc::new(WhisperBackend::new()), client.clone())
}

fn dictate(backend: &LiveDictationBackend) -> (Result<String, sagascript_core::error::DictationError>, DictationTimings) {
    let mut timings = DictationTimings::default();
    let result = backend.transcribe(
        WhisperModel::Base,
        &ramp(3),
        Language::Swedish,
        &TranscribeOptions::default(),
        &mut timings,
    );
    (result, timings)
}

#[test]
fn second_utterance_reuses_the_warm_host() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("log");
    let shared = client(
        &[("FAKE_LOG", log.to_str().unwrap()), ("FAKE_LOAD_DELAY_MS", "300")],
        tmp.path(),
    );

    // One LiveDictationBackend per utterance, as the app does.
    let (first, first_timings) = dictate(&live(&shared));
    let (second, second_timings) = dictate(&live(&shared));

    assert!(!first.unwrap().is_empty());
    assert!(!second.unwrap().is_empty());
    assert!(!first_timings.model_cached);
    assert!(first_timings.model_ms >= 250.0, "{}", first_timings.model_ms);
    assert!(second_timings.model_cached, "warm host reported as cold");
    assert!(second_timings.model_ms < 100.0, "{}", second_timings.model_ms);
    assert!(second_timings.inference_started);
    assert_eq!(spawn_count(&log), 1, "host must not respawn between utterances");
}

#[test]
fn live_cancel_is_sticky_and_needs_no_engine() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("log");
    let shared = client(&[("FAKE_LOG", log.to_str().unwrap())], tmp.path());
    let backend = live(&shared);
    backend.request_abort();

    let (result, timings) = dictate(&backend);

    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(!timings.inference_started);
    assert_eq!(spawn_count(&log), 0);
}

#[test]
fn live_abort_during_inference_cancels_promptly() {
    let tmp = tempfile::tempdir().unwrap();
    let shared = client(&[("FAKE_WINDOW_DELAY_MS", "3000")], tmp.path());
    let backend = Arc::new(live(&shared));
    let job = {
        let backend = backend.clone();
        std::thread::spawn(move || dictate(&backend).0)
    };
    std::thread::sleep(Duration::from_millis(500));
    let at = Instant::now();
    backend.request_abort();

    let error = job.join().unwrap().unwrap_err();

    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(at.elapsed() < Duration::from_secs(2), "{:?}", at.elapsed());
}

#[test]
fn live_abort_during_model_load_returns_promptly_and_keeps_the_host() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("log");
    let shared = client(
        &[("FAKE_LOAD_DELAY_MS", "3000"), ("FAKE_LOG", log.to_str().unwrap())],
        tmp.path(),
    );
    let backend = Arc::new(live(&shared));
    let job = {
        let backend = backend.clone();
        std::thread::spawn(move || dictate(&backend))
    };
    std::thread::sleep(Duration::from_millis(500));
    let at = Instant::now();
    backend.request_abort();

    let (result, timings) = job.join().unwrap();

    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(at.elapsed() < Duration::from_millis(250), "{:?}", at.elapsed());
    assert!(!timings.inference_started);
    // The host survives so the first-use load can finish for the next utterance.
    assert!(shared.snapshot().running, "host must not be killed");
    assert_eq!(spawn_count(&log), 1);
}
