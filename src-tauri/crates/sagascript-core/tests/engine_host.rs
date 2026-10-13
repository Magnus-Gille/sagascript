//! Engine-host client tests against the deterministic fake host
//! (`tests/support/fake_engine_host.rs`); no real model is involved.

use sagascript_core::transcription::engine_host::client::WindowRequest;
use sagascript_core::transcription::engine_host::{
    CancelToken, EngineHostClient, EngineHostConfig, EngineHostError, ErrorCode, LoadEvent,
    LoadObserver, LoadSpec, Priority, Timeouts,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FAKE: &str = env!("CARGO_BIN_EXE_sagascript-fake-engine-host");
const SR: usize = 16_000;

struct Fixture {
    client: EngineHostClient,
    tmp: tempfile::TempDir,
}

fn config(env: &[(&str, &str)], tmp: &Path) -> EngineHostConfig {
    let mut cfg = EngineHostConfig::new(
        FAKE,
        LoadSpec {
            model_dir: "/models/ok".into(),
            model_id: "fake-model".into(),
            compute_units: "cpu".into(),
        },
    );
    cfg.env = env
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    cfg.timeouts = Timeouts {
        hello: Duration::from_secs(5),
        load: Duration::from_secs(5),
        ping: Duration::from_millis(500),
        status: Duration::from_secs(2),
        unload: Duration::from_secs(2),
        transcribe_min: Duration::from_secs(5),
        transcribe_factor: 0.0,
        shutdown_grace: Duration::from_millis(500),
        write: Duration::from_secs(2),
        wedge_grace: Duration::from_millis(500),
    };
    cfg.idle_unload = None;
    cfg.idle_shutdown = None;
    cfg.restart_backoff = vec![
        Duration::ZERO,
        Duration::from_millis(200),
        Duration::from_millis(400),
    ];
    cfg.busy_retries = 50;
    cfg.busy_retry_delay = Duration::from_millis(20);
    cfg.temp_dir = Some(tmp.to_path_buf());
    cfg
}

fn fixture_with(env: &[(&str, &str)], tweak: impl FnOnce(&mut EngineHostConfig)) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(env, tmp.path());
    tweak(&mut cfg);
    Fixture {
        client: EngineHostClient::new(cfg),
        tmp,
    }
}

fn fixture(env: &[(&str, &str)]) -> Fixture {
    fixture_with(env, |_| {})
}

/// Sample `i` holds the value `i`, so the fake host derives absolute token ids.
fn pcm(seconds: f64) -> Vec<f32> {
    (0..(seconds * SR as f64) as usize)
        .map(|i| i as f32)
        .collect()
}

fn expected_ids(n_samples: usize) -> Vec<u32> {
    (0..n_samples.div_ceil(8000) as u32).collect()
}

fn temp_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir).unwrap().count()
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    // `tasklist` prints the process as a CSV row containing "<pid>" while it exists.
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
        .unwrap_or(false)
}

fn wait_until(what: &str, limit: Duration, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

const LONG_ENV: [(&str, &str); 2] = [("FAKE_WINDOW_S", "10"), ("FAKE_OVERLAP_S", "4")];

// ------------------------------------------------------------- handshake

#[test]
fn handshake_accepts_and_exposes_capabilities() {
    let f = fixture(&[]);
    let caps = f.client.warm().unwrap();
    assert_eq!(caps.sample_rate, 16000);
    assert_eq!(caps.max_in_flight, 2);
    let snap = f.client.snapshot();
    assert!(snap.running && snap.loaded);
    assert_eq!(
        f.client.status().unwrap().model_id.as_deref(),
        Some("fake-model")
    );
    f.client.ping().unwrap();
}

fn recording_observer() -> (LoadObserver, Arc<std::sync::Mutex<Vec<LoadEvent>>>) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = events.clone();
    (
        LoadObserver(Arc::new(move |event| sink.lock().unwrap().push(event))),
        events,
    )
}

#[test]
fn load_observer_reports_started_then_ready_once_and_nothing_when_warm() {
    let (observer, events) = recording_observer();
    let f = fixture_with(&[], |c| c.load_observer = Some(observer));
    f.client.warm().unwrap();
    {
        let seen = events.lock().unwrap();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert_eq!(seen[0], LoadEvent::Started);
        assert!(matches!(seen[1], LoadEvent::Ready { .. }), "{seen:?}");
    }
    // Already loaded: the fast path reports no new load.
    f.client.warm().unwrap();
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[test]
fn load_observer_reports_failure() {
    let (observer, events) = recording_observer();
    let f = fixture_with(&[("FAKE_PROTOCOL", "2")], |c| {
        c.load_observer = Some(observer)
    });
    assert!(f.client.warm().is_err());
    let seen = events.lock().unwrap();
    assert_eq!(seen[0], LoadEvent::Started);
    assert!(
        matches!(seen.last(), Some(LoadEvent::Failed(_))),
        "{seen:?}"
    );
}

#[test]
fn load_observer_reports_a_terminal_event_when_the_wait_is_cancelled() {
    let (observer, events) = recording_observer();
    let f = fixture_with(&[("FAKE_LOAD_DELAY_MS", "1500")], |c| {
        c.load_observer = Some(observer)
    });
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            cancel.cancel();
        })
    };
    let r = f.client.warm_with_cancel(&cancel);
    canceller.join().unwrap();
    assert!(matches!(r, Err(EngineHostError::Cancelled)), "{r:?}");
    let seen = events.lock().unwrap();
    assert_eq!(seen.first(), Some(&LoadEvent::Started), "{seen:?}");
    assert_eq!(seen.last(), Some(&LoadEvent::Cancelled), "{seen:?}");
}

#[test]
fn unload_if_idle_frees_a_loaded_model_and_the_next_use_reloads() {
    let f = fixture(&[]);
    assert!(!f.client.unload_if_idle(), "nothing loaded yet");
    f.client.warm().unwrap();
    assert!(f.client.unload_if_idle());
    assert!(!f.client.snapshot().loaded);
    assert!(!f.client.unload_if_idle(), "already unloaded");
    f.client.warm().unwrap();
    assert!(f.client.snapshot().loaded);
}

#[test]
fn handshake_rejects_protocol_mismatch() {
    let f = fixture(&[("FAKE_PROTOCOL", "2")]);
    match f.client.warm() {
        Err(EngineHostError::Handshake(m)) => assert!(m.contains("protocol 2"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(!f.client.snapshot().running);
}

#[test]
fn handshake_rejects_missing_git_sha() {
    let f = fixture(&[("FAKE_NO_SHA", "1")]);
    match f.client.warm() {
        Err(EngineHostError::Handshake(m)) => assert!(m.contains("git_sha"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn garbage_stdout_lines_are_ignored() {
    let f = fixture(&[("FAKE_GARBAGE", "1")]);
    f.client.warm().unwrap();
    let out = f
        .client
        .transcribe_dictation(&pcm(2.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 4);
}

#[test]
fn model_missing_is_reported_as_remote_error() {
    let f = fixture_with(&[], |c| c.load.model_dir = "/models/missing".into());
    match f.client.warm() {
        Err(EngineHostError::Remote {
            code: ErrorCode::ModelMissing,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    // Not a crash: the host stays up and nothing is counted against the budget.
    let snap = f.client.snapshot();
    assert!(snap.running && !snap.loaded);
    assert_eq!(snap.crashes_in_window, 0);
}

// ----------------------------------------------------- routing / timeouts

#[test]
fn out_of_order_responses_are_routed_by_id() {
    // 3 host slots => batch lane of 2 (one slot stays reserved for dictation).
    let f = fixture(&[("FAKE_FIRST_SLOW_MS", "400"), ("FAKE_MAX_IN_FLIGHT", "3")]);
    let samples = pcm(4.0);
    let pcm_file = sagascript_core::transcription::engine_host::pipeline::PcmFile::write(
        &samples,
        Some(f.tmp.path()),
    )
    .unwrap();
    let cancel = CancelToken::new();
    let req = |offset: u64| WindowRequest {
        pcm_path: pcm_file.path().to_path_buf(),
        offset_samples: offset,
        num_samples: 32_000,
        sample_rate: 16_000,
        priority: Priority::Batch,
        boost: None,
    };
    let first = f
        .client
        .submit_window(&req(0), &cancel, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    let second = f
        .client
        .submit_window(&req(32_000), &cancel, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    let t = Instant::now();
    let b = second.wait(&cancel).unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(300),
        "second must not wait for first"
    );
    let a = first.wait(&cancel).unwrap();
    assert_eq!(a.tokens[0].id, 0);
    assert_eq!(b.tokens[0].id, 4);
}

#[test]
fn window_timeout_is_reported_and_host_stays_usable() {
    // Slow first window that honours cancel: a responsive host, not a wedged one.
    let f = fixture_with(&[("FAKE_FIRST_SLOW_MS", "5000")], |c| {
        c.timeouts.transcribe_min = Duration::from_millis(300)
    });
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid;
    let t = Instant::now();
    match f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
    {
        Err(EngineHostError::Timeout {
            op: "transcribe_window",
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert!(t.elapsed() < Duration::from_secs(3));
    // The host answered the cancelled window within the wedge grace, so it is
    // kept: same process, no crash, and the next window runs on it.
    let snap = f.client.snapshot();
    assert!(snap.running);
    assert_eq!(snap.pid, pid);
    assert_eq!(snap.crashes_in_window, 0);
    f.client.ping().unwrap();
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
    assert_eq!(f.client.snapshot().pid, pid);
}

/// A host that never answers a timed-out window, even after `cancel`, is
/// killed after the wedge grace although it still answers `ping`.
#[test]
fn host_that_never_answers_a_timed_out_window_is_killed() {
    let f = fixture_with(&[("FAKE_HANG", "1")], |c| {
        c.timeouts.transcribe_min = Duration::from_millis(300);
        c.timeouts.wedge_grace = Duration::from_millis(300);
    });
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    let r = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new());
    assert!(
        matches!(
            r,
            Err(EngineHostError::Timeout {
                op: "transcribe_window",
                ..
            })
        ),
        "{r:?}"
    );
    wait_until("hung host killed", Duration::from_secs(2), || {
        !pid_alive(pid)
    });
    assert_eq!(f.client.snapshot().crashes_in_window, 1);
}

#[test]
fn load_timeout_is_reported() {
    let f = fixture_with(&[("FAKE_LOAD_DELAY_MS", "2000")], |c| {
        c.timeouts.load = Duration::from_millis(200)
    });
    match f.client.warm() {
        Err(EngineHostError::Timeout { op: "load", .. }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn busy_responses_are_retried() {
    let f = fixture(&[
        ("FAKE_MAX_IN_FLIGHT", "1"),
        ("FAKE_ADVERTISE_IN_FLIGHT", "2"),
        ("FAKE_BUSY", "1"),
        ("FAKE_WINDOW_S", "10"),
        ("FAKE_OVERLAP_S", "4"),
        ("FAKE_WINDOW_DELAY_MS", "60"),
    ]);
    let samples = pcm(30.0);
    let out = f
        .client
        .transcribe_file_samples(&samples, &CancelToken::new(), None)
        .unwrap();
    let ids: Vec<u32> = out.tokens.iter().map(|t| t.id).collect();
    assert_eq!(ids, expected_ids(samples.len()));
}

// ------------------------------------------------------------- cancel

fn submit_slow(
    f: &Fixture,
    cancel: &CancelToken,
) -> sagascript_core::transcription::engine_host::client::WindowTicket {
    let samples = pcm(2.0);
    let file = sagascript_core::transcription::engine_host::pipeline::PcmFile::write(
        &samples,
        Some(f.tmp.path()),
    )
    .unwrap();
    let ticket = f
        .client
        .submit_window(
            &WindowRequest {
                pcm_path: file.path().to_path_buf(),
                offset_samples: 0,
                num_samples: samples.len() as u64,
                sample_rate: 16_000,
                priority: Priority::Batch,
                boost: None,
            },
            cancel,
            Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
    std::mem::forget(file); // keep the file alive for the host; the tmp dir cleans up
    ticket
}

#[test]
fn cancel_cooperative_stops_host_work() {
    let f = fixture(&[("FAKE_BATCH_DELAY_MS", "3000")]);
    let cancel = CancelToken::new();
    let ticket = submit_slow(&f, &cancel);
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        c2.cancel();
    });
    let t = Instant::now();
    assert!(matches!(
        ticket.wait(&cancel),
        Err(EngineHostError::Cancelled)
    ));
    assert!(t.elapsed() < Duration::from_secs(1));
    wait_until(
        "host in_flight to drop to 0",
        Duration::from_secs(2),
        || f.client.status().map(|s| s.in_flight == 0).unwrap_or(false),
    );
}

#[test]
fn cancel_ignored_by_host_still_returns_cancelled_in_bound() {
    let f = fixture(&[("FAKE_BATCH_DELAY_MS", "1500"), ("FAKE_IGNORE_CANCEL", "1")]);
    let cancel = CancelToken::new();
    let ticket = submit_slow(&f, &cancel);
    cancel.cancel();
    let t = Instant::now();
    assert!(matches!(
        ticket.wait(&cancel),
        Err(EngineHostError::Cancelled)
    ));
    assert!(t.elapsed() < Duration::from_millis(500));
    // The host really did keep working.
    assert_eq!(f.client.status().unwrap().in_flight, 1);
}

// -------------------------------------------------------------- crashes

#[test]
fn crash_mid_request_fails_with_stderr_tail() {
    let f = fixture_with(&[("FAKE_CRASH_ON_WINDOW", "1")], |c| {
        c.max_crash_retries = 0
    });
    match f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
    {
        Err(EngineHostError::Crashed {
            stderr_tail,
            status,
        }) => {
            assert!(
                stderr_tail.contains("crash mid-request"),
                "tail: {stderr_tail}"
            );
            assert!(status.contains('4'), "status: {status}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn crashed_window_is_retried_once_on_a_fresh_host() {
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("once");
    let f = fixture(&[("FAKE_CRASH_ONCE_FILE", marker_path.to_str().unwrap())]);
    let out = f
        .client
        .transcribe_dictation(&pcm(2.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 4);
    assert_eq!(f.client.snapshot().crashes_in_window, 1);
}

#[test]
fn deterministic_crash_is_not_retried_more_than_once() {
    let f = fixture(&[("FAKE_CRASH_ON_WINDOW", "1")]);
    assert!(f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap_err()
        .is_crash());
    // one original attempt + exactly one retry = two crashes
    assert_eq!(f.client.snapshot().crashes_in_window, 2);
}

#[test]
fn restart_backoff_then_failed_until_reset() {
    let f = fixture(&[("FAKE_CRASH_ON_START", "1")]);
    let mut starts = Vec::new();
    for _ in 0..4 {
        let t = Instant::now();
        match f.client.warm() {
            Err(EngineHostError::Crashed { stderr_tail, .. }) => {
                assert!(stderr_tail.contains("crash on start"), "{stderr_tail}")
            }
            other => panic!("{other:?}"),
        }
        starts.push(t.elapsed());
    }
    // backoff 0, 200 ms, 400 ms before attempts 2, 3, 4
    assert!(starts[2] >= Duration::from_millis(150), "{starts:?}");
    assert!(starts[3] >= Duration::from_millis(350), "{starts:?}");
    assert!(matches!(f.client.warm(), Err(EngineHostError::Failed(_))));
    assert!(f.client.snapshot().failed.is_some());
    // Failed is sticky until reset, and fails fast.
    let t = Instant::now();
    assert!(matches!(f.client.warm(), Err(EngineHostError::Failed(_))));
    assert!(t.elapsed() < Duration::from_millis(100));
    f.client.reset();
    assert!(matches!(
        f.client.warm(),
        Err(EngineHostError::Crashed { .. })
    ));
}

// -------------------------------------------------------------- lifecycle

#[test]
fn idle_unload_then_shutdown() {
    let f = fixture_with(&[], |c| {
        c.idle_unload = Some(Duration::from_millis(250));
        // Wide gap after the unload so the "unloaded but alive" state stays
        // observable on a loaded runner (shutdown idles from the unload ack).
        c.idle_shutdown = Some(Duration::from_millis(1500));
        c.supervisor_tick = Duration::from_millis(50);
    });
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    // `snapshot().loaded` leads the host (it flips when the unload is sent, so
    // no request is dispatched to a host about to unload); wait on the host's
    // own state, not the snapshot.
    wait_until("idle unload", Duration::from_secs(3), || {
        f.client.status().is_ok_and(|s| s.model_id.is_none())
    });
    assert!(!f.client.snapshot().loaded);
    assert!(f.client.snapshot().running);
    wait_until("idle shutdown", Duration::from_secs(3), || {
        !f.client.snapshot().running
    });
    wait_until("process exit", Duration::from_secs(2), || !pid_alive(pid));
    // Lazy restart on the next request; the restart is not a crash.
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
    assert_eq!(f.client.snapshot().crashes_in_window, 0);
}

#[test]
fn snapshot_leads_a_slow_host_unload_and_requests_wait_for_it() {
    let f = fixture_with(&[("FAKE_UNLOAD_DELAY_MS", "400")], |c| {
        c.idle_unload = Some(Duration::from_millis(100));
        c.idle_shutdown = None;
        c.supervisor_tick = Duration::from_millis(20);
    });
    f.client.warm().unwrap();
    wait_until("snapshot reports unloaded", Duration::from_secs(3), || {
        !f.client.snapshot().loaded
    });
    // The host has not acknowledged the unload yet, so it still holds the model.
    assert_eq!(
        f.client.status().unwrap().model_id.as_deref(),
        Some("fake-model")
    );
    wait_until("host unloaded", Duration::from_secs(3), || {
        f.client.status().is_ok_and(|s| s.model_id.is_none())
    });
    // A request after the unload reloads lazily and succeeds.
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
}

#[test]
fn busy_client_is_not_unloaded_while_a_request_runs() {
    let f = fixture_with(&[("FAKE_BATCH_DELAY_MS", "600")], |c| {
        c.idle_unload = Some(Duration::from_millis(150));
        c.supervisor_tick = Duration::from_millis(30);
    });
    let out = f
        .client
        .transcribe_file_samples(&pcm(3.0), &CancelToken::new(), None)
        .unwrap();
    assert_eq!(out.tokens.len(), 6);
}

#[test]
fn explicit_unload_reloads_lazily() {
    let f = fixture(&[]);
    f.client.warm().unwrap();
    f.client.unload().unwrap();
    assert!(!f.client.snapshot().loaded);
    assert_eq!(f.client.status().unwrap().model_id, None);
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
}

#[test]
fn explicit_shutdown_exits_process() {
    let f = fixture(&[]);
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    f.client.shutdown();
    assert!(!f.client.snapshot().running);
    assert!(!pid_alive(pid));
    assert!(matches!(
        f.client.status(),
        Err(EngineHostError::NotRunning)
    ));
}

#[test]
fn no_orphan_host_after_drop() {
    let f = fixture(&[]);
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    assert!(pid_alive(pid));
    drop(f.client);
    assert!(
        !pid_alive(pid),
        "host {pid} still running after client drop"
    );
}

/// Dropping the client while the host still hangs on a timed-out request
/// (its wedge grace still running) ends the host at once: the watcher that
/// waits for the late answer does not keep the client or its host alive.
#[test]
fn no_orphan_host_when_dropped_with_hung_request() {
    let f = fixture_with(&[("FAKE_HANG", "1")], |c| {
        c.timeouts.transcribe_min = Duration::from_millis(200);
        // Far away: the watchdog must not be what ends the host here.
        c.timeouts.wedge_grace = Duration::from_secs(60);
    });
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    let r = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new());
    assert!(
        matches!(
            r,
            Err(EngineHostError::Timeout {
                op: "transcribe_window",
                ..
            })
        ),
        "{r:?}"
    );
    assert!(pid_alive(pid), "the hung request is still within its grace");
    let client = f.client;
    let ((), elapsed) = run_bounded(Duration::from_secs(10), move || drop(client));
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    assert!(
        !pid_alive(pid),
        "host {pid} still running after client drop"
    );
}

// --------------------------------------------------- long-audio pipeline

#[test]
fn long_audio_reconstructs_synthetic_stream_exactly() {
    for (seconds, extra) in [
        (47.0, vec![]),
        (47.0, vec![("FAKE_FIRST_SLOW_MS", "250")]),
        (10.0, vec![]),
        (10.3, vec![]),
        (95.5, vec![("FAKE_WINDOW_DELAY_MS", "20")]),
    ] {
        let mut env: Vec<(&str, &str)> = LONG_ENV.to_vec();
        env.extend(extra);
        let f = fixture(&env);
        let samples = pcm(seconds);
        let mut progress = Vec::new();
        let mut cb = |m: usize, t: usize| progress.push((m, t));
        let out = f
            .client
            .transcribe_file_samples(&samples, &CancelToken::new(), Some(&mut cb))
            .unwrap();

        let ids: Vec<u32> = out.tokens.iter().map(|t| t.id).collect();
        assert_eq!(ids, expected_ids(samples.len()), "{seconds}s");
        // Absolute timing survives offsetting and merging.
        for t in &out.tokens {
            assert!(
                (t.start - f64::from(t.id) * 0.5).abs() < 1e-9,
                "token {} at {}",
                t.id,
                t.start
            );
        }
        assert_eq!(out.words.len(), ids.len());
        assert_eq!(out.words[3].word, "t3");
        assert!(out.text.starts_with("t0 t1 t2"));
        assert!((out.audio_s - seconds).abs() < 1e-6);

        // Window plan matches hello (10 s window, 4 s overlap => 6 s step).
        let expected_windows = if seconds <= 10.0 {
            1
        } else {
            ((seconds - 10.0) / 6.0).ceil() as usize + 1
        };
        assert_eq!(out.windows.len(), expected_windows, "{seconds}s");
        assert_eq!(out.windows[0].start_sample, 0);
        assert_eq!(out.windows.last().unwrap().end_sample, samples.len());
        for (i, w) in out.windows.iter().enumerate() {
            assert_eq!(w.index, i);
            assert_eq!((w.preprocess_ms, w.encode_ms, w.decode_ms), (1, 2, 3));
        }

        // Progress is monotonic, complete and ends at 100%.
        assert!(!progress.is_empty());
        assert!(progress.windows(2).all(|p| p[1].0 == p[0].0 + 1));
        assert_eq!(
            *progress.last().unwrap(),
            (expected_windows, expected_windows)
        );
        assert_eq!(
            temp_entries(f.tmp.path()),
            0,
            "temp PCM dir must be removed"
        );
    }
}

#[test]
fn empty_audio_does_not_start_the_host() {
    let f = fixture(&[]);
    let out = f
        .client
        .transcribe_dictation(&[], &CancelToken::new())
        .unwrap();
    assert!(out.text.is_empty() && out.tokens.is_empty());
    assert!(!f.client.snapshot().running);
}

#[test]
fn dictation_within_max_window_is_a_single_interactive_window() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let f = fixture(&[("FAKE_LOG", log_path.to_str().unwrap())]);
    let samples = pcm(5.0);
    let out = f
        .client
        .transcribe_dictation(&samples, &CancelToken::new())
        .unwrap();
    assert_eq!(out.windows.len(), 1);
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(samples.len())
    );
    let ops = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(
        ops.lines()
            .filter(|l| l.starts_with("transcribe_window"))
            .count(),
        1
    );
}

#[test]
fn dictation_longer_than_max_window_uses_the_windowed_path() {
    let f = fixture(&LONG_ENV);
    let samples = pcm(25.0);
    let out = f
        .client
        .transcribe_dictation(&samples, &CancelToken::new())
        .unwrap();
    assert!(out.windows.len() > 1);
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(samples.len())
    );
}

#[test]
fn dictation_is_not_queued_behind_a_running_file_job() {
    let f = fixture(&[
        ("FAKE_WINDOW_S", "10"),
        ("FAKE_OVERLAP_S", "4"),
        ("FAKE_BATCH_DELAY_MS", "400"),
    ]);
    f.client.warm().unwrap();
    let job_done = Arc::new(AtomicBool::new(false));
    let job = {
        let client = f.client.clone();
        let done = job_done.clone();
        std::thread::spawn(move || {
            let samples = pcm(47.0);
            let out = client
                .transcribe_file_samples(&samples, &CancelToken::new(), None)
                .unwrap();
            done.store(true, Ordering::SeqCst);
            (out, samples.len())
        })
    };
    std::thread::sleep(Duration::from_millis(300));
    let t = Instant::now();
    let d = f
        .client
        .transcribe_dictation(&pcm(3.0), &CancelToken::new())
        .unwrap();
    let latency = t.elapsed();
    assert_eq!(d.tokens.len(), 6);
    assert!(
        latency < Duration::from_millis(800),
        "dictation took {latency:?}"
    );
    assert!(
        !job_done.load(Ordering::SeqCst),
        "file job should still be running (8 windows x 400 ms / 2)"
    );
    let (out, n) = job.join().unwrap();
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(n)
    );
}

#[test]
fn cancel_during_long_job_returns_cancelled_and_cleans_up() {
    let mut env = LONG_ENV.to_vec();
    env.push(("FAKE_WINDOW_DELAY_MS", "80"));
    let f = fixture(&env);
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    let mut merged_at_cancel = 0;
    let mut cb = |m: usize, _t: usize| {
        merged_at_cancel = m;
        if m == 2 {
            c2.cancel();
        }
    };
    let t = Instant::now();
    let r = f
        .client
        .transcribe_file_samples(&pcm(95.0), &cancel, Some(&mut cb));
    assert!(matches!(r, Err(EngineHostError::Cancelled)), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(merged_at_cancel, 2, "no windows merged after cancel");
    assert_eq!(temp_entries(f.tmp.path()), 0);
    wait_until("host idle after cancel", Duration::from_secs(2), || {
        f.client.status().map(|s| s.in_flight == 0).unwrap_or(false)
    });
}

#[test]
fn pre_cancelled_job_never_touches_the_host() {
    let f = fixture(&[]);
    let cancel = CancelToken::new();
    cancel.cancel();
    assert!(matches!(
        f.client.transcribe_file_samples(&pcm(1.0), &cancel, None),
        Err(EngineHostError::Cancelled)
    ));
    assert!(!f.client.snapshot().running);
    assert_eq!(temp_entries(f.tmp.path()), 0);
}

#[test]
fn temp_pcm_is_removed_after_error() {
    let f = fixture_with(&[("FAKE_CRASH_ON_WINDOW", "1")], |c| {
        c.max_crash_retries = 0
    });
    assert!(f
        .client
        .transcribe_file_samples(&pcm(2.0), &CancelToken::new(), None)
        .is_err());
    assert_eq!(temp_entries(f.tmp.path()), 0);
}

#[cfg(unix)]
#[test]
fn temp_pcm_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let pcm_file = sagascript_core::transcription::engine_host::pipeline::PcmFile::write(
        &pcm(1.0),
        Some(tmp.path()),
    )
    .unwrap();
    let mode = std::fs::metadata(pcm_file.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let dir_mode = std::fs::metadata(pcm_file.dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    assert_eq!(
        std::fs::metadata(pcm_file.path()).unwrap().len(),
        16_000 * 4
    );
}

#[test]
fn crash_mid_long_job_resubmits_and_still_reconstructs() {
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("once");
    let mut env = LONG_ENV.to_vec();
    env.push(("FAKE_CRASH_ONCE_FILE", marker_path.to_str().unwrap()));
    let f = fixture(&env);
    let samples = pcm(47.0);
    let out = f
        .client
        .transcribe_file_samples(&samples, &CancelToken::new(), None)
        .unwrap();
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(samples.len())
    );
    assert_eq!(f.client.snapshot().crashes_in_window, 1);
}

// ------------------------------------------------ review fixes (2026-09)

use sagascript_core::transcription::engine_host::TestHook;
use std::sync::mpsc;
use std::sync::Mutex;

fn ops_of(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split(' ').next().unwrap().to_string())
        .collect()
}

/// Finding 1: `hello` advertises 30 s but the loaded model only takes 15 s.
#[test]
fn load_window_bounds_the_effective_capabilities() {
    let env = [
        ("FAKE_WINDOW_S", "30"),
        ("FAKE_LOAD_WINDOW_S", "15"),
        ("FAKE_OVERLAP_S", "4"),
    ];
    let f = fixture(&env);
    let caps = f.client.warm().unwrap();
    assert!((caps.max_window_s - 15.0).abs() < 1e-9, "{caps:?}");
    assert!(caps.preferred_window_s <= 15.0, "{caps:?}");
    assert_eq!(f.client.capabilities().unwrap().max_window_s, 15.0);

    for seconds in [20.0, 29.0, 40.0] {
        let samples = pcm(seconds);
        let out = f
            .client
            .transcribe_file_samples(&samples, &CancelToken::new(), None)
            .unwrap();
        assert!(out.windows.len() >= 2, "{seconds}s: {}", out.windows.len());
        assert!(out
            .windows
            .iter()
            .all(|w| w.end_sample - w.start_sample <= 15 * SR));
        assert_eq!(
            out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
            expected_ids(samples.len()),
            "{seconds}s"
        );
    }
    // Dictation takes the same path.
    let out = f
        .client
        .transcribe_dictation(&pcm(20.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.windows.len(), 2);
}

#[test]
fn effective_window_survives_restart_and_reload() {
    let env = [
        ("FAKE_WINDOW_S", "30"),
        ("FAKE_LOAD_WINDOW_S", "15"),
        ("FAKE_OVERLAP_S", "4"),
    ];
    let f = fixture(&env);
    f.client.warm().unwrap();
    f.client.unload().unwrap();
    assert!(f.client.warm().unwrap().max_window_s <= 15.0);
    f.client.shutdown();
    assert!(f.client.warm().unwrap().max_window_s <= 15.0);
    let out = f
        .client
        .transcribe_file_samples(&pcm(20.0), &CancelToken::new(), None)
        .unwrap();
    assert_eq!(out.windows.len(), 2);
}

/// Finding 2: with 3 host slots, batch work may use 2 so dictation starts at once.
#[test]
fn interactive_request_gets_a_reserved_host_slot() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let f = fixture(&[
        ("FAKE_WINDOW_S", "10"),
        ("FAKE_OVERLAP_S", "4"),
        ("FAKE_MAX_IN_FLIGHT", "3"),
        ("FAKE_BATCH_DELAY_MS", "1500"),
        ("FAKE_LOG", log_path.to_str().unwrap()),
    ]);
    f.client.warm().unwrap();
    let job = {
        let client = f.client.clone();
        std::thread::spawn(move || {
            client
                .transcribe_file_samples(&pcm(47.0), &CancelToken::new(), None)
                .map(|o| o.tokens.len())
        })
    };
    // Wait until the batch lane is full.
    wait_until("batch lane full", Duration::from_secs(5), || {
        ops_of(&log_path)
            .iter()
            .filter(|o| *o == "transcribe_window")
            .count()
            >= 2
    });
    std::thread::sleep(Duration::from_millis(100));
    let batch_started = ops_of(&log_path)
        .iter()
        .filter(|o| *o == "transcribe_window")
        .count();
    assert_eq!(batch_started, 2, "batch must leave one slot free");
    let t = Instant::now();
    let d = f
        .client
        .transcribe_dictation(&pcm(3.0), &CancelToken::new())
        .unwrap();
    let latency = t.elapsed();
    assert_eq!(d.tokens.len(), 6);
    assert!(
        latency < Duration::from_millis(700),
        "dictation waited for a batch window: {latency:?}"
    );
    assert!(!job.is_finished());
    job.join().unwrap().unwrap();
}

/// A host with a single slot cannot reserve one for dictation, but dictation is
/// still sent right after the running window and before any further batch window.
#[test]
fn single_slot_host_sends_dictation_before_next_batch_window() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let f = fixture(&[
        ("FAKE_WINDOW_S", "10"),
        ("FAKE_OVERLAP_S", "4"),
        ("FAKE_MAX_IN_FLIGHT", "1"),
        ("FAKE_BATCH_DELAY_MS", "1500"),
        ("FAKE_LOG", log_path.to_str().unwrap()),
    ]);
    f.client.warm().unwrap();
    let job = {
        let client = f.client.clone();
        std::thread::spawn(move || {
            client
                .transcribe_file_samples(&pcm(47.0), &CancelToken::new(), None)
                .map(|o| o.tokens.len())
        })
    };
    let windows = |p: &Path| {
        ops_of(p)
            .iter()
            .filter(|o| *o == "transcribe_window")
            .count()
    };
    wait_until("first batch window running", Duration::from_secs(5), || {
        windows(&log_path) >= 1
    });
    let started = Instant::now();
    let d = f
        .client
        .transcribe_dictation(&pcm(3.0), &CancelToken::new())
        .unwrap();
    assert_eq!(d.tokens.len(), 6);
    // Dictation waits for the running window (~1.5 s) only. If a further batch
    // window (another 1.5 s) were sent first this would take about 3 s.
    let waited = started.elapsed();
    assert!(
        waited < Duration::from_millis(2200),
        "dictation waited behind another batch window: {waited:?}"
    );
    assert!(!job.is_finished());
    job.join().unwrap().unwrap();
}

/// Finding 3: the idle supervisor must not unload between readiness and dispatch.
#[test]
fn idle_unload_cannot_race_with_a_batch_request() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let reached_tx = Mutex::new(reached_tx);
    let f = fixture_with(&[("FAKE_LOG", log_path.to_str().unwrap())], |cfg| {
        cfg.idle_unload = Some(Duration::ZERO);
        cfg.supervisor_tick = Duration::from_secs(3600);
        cfg.test_hook = Some(TestHook(Arc::new(move |point| {
            if point == "after_ready" {
                let _ = reached_tx.lock().unwrap().send(());
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
            }
        })));
    });
    f.client.warm().unwrap();
    let t = {
        let client = f.client.clone();
        std::thread::spawn(move || {
            let dir = tempfile::tempdir().unwrap();
            let pcm_path = dir.path().join("a.f32le");
            let bytes: Vec<u8> = (0..8000u32)
                .flat_map(|i| (i as f32).to_le_bytes())
                .collect();
            std::fs::write(&pcm_path, bytes).unwrap();
            let req = WindowRequest {
                pcm_path,
                offset_samples: 0,
                num_samples: 8000,
                sample_rate: 16000,
                priority: Priority::Batch,
                boost: None,
            };
            let ticket = client
                .submit_window(&req, &CancelToken::new(), Duration::from_secs(2))?
                .expect("slot");
            ticket.wait(&CancelToken::new())
        })
    };
    reached_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    f.client.run_idle_check_now();
    release_tx.send(()).unwrap();
    let out = t.join().unwrap().unwrap();
    assert_eq!(out.tokens.len(), 1);
    assert!(
        !ops_of(&log_path).iter().any(|o| o == "unload"),
        "supervisor unloaded under an in-progress request: {:?}",
        ops_of(&log_path)
    );
}

/// Finding 3 (defense in depth): a `not_loaded` answer reloads and resends once.
#[test]
fn not_loaded_response_is_retried_once_after_reload() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let (reached_tx, reached_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Mutex::new(release_rx);
    let reached_tx = Mutex::new(reached_tx);
    let f = fixture_with(&[("FAKE_LOG", log_path.to_str().unwrap())], |cfg| {
        cfg.test_hook = Some(TestHook(Arc::new(move |point| {
            if point == "after_ready" {
                let _ = reached_tx.lock().unwrap().send(());
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
            }
        })));
    });
    f.client.warm().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let pcm_path = dir.path().join("a.f32le");
    let bytes: Vec<u8> = (0..8000u32)
        .flat_map(|i| (i as f32).to_le_bytes())
        .collect();
    std::fs::write(&pcm_path, bytes).unwrap();
    let t = {
        let client = f.client.clone();
        let pcm_path = pcm_path.clone();
        std::thread::spawn(move || {
            let req = WindowRequest {
                pcm_path,
                offset_samples: 0,
                num_samples: 8000,
                sample_rate: 16000,
                priority: Priority::Interactive,
                boost: None,
            };
            let ticket = client
                .submit_window(&req, &CancelToken::new(), Duration::from_secs(2))?
                .expect("slot");
            ticket.wait(&CancelToken::new())
        })
    };
    reached_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    f.client.unload().unwrap();
    release_tx.send(()).unwrap();
    let out = t.join().unwrap().unwrap();
    assert_eq!(out.tokens.len(), 1);
    let loads = ops_of(&log_path).iter().filter(|o| *o == "load").count();
    assert_eq!(loads, 2, "{:?}", ops_of(&log_path));
}

/// Finding 4: cancelling while the model loads returns promptly and keeps the host.
#[test]
fn cancel_during_slow_load_returns_promptly_without_killing_the_host() {
    let log = tempfile::tempdir().unwrap();
    let log_path = log.path().join("ops.log");
    let f = fixture(&[
        ("FAKE_LOAD_DELAY_MS", "2500"),
        ("FAKE_LOG", log_path.to_str().unwrap()),
    ]);
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            cancel.cancel();
            Instant::now()
        })
    };
    let r = f.client.warm_with_cancel(&cancel);
    let returned = Instant::now();
    let cancelled_at = canceller.join().unwrap();
    assert!(matches!(r, Err(EngineHostError::Cancelled)), "{r:?}");
    assert!(
        returned.saturating_duration_since(cancelled_at) < Duration::from_millis(250),
        "cancel took {:?}",
        returned.saturating_duration_since(cancelled_at)
    );
    let pid = f.client.snapshot().pid.expect("host must stay alive");
    assert!(pid_alive(pid));
    // The in-progress load completes and is reused by the next caller.
    f.client.warm().unwrap();
    assert_eq!(f.client.snapshot().pid, Some(pid));
    let loads = ops_of(&log_path).iter().filter(|o| *o == "load").count();
    assert_eq!(loads, 1, "{:?}", ops_of(&log_path));
}

/// Finding 6: a newline-less flood is bounded; the host is killed.
#[test]
fn newline_less_flood_kills_the_host_and_fails_in_flight_requests() {
    let f = fixture(&[("FAKE_FLOOD", "1")]);
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    let t = Instant::now();
    let r = f
        .client
        .transcribe_dictation(&pcm(2.0), &CancelToken::new());
    assert!(
        matches!(&r, Err(EngineHostError::Protocol(m)) if m.contains("16 MiB")),
        "{r:?}"
    );
    assert!(t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
    wait_until("host killed", Duration::from_secs(3), || !pid_alive(pid));
}

// ------------------------------------------------- recovery (#366)

use sagascript_core::transcription::engine_host::pipeline::{
    sweep_stale_pcm_dirs, sweep_stale_pcm_dirs_in_background, PcmFile,
};
use sagascript_core::transcription::engine_host::BoostSpec;
use std::path::PathBuf;

/// Run `job` on its own thread and fail the test (instead of hanging it) when
/// it has not finished within `limit`. Returns the result and how long it took.
fn run_bounded<T: Send + 'static>(
    limit: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let started = Instant::now();
        let out = job();
        let _ = tx.send((out, started.elapsed()));
    });
    rx.recv_timeout(limit)
        .unwrap_or_else(|_| panic!("still blocked after {limit:?}"))
}

/// (a) Repeated start failures mark the host failed for a cooldown only: the
/// first request after it gets one fresh attempt (a failed attempt re-opens the
/// cooldown), so a host that is healthy again works without an app restart.
#[test]
fn failed_host_gets_a_fresh_attempt_after_the_cooldown() {
    let marker = tempfile::tempdir().unwrap();
    let crash = marker.path().join("crash");
    std::fs::write(&crash, b"crash").unwrap();
    // Long enough that two back-to-back calls never straddle it on a loaded runner.
    let cooldown = Duration::from_secs(1);
    let f = fixture_with(
        &[("FAKE_CRASH_ON_START_FILE", crash.to_str().unwrap())],
        |c| c.failed_cooldown = cooldown,
    );
    for _ in 0..4 {
        let r = f.client.warm();
        assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    }
    assert!(matches!(f.client.warm(), Err(EngineHostError::Failed(_))));
    let opened = Instant::now();
    assert_eq!(f.client.snapshot().spawns, 4);

    // Within the cooldown: fail fast without spawning anything.
    assert!(matches!(f.client.warm(), Err(EngineHostError::Failed(_))));
    assert_eq!(f.client.snapshot().spawns, 4);

    // After it: exactly one attempt, which fails here and re-opens the cooldown.
    std::thread::sleep(cooldown.saturating_sub(opened.elapsed()) + Duration::from_millis(50));
    let r = f.client.warm();
    assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    assert_eq!(f.client.snapshot().spawns, 5);
    match f.client.warm() {
        Err(EngineHostError::Failed(m)) => assert!(m.contains("next attempt in"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(f.client.snapshot().spawns, 5);

    // The host is healthy again: the next attempt after the cooldown succeeds.
    std::fs::remove_file(&crash).unwrap();
    wait_until(
        "recovery after the cooldown",
        Duration::from_secs(5),
        || f.client.warm().is_ok(),
    );
    let snap = f.client.snapshot();
    assert!(snap.failed.is_none(), "{:?}", snap.failed);
    assert_eq!(snap.crashes_in_window, 0);
    assert_eq!(snap.spawns, 6);
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
}

/// The fresh start after the cooldown counts as a recovery only once the
/// model has loaded: a host that starts but crashes on `load` re-opens the
/// cooldown instead of earning a new crash budget (4 more spawn/crash rounds).
#[test]
fn failed_cooldown_recovers_only_after_a_successful_load() {
    let marker = tempfile::tempdir().unwrap();
    let crash = marker.path().join("crash");
    std::fs::write(&crash, b"crash").unwrap();
    let cooldown = Duration::from_secs(1);
    let f = fixture_with(
        &[("FAKE_CRASH_ON_LOAD_FILE", crash.to_str().unwrap())],
        |c| c.failed_cooldown = cooldown,
    );
    for _ in 0..4 {
        let r = f.client.warm();
        assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    }
    assert!(matches!(f.client.warm(), Err(EngineHostError::Failed(_))));
    let opened = Instant::now();
    assert_eq!(f.client.snapshot().spawns, 4);

    // After the cooldown: one start whose handshake succeeds but whose load crashes.
    std::thread::sleep(cooldown.saturating_sub(opened.elapsed()) + Duration::from_millis(50));
    let r = f.client.warm();
    assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    assert_eq!(f.client.snapshot().spawns, 5);
    // That re-opened the cooldown: failed again, without another spawn.
    match f.client.warm() {
        Err(EngineHostError::Failed(m)) => {
            assert!(m.contains("load after the cooldown failed"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(f.client.snapshot().spawns, 5);

    // Healthy again: the next attempt after the cooldown loads and recovers.
    std::fs::remove_file(&crash).unwrap();
    wait_until(
        "recovery after the cooldown",
        Duration::from_secs(5),
        || f.client.warm().is_ok(),
    );
    let snap = f.client.snapshot();
    assert!(snap.failed.is_none(), "{:?}", snap.failed);
    assert_eq!(snap.crashes_in_window, 0);
    assert_eq!(snap.spawns, 6);
}

/// (b) A window that ignores cancel keeps its host slot while the host still
/// answers `ping` (a Core ML prediction cannot be interrupted). Within the
/// request timeout plus the wedge grace the client kills that host, and the
/// next window succeeds on a fresh one.
#[test]
fn host_wedged_on_a_timed_out_window_is_restarted_and_the_next_window_succeeds() {
    let marker = tempfile::tempdir().unwrap();
    let wedge = marker.path().join("wedge");
    let timeout = Duration::from_millis(300);
    let grace = Duration::from_millis(300);
    let f = fixture_with(
        &[
            ("FAKE_WEDGE_ONCE_FILE", wedge.to_str().unwrap()),
            ("FAKE_MAX_IN_FLIGHT", "1"),
        ],
        |c| {
            c.timeouts.transcribe_min = timeout;
            c.timeouts.wedge_grace = grace;
        },
    );
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();
    let samples = pcm(1.0);
    let pcm_file = PcmFile::write(&samples, Some(f.tmp.path())).unwrap();
    let cancel = CancelToken::new();
    let started = Instant::now();
    let ticket = f
        .client
        .submit_window(
            &WindowRequest {
                pcm_path: pcm_file.path().to_path_buf(),
                offset_samples: 0,
                num_samples: samples.len() as u64,
                sample_rate: 16_000,
                priority: Priority::Interactive,
                boost: None,
            },
            &cancel,
            Duration::from_secs(1),
        )
        .unwrap()
        .unwrap();
    wait_until("window wedged in the host", Duration::from_secs(2), || {
        f.client.status().is_ok_and(|s| s.in_flight == 1)
    });
    f.client.ping().expect("a wedged host still answers ping");

    let r = ticket.wait(&cancel);
    assert!(
        matches!(
            r,
            Err(EngineHostError::Timeout {
                op: "transcribe_window",
                ..
            })
        ),
        "{r:?}"
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < timeout + grace + Duration::from_millis(1500),
        "{elapsed:?}"
    );
    wait_until("wedged host killed", Duration::from_secs(2), || {
        !pid_alive(pid)
    });

    let out = f
        .client
        .transcribe_dictation(&pcm(2.0), &CancelToken::new())
        .unwrap();
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(2 * SR)
    );
    let snap = f.client.snapshot();
    assert_ne!(snap.pid, Some(pid));
    assert_eq!(snap.spawns, 2);
    assert_eq!(
        snap.crashes_in_window, 1,
        "the wedge counts against the budget"
    );
}

/// #397: a healthy host can spend longer than the window timeout in one
/// uninterruptible call (the Neural Engine recompiling the encoder took
/// 11.5-16.7 s). The caller gets its timeout at once, not after the wedge
/// grace; the host answers the cancelled window late but within the grace, so
/// it is not killed, no crash is counted and later requests keep using it
/// (killing it would throw the compile away and the next dictation would pay
/// for it again).
#[test]
fn slow_uninterruptible_window_times_out_promptly_and_keeps_the_host() {
    let timeout = Duration::from_millis(300);
    // Longer than the 5 s grace used before #397 was addressed.
    let stall = Duration::from_secs(6);
    let stall_ms = stall.as_millis().to_string();
    let f = fixture_with(
        &[("FAKE_FIRST_UNINTERRUPTIBLE_MS", stall_ms.as_str())],
        |c| {
            c.timeouts.transcribe_min = timeout;
            c.timeouts.wedge_grace = Timeouts::default().wedge_grace;
        },
    );
    f.client.warm().unwrap();
    let pid = f.client.snapshot().pid.unwrap();

    let started = Instant::now();
    let r = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new());
    let elapsed = started.elapsed();
    assert!(
        matches!(
            r,
            Err(EngineHostError::Timeout {
                op: "transcribe_window",
                ..
            })
        ),
        "{r:?}"
    );
    assert!(
        elapsed < timeout + Duration::from_secs(1),
        "the caller waited for the wedge grace: {elapsed:?}"
    );

    // The host is busy, not broken: the next dictation runs on it meanwhile.
    let out = f
        .client
        .transcribe_dictation(&pcm(1.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 2);
    assert_eq!(f.client.snapshot().pid, Some(pid));

    // The stalled window ends and answers its cancel within the grace.
    wait_until(
        "late answer to the timed-out window",
        stall + Duration::from_secs(5),
        || f.client.status().is_ok_and(|s| s.in_flight == 0),
    );
    assert!(started.elapsed() >= stall, "{:?}", started.elapsed());
    assert!(pid_alive(pid), "a host that answered late was killed");
    let snap = f.client.snapshot();
    assert_eq!(snap.pid, Some(pid));
    assert_eq!(snap.crashes_in_window, 0);
    assert_eq!(snap.spawns, 1);
    let out = f
        .client
        .transcribe_dictation(&pcm(2.0), &CancelToken::new())
        .unwrap();
    assert_eq!(out.tokens.len(), 4);
    assert_eq!(f.client.snapshot().pid, Some(pid));
}

/// 500 terms of 64 four-byte characters: each window request line is about
/// 125 KB, more than an OS pipe buffer holds.
fn pipe_filling_boost() -> BoostSpec {
    let terms = (0..500).map(|i| format!("{i:03}{}", "\u{1F600}".repeat(61)));
    let boost = BoostSpec::new(terms, 5.0).unwrap();
    assert_eq!(boost.terms.len(), 500);
    boost
}

/// A host that reads the first window and then stops reading stdin, with a
/// batch lane of two so the second window's write fills the pipe.
fn stop_reading_fixture(marker: &Path, log: &Path, write_timeout: Duration) -> Fixture {
    let marker = marker.join("stop-reading");
    let f = fixture_with(
        &[
            ("FAKE_STOP_READING_ONCE_FILE", marker.to_str().unwrap()),
            ("FAKE_MAX_IN_FLIGHT", "3"),
            ("FAKE_WINDOW_S", "10"),
            ("FAKE_OVERLAP_S", "4"),
            ("FAKE_LOG", log.to_str().unwrap()),
        ],
        |c| c.timeouts.write = write_timeout,
    );
    f.client.set_boost(Some(pipe_filling_boost()));
    f.client.warm().unwrap();
    f
}

/// (c) A host that stops reading stdin: the blocked write ends in a request
/// timeout instead of an eternal hang, that host is killed, and the next job
/// runs on a fresh host.
#[test]
fn blocked_stdin_write_times_out_instead_of_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ops.log");
    let f = stop_reading_fixture(dir.path(), &log, Duration::from_millis(300));
    let pid = f.client.snapshot().pid.unwrap();
    let client = f.client.clone();
    let (r, elapsed) = run_bounded(Duration::from_secs(20), move || {
        client
            .transcribe_file_samples(&pcm(16.0), &CancelToken::new(), None)
            .map(|o| o.tokens.len())
    });
    assert!(
        matches!(
            r,
            Err(EngineHostError::Timeout {
                op: "transcribe_window",
                ..
            })
        ),
        "{r:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    wait_until(
        "host that stopped reading killed",
        Duration::from_secs(3),
        || !pid_alive(pid),
    );

    let samples = pcm(16.0);
    let out = f
        .client
        .transcribe_file_samples(&samples, &CancelToken::new(), None)
        .unwrap();
    assert_eq!(
        out.tokens.iter().map(|t| t.id).collect::<Vec<_>>(),
        expected_ids(samples.len())
    );
    assert_eq!(f.client.snapshot().spawns, 2);
}

/// (c) Shutting the host down does not wait for a stdin write that cannot
/// complete (the write timeout is far away here): it returns within its grace.
#[test]
fn shutdown_is_not_blocked_by_a_stdin_write_that_cannot_complete() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("ops.log");
    let f = stop_reading_fixture(dir.path(), &log, Duration::from_secs(30));
    let pid = f.client.snapshot().pid.unwrap();
    let job = {
        let client = f.client.clone();
        std::thread::spawn(move || {
            client
                .transcribe_file_samples(&pcm(16.0), &CancelToken::new(), None)
                .map(|o| o.tokens.len())
        })
    };
    wait_until("host stopped reading", Duration::from_secs(5), || {
        ops_of(&log).iter().any(|o| o == "transcribe_window")
    });
    std::thread::sleep(Duration::from_millis(300)); // the second window's write is blocked now
    let client = f.client.clone();
    let ((), elapsed) = run_bounded(Duration::from_secs(10), move || client.shutdown());
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    wait_until("host gone", Duration::from_secs(3), || !pid_alive(pid));
    wait_until("blocked job released", Duration::from_secs(10), || {
        job.is_finished()
    });
}

/// The restart backoff is slept with the lifecycle lock released (shutdown is
/// not held up) and a cancelled caller stops waiting at once.
#[test]
fn restart_backoff_is_cancellable_and_does_not_block_shutdown() {
    let f = fixture_with(&[("FAKE_CRASH_ON_START", "1")], |c| {
        c.restart_backoff = vec![Duration::from_secs(5)]
    });
    let r = f.client.warm();
    assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    let cancel = CancelToken::new();
    let waiter = {
        let client = f.client.clone();
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let r = client.warm_with_cancel(&cancel).map(|_| ());
            (r, started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(200)); // the waiter is in the 5 s backoff
    let client = f.client.clone();
    let ((), elapsed) = run_bounded(Duration::from_secs(10), move || client.shutdown());
    assert!(
        elapsed < Duration::from_secs(1),
        "shutdown waited for the restart backoff: {elapsed:?}"
    );
    cancel.cancel();
    wait_until("cancelled waiter returns", Duration::from_secs(2), || {
        waiter.is_finished()
    });
    let (r, waited) = waiter.join().unwrap();
    assert!(matches!(r, Err(EngineHostError::Cancelled)), "{r:?}");
    assert!(waited < Duration::from_secs(2), "{waited:?}");
    assert_eq!(f.client.snapshot().spawns, 1, "no restart after the cancel");
}

/// `connect` (`engine status` / `engine doctor`) sleeps a pending restart
/// backoff with the lifecycle lock released as well.
#[test]
fn connect_sleeps_the_restart_backoff_without_blocking_shutdown() {
    let backoff = Duration::from_millis(2500);
    let f = fixture_with(&[("FAKE_CRASH_ON_START", "1")], |c| {
        c.restart_backoff = vec![backoff]
    });
    let r = f.client.warm();
    assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
    let connecting = {
        let client = f.client.clone();
        std::thread::spawn(move || client.connect().map(|_| ()))
    };
    std::thread::sleep(Duration::from_millis(200)); // connect is in the backoff
    let client = f.client.clone();
    let ((), elapsed) = run_bounded(Duration::from_secs(10), move || client.shutdown());
    assert!(
        elapsed < Duration::from_millis(1200),
        "shutdown waited for connect's restart backoff: {elapsed:?}"
    );
    let r = connecting.join().unwrap();
    assert!(matches!(r, Err(EngineHostError::Crashed { .. })), "{r:?}");
}

const PCM_OWNER_PARENT: &str = "SAGASCRIPT_TEST_PCM_OWNER_PARENT";

/// Helper process for `startup_sweep_removes_only_pcm_dirs_of_dead_owners`:
/// re-run with `PCM_OWNER_PARENT` set, it owns a live PCM dir until killed.
/// A no-op as a normal test.
#[test]
fn pcm_dir_owner_process() {
    let Some(parent) = std::env::var_os(PCM_OWNER_PARENT) else {
        return;
    };
    let file = PcmFile::write(&pcm(0.1), Some(Path::new(&parent))).unwrap();
    let name = file
        .dir()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    println!("\nPCM_DIR={name}");
    std::io::Write::flush(&mut std::io::stdout()).unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

fn spawn_pcm_owner(parent: &Path) -> (std::process::Child, PathBuf) {
    use std::io::BufRead;
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "pcm_dir_owner_process",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PCM_OWNER_PARENT, parent)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = std::io::BufReader::new(stdout).lines();
    let name = loop {
        let line = lines
            .next()
            .expect("owner process exited before creating its dir")
            .unwrap();
        if let Some(name) = line.split("PCM_DIR=").nth(1) {
            break name.trim().to_string();
        }
    };
    (child, parent.join(name))
}

/// (d) PCM temp dirs left behind by a killed process are removed by the
/// startup sweep; dirs owned by a live process (another one, or this one) and
/// entries that do not match exactly are kept.
#[test]
fn startup_sweep_removes_only_pcm_dirs_of_dead_owners() {
    let parent = tempfile::tempdir().unwrap();
    let (mut dead, dead_dir) = spawn_pcm_owner(parent.path());
    let (mut live, live_dir) = spawn_pcm_owner(parent.path());
    let ours = PcmFile::write(&pcm(0.1), Some(parent.path())).unwrap();
    dead.kill().unwrap(); // no destructor runs: the dir stays behind
    dead.wait().unwrap();
    assert!(dead_dir.join("audio.f32le").is_file());
    // Windows may release a killed process's file lock with a delay; the
    // startup sweep below runs once, so wait until the lock is free.
    wait_until("dead owner's lock released", Duration::from_secs(5), || {
        std::fs::File::open(dead_dir.join("owner.lock"))
            .is_ok_and(|f| fs2::FileExt::try_lock_exclusive(&f).is_ok())
    });

    // Entries the sweep must leave alone: a non-matching name and a dir in
    // the pre-#366 format, whose owner cannot be proven dead.
    let unrelated = parent.path().join("sagascript-pcm-notes");
    std::fs::create_dir(&unrelated).unwrap();
    let legacy = parent.path().join("sagascript-pcm-AbC123");
    std::fs::create_dir(&legacy).unwrap();
    std::fs::write(legacy.join("audio.f32le"), b"old").unwrap();
    // A symlink named like a PCM dir is never followed.
    #[cfg(unix)]
    let outside = {
        // A copy of the stale dir: it would be removed if the link were followed.
        let outside = tempfile::tempdir().unwrap();
        for entry in std::fs::read_dir(&dead_dir).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), outside.path().join(entry.file_name())).unwrap();
        }
        std::os::unix::fs::symlink(
            outside.path(),
            parent.path().join("sagascript-pcm-symlinkABCDE"),
        )
        .unwrap();
        outside
    };

    sweep_stale_pcm_dirs_in_background(parent.path().to_path_buf());
    wait_until("stale PCM dir removed", Duration::from_secs(5), || {
        !dead_dir.exists()
    });
    assert!(
        live_dir.join("audio.f32le").is_file(),
        "live owner's dir kept"
    );
    assert!(ours.path().is_file(), "this process's dir kept");
    assert!(unrelated.is_dir() && legacy.join("audio.f32le").is_file());
    #[cfg(unix)]
    assert!(
        outside.path().join("audio.f32le").is_file(),
        "symlink followed"
    );

    live.kill().unwrap();
    live.wait().unwrap();
    let mut removed = 0;
    // Windows may release a killed process's file lock with a delay.
    wait_until("second owner's dir removed", Duration::from_secs(5), || {
        let swept = sweep_stale_pcm_dirs(parent.path()).unwrap();
        removed += swept.removed;
        assert!(swept.kept >= 1, "{swept:?}");
        !live_dir.exists()
    });
    assert_eq!(removed, 1);
    assert!(ours.path().is_file());
}
