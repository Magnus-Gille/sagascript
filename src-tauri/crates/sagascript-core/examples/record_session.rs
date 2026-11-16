//! A developer tool for the hardware checks of the recorder (SAGA-563). It is
//! **not** run by any test: it opens the default microphone.
//!
//! ```text
//! cargo run -p sagascript-core --example record_session --features record -- \
//!     <recordings-dir> [seconds (default 10)] [lossless|compact (default lossless)] [channel-pair e.g. 3,4]
//! ```
//!
//! It records through a `RecordingSession` with `MicSource`, prints the status
//! once a second (elapsed, peak, gaps, dropped frames, stalls, bytes, free
//! disk) and the manifest when it is done. Stop early with Ctrl-C and run
//! `sagascript_core::recording::RecordingStore::recover_pending` (or phase
//! 1c's `recordings recover`) to see recovery finish the folder.
//!
//! A terminal gets its own microphone permission; the permission prompt of the
//! app itself is checked with `cargo tauri build --debug` once phase 1c has a
//! Record window.
use std::time::{Duration, Instant};

use sagascript_core::recording::capture::{CaptureSource, MicConfig, MicSource};
use sagascript_core::recording::{
    Client, Quality, RecordingSession, RecordingStore, SessionConfig, SourceSpec,
};

fn started_at() -> String {
    let fallback = "1970-01-01T00:00:00.000+00:00".to_string();
    let Ok(output) = std::process::Command::new("date")
        .arg("+%Y-%m-%dT%H:%M:%S.000%z")
        .output()
    else {
        return fallback;
    };
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.len() < 5 || !text.is_ascii() {
        return fallback;
    }
    // +0100 -> +01:00
    let (head, zone) = text.split_at(text.len() - 2);
    format!("{head}:{zone}")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("usage: record_session <recordings-dir> [seconds] [lossless|compact] [pair]");
        std::process::exit(2);
    };
    let seconds: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let quality = match args.get(3).map(String::as_str) {
        Some("compact") => Quality::Compact,
        _ => Quality::Lossless,
    };
    let mut mic = MicConfig::default();
    if let Some((a, b)) = args.get(4).and_then(|p| p.split_once(',')) {
        mic.channel_pair = (a.parse().expect("channel"), b.parse().expect("channel"));
    }

    let source = MicSource::open(mic).expect("open the default input device");
    eprintln!("input: {:?} {:?}", source.format(), source.negotiated());
    let config = SessionConfig::new(
        "Hardware check",
        started_at(),
        Client {
            kind: "desktop".into(),
            app_version: "dev".into(),
            build: "record_session example".into(),
            extra: Default::default(),
        },
        quality,
    );
    let session = RecordingSession::new(RecordingStore::new(dir), config);
    session
        .start(vec![SourceSpec::mic(source)])
        .expect("start recording");
    eprintln!("recording to {:?}", session.folder());

    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until && !session.state().is_terminal() {
        std::thread::sleep(Duration::from_secs(1));
        let status = session.status();
        let track = &status.tracks[0];
        eprintln!(
            "{:>5.1}s peak {:.3} gaps {} dropped {} stalled {} lost {} ended {:?} bytes {} free {:?} warnings {:?}",
            status.elapsed.as_secs_f64(),
            track.peak,
            track.gap_count,
            track.dropped_frames,
            track.stalled,
            track.device_lost,
            track.ended,
            status.bytes_written,
            status.free_disk_bytes,
            status.warnings,
        );
    }
    match session.stop() {
        Ok(outcome) => {
            eprintln!("ended early: {:?}", outcome.ended_early);
            println!(
                "{}",
                serde_json::to_string_pretty(&outcome.manifest).unwrap()
            );
        }
        Err(error) => {
            eprintln!("failed: {} ({error})", error.code());
            std::process::exit(1);
        }
    }
}
