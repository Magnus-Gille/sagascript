//! Deterministic fake `sagascript-engine-host` for tests (no model needed).
//!
//! Tokens: one per 0.5 s of window audio (`k * 8000` samples). Token `k` reads
//! the f32 sample at `offset + k * 8000`; the test PCM stores the absolute
//! sample index as the sample value, so the token id is the absolute token
//! index `value / 8000`. Overlapping windows therefore agree exactly and the
//! merged stream must be `0..N`.
//!
//! Behavior is set through environment variables (all optional):
//!   FAKE_WINDOW_S, FAKE_OVERLAP_S, FAKE_MAX_WINDOW_S   capabilities
//!   FAKE_MAX_IN_FLIGHT (actual slots), FAKE_ADVERTISE_IN_FLIGHT
//!   FAKE_PROTOCOL=<n>, FAKE_NO_SHA=1, FAKE_GARBAGE=1   handshake / stdout faults
//!   FAKE_CRASH_ON_START=1                               exit(3) immediately
//!   FAKE_CRASH_ON_WINDOW=1                              exit(4) on first window
//!   FAKE_CRASH_ONCE_FILE=<path>                         like above, only if the file is absent (creates it)
//!   FAKE_HANG=1                                         never answer transcribe_window
//!   FAKE_WINDOW_DELAY_MS, FAKE_BATCH_DELAY_MS           per-window delay (batch adds to base)
//!   FAKE_FIRST_SLOW_MS                                  extra delay for the first window received
//!   FAKE_IGNORE_CANCEL=1                                answer cancel but keep working
//!   FAKE_BUSY=1                                         reject with busy when slots are full
//!   FAKE_LOAD_DELAY_MS                                  delay `load`
//!   FAKE_LOAD_WINDOW_S                                  `LoadResult.window_s` and the enforced limit
//!                                                       (default: FAKE_MAX_WINDOW_S / FAKE_WINDOW_S)
//!   FAKE_FLOOD=1                                        on the first window, write 17 MiB without a newline
//!   FAKE_LOG=<path>                                     append "<op> <id>" lines

use sagascript_engine_protocol::{
    decode_request, encode_err, encode_event, encode_ok, ErrorCode, ParsedRequest, Priority,
    RequestOp,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const TOKEN_SAMPLES: u64 = 8000;

fn env_f64(k: &str, d: f64) -> f64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}
fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}
/// The window the loaded model really accepts (what `load` reports).
fn enforced_window_s() -> f64 {
    env_f64(
        "FAKE_LOAD_WINDOW_S",
        env_f64("FAKE_MAX_WINDOW_S", env_f64("FAKE_WINDOW_S", 30.0)),
    )
}
fn env_flag(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0")
}

struct Job {
    seq: u64,
    rank: u8,
    id: u64,
}

#[derive(Default)]
struct Sched {
    running: usize,
    queue: Vec<Job>,
}

struct Host {
    out: Mutex<std::io::Stdout>,
    sched: Mutex<Sched>,
    cv: Condvar,
    cancels: Mutex<HashMap<u64, Arc<AtomicBool>>>,
    loaded: Mutex<Option<String>>,
    seq: AtomicU64,
    windows_seen: AtomicU64,
    started: Instant,
    max_in_flight: usize,
}

impl Host {
    fn write(&self, line: &str) {
        let mut o = self.out.lock().unwrap();
        let _ = o.write_all(line.as_bytes());
        let _ = o.flush();
    }
}

fn log(op: &str, id: u64) {
    if let Ok(path) = std::env::var("FAKE_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{op} {id}");
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !args.windows(2).any(|w| w[0] == "--protocol" && w[1] == "1") {
        eprintln!("fake host: expected --protocol 1, got {args:?}");
        std::process::exit(2);
    }
    if env_flag("FAKE_CRASH_ON_START") {
        eprintln!("fake host: fatal: crash on start");
        std::process::exit(3);
    }
    let max_in_flight = env_u64("FAKE_MAX_IN_FLIGHT", 2) as usize;
    let host = Arc::new(Host {
        out: Mutex::new(std::io::stdout()),
        sched: Mutex::new(Sched::default()),
        cv: Condvar::new(),
        cancels: Mutex::new(HashMap::new()),
        loaded: Mutex::new(None),
        seq: AtomicU64::new(0),
        windows_seen: AtomicU64::new(0),
        started: Instant::now(),
        max_in_flight,
    });
    if env_flag("FAKE_GARBAGE") {
        host.write("this is not json\n");
        host.write("{\"half\": \n");
    }
    eprintln!("fake host: started pid {}", std::process::id());

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match decode_request(&line) {
            Err(e) => host.write(&encode_err(0, ErrorCode::Protocol, &e.0, false)),
            Ok(ParsedRequest::UnknownOp { id, op }) => host.write(&encode_err(
                id,
                ErrorCode::Unsupported,
                &format!("unknown op {op}"),
                false,
            )),
            Ok(ParsedRequest::BadParams { id, message }) => {
                host.write(&encode_err(id, ErrorCode::BadRequest, &message, false))
            }
            Ok(ParsedRequest::Ok { id, op }) => {
                log(op.name(), id);
                handle(&host, id, op);
            }
        }
    }
    eprintln!("fake host: stdin closed, exiting");
}

fn handle(host: &Arc<Host>, id: u64, op: RequestOp) {
    match op {
        RequestOp::Hello { .. } => {
            let mut hostinfo = json!({"name":"sagascript-engine-host","version":"0.0.0-fake",
                "engine":"fake","engine_version":"1"});
            if !env_flag("FAKE_NO_SHA") {
                hostinfo["git_sha"] = SHA.into();
            }
            let advertised = env_u64("FAKE_ADVERTISE_IN_FLIGHT", host.max_in_flight as u64);
            let window = env_f64("FAKE_WINDOW_S", 30.0);
            host.write(&encode_ok(
                id,
                json!({"protocol": env_u64("FAKE_PROTOCOL", 1), "host": hostinfo,
                  "capabilities": {"sample_rate":16000,
                    "max_window_s": env_f64("FAKE_MAX_WINDOW_S", window),
                    "preferred_window_s": window,
                    "preferred_overlap_s": env_f64("FAKE_OVERLAP_S", 6.0),
                    "max_in_flight": advertised, "token_timestamps": true,
                    "languages":["sv"],"compute_units":["cpu"],"min_macos":"14.0"}}),
            ));
        }
        RequestOp::Load {
            model_dir,
            model_id,
            ..
        } => {
            let host = host.clone();
            std::thread::spawn(move || {
                host.write(&encode_event(
                    id,
                    "load_progress",
                    json!({"phase":"compiling"}),
                ));
                std::thread::sleep(Duration::from_millis(env_u64("FAKE_LOAD_DELAY_MS", 0)));
                if model_dir.contains("missing") {
                    host.write(&encode_err(
                        id,
                        ErrorCode::ModelMissing,
                        "no such model",
                        false,
                    ));
                    return;
                }
                *host.loaded.lock().unwrap() = Some(model_id.clone());
                host.write(&encode_ok(
                    id,
                    json!({"model_id":model_id,"load_ms":1,"compiled":false,"window_s":enforced_window_s(),
                      "frame_s":0.08,"vocab_size":1025,"blank_id":1024}),
                ));
            });
        }
        RequestOp::TranscribeWindow {
            pcm_path,
            offset_samples,
            num_samples,
            priority,
            ..
        } => {
            let n = host.windows_seen.fetch_add(1, Ordering::SeqCst);
            if host.loaded.lock().unwrap().is_none() {
                host.write(&encode_err(id, ErrorCode::NotLoaded, "load first", false));
                return;
            }
            let max_samples = (enforced_window_s() * 16000.0) as u64;
            if num_samples > max_samples {
                host.write(&encode_err(
                    id,
                    ErrorCode::BadRequest,
                    "window too long",
                    false,
                ));
                return;
            }
            if env_flag("FAKE_FLOOD") {
                let chunk = vec![b'x'; 1 << 20];
                let mut o = host.out.lock().unwrap();
                for _ in 0..17 {
                    let _ = o.write_all(&chunk);
                }
                let _ = o.flush();
                drop(o);
                std::thread::sleep(Duration::from_secs(30));
                return;
            }
            if env_flag("FAKE_CRASH_ON_WINDOW") {
                eprintln!("fake host: fatal: crash mid-request {id}");
                std::process::exit(4);
            }
            if let Ok(marker) = std::env::var("FAKE_CRASH_ONCE_FILE") {
                if !std::path::Path::new(&marker).exists() {
                    let _ = std::fs::write(&marker, b"crashed");
                    eprintln!("fake host: fatal: crash-once mid-request {id}");
                    std::process::exit(4);
                }
            }
            if env_flag("FAKE_HANG") {
                return;
            }
            if env_flag("FAKE_BUSY") {
                let s = host.sched.lock().unwrap();
                if s.running + s.queue.len() >= host.max_in_flight {
                    drop(s);
                    host.write(&encode_err(id, ErrorCode::Busy, "queue full", true));
                    return;
                }
            }
            let cancel = Arc::new(AtomicBool::new(false));
            host.cancels.lock().unwrap().insert(id, cancel.clone());
            let seq = host.seq.fetch_add(1, Ordering::SeqCst);
            let rank = if priority == Priority::Interactive {
                0
            } else {
                1
            };
            host.sched.lock().unwrap().queue.push(Job { seq, rank, id });
            let host = host.clone();
            std::thread::spawn(move || {
                run_window(
                    &host,
                    id,
                    n,
                    &pcm_path,
                    offset_samples,
                    num_samples,
                    priority,
                    &cancel,
                );
            });
        }
        RequestOp::Cancel { target } => {
            let flag = host.cancels.lock().unwrap().get(&target).cloned();
            let found = flag.is_some();
            if let Some(f) = flag {
                if !env_flag("FAKE_IGNORE_CANCEL") {
                    f.store(true, Ordering::SeqCst);
                    host.cv.notify_all();
                }
            }
            host.write(&encode_ok(
                id,
                json!({"cancelled": found && !env_flag("FAKE_IGNORE_CANCEL")}),
            ));
        }
        RequestOp::Status => {
            let s = host.sched.lock().unwrap();
            let in_flight = s.running + s.queue.len();
            drop(s);
            let model = host.loaded.lock().unwrap().clone();
            host.write(&encode_ok(
                id,
                json!({"state": if in_flight > 0 {"busy"} else if model.is_some() {"ready"} else {"idle"},
                  "model_id": model, "in_flight": in_flight, "rss_bytes": 1,
                  "uptime_s": host.started.elapsed().as_secs_f64()}),
            ));
        }
        RequestOp::Ping => host.write(&encode_ok(id, json!({}))),
        RequestOp::Unload => {
            *host.loaded.lock().unwrap() = None;
            host.write(&encode_ok(id, json!({})));
        }
        RequestOp::Shutdown => {
            host.write(&encode_ok(id, json!({})));
            eprintln!("fake host: shutdown requested");
            std::process::exit(0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_window(
    host: &Host,
    id: u64,
    nth: u64,
    pcm_path: &str,
    offset: u64,
    num: u64,
    priority: Priority,
    cancel: &AtomicBool,
) {
    // Wait for a slot: best (rank, seq) first, at most max_in_flight running.
    {
        let mut s = host.sched.lock().unwrap();
        loop {
            if cancel.load(Ordering::SeqCst) {
                s.queue.retain(|j| j.id != id);
                drop(s);
                finish_cancelled(host, id);
                return;
            }
            let best = s.queue.iter().min_by_key(|j| (j.rank, j.seq)).map(|j| j.id);
            if s.running < host.max_in_flight && best == Some(id) {
                s.queue.retain(|j| j.id != id);
                s.running += 1;
                break;
            }
            s = host
                .cv
                .wait_timeout(s, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
    let mut delay = env_u64("FAKE_WINDOW_DELAY_MS", 0);
    if priority == Priority::Batch {
        delay += env_u64("FAKE_BATCH_DELAY_MS", 0);
    }
    if nth == 0 {
        delay += env_u64("FAKE_FIRST_SLOW_MS", 0);
    }
    let end = Instant::now() + Duration::from_millis(delay);
    let mut was_cancelled = false;
    while Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            was_cancelled = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = if was_cancelled {
        Err(())
    } else {
        Ok(read_tokens(pcm_path, offset, num))
    };
    {
        let mut s = host.sched.lock().unwrap();
        s.running -= 1;
    }
    host.cv.notify_all();
    host.cancels.lock().unwrap().remove(&id);
    match result {
        Err(()) => host.write(&encode_err(id, ErrorCode::Cancelled, "cancelled", false)),
        Ok(Err(msg)) => host.write(&encode_err(id, ErrorCode::Engine, &msg, false)),
        Ok(Ok(tokens)) => host.write(&encode_ok(
            id,
            json!({"tokens": tokens, "audio_s": num as f64 / 16000.0,
              "timings": {"preprocess_ms":1,"encode_ms":2,"decode_ms":3}}),
        )),
    }
}

fn finish_cancelled(host: &Host, id: u64) {
    host.cancels.lock().unwrap().remove(&id);
    host.cv.notify_all();
    host.write(&encode_err(id, ErrorCode::Cancelled, "cancelled", false));
}

fn read_tokens(path: &str, offset: u64, num: u64) -> Result<Vec<Value>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let mut tokens = Vec::new();
    let mut k = 0u64;
    while k * TOKEN_SAMPLES < num {
        f.seek(SeekFrom::Start((offset + k * TOKEN_SAMPLES) * 4))
            .map_err(|e| e.to_string())?;
        let mut b = [0u8; 4];
        f.read_exact(&mut b).map_err(|e| format!("read: {e}"))?;
        let value = f32::from_le_bytes(b);
        let tok = (value as u64) / TOKEN_SAMPLES;
        tokens.push(json!({"id": tok, "text": format!("\u{2581}t{tok}"),
            "start": k as f64 * 0.5, "duration": 0.4}));
        k += 1;
    }
    Ok(tokens)
}
