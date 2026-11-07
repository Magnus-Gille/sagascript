//! A long recording through a whole `RecordingSession` keeps memory flat
//! (SAGA-563, design §6 row 1b), without needing the time it represents.
//!
//! The fake source runs on a manual clock, so a "two hour" recording is two
//! hours of *audio* delivered as fast as the writer thread can encode it. A
//! counting allocator measures live heap bytes (the whole process, so the
//! tests of this file run one at a time under `SERIAL`):
//!
//! * the live heap sampled once a virtual minute must not grow: its range
//!   after the first two minutes stays under 1 MiB however long the run;
//! * its peak above the start stays under a fixed budget that holds the block
//!   pool, the encoder and the writer thread, and not a second of audio more.
//!
//! The finished file is then decoded *streaming* (one packet at a time, so the
//! check itself is flat) and must match the manifest: frames, the MD5 in
//! STREAMINFO, the hash of the frame bytes. That is the decode half of the
//! "also cover the transcription path" request of the ticket; `decode_tracks`
//! (design §4.6) is phase 2.
//!
//! `cargo test --release -p sagascript-core --test recording_session_soak --
//! --ignored --test-threads=1` runs the two-hour version. The default tests
//! run five virtual minutes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sagascript_core::recording::capture::{FakeSignal, FakeSource, ManualClock, NativeFormat};
use sagascript_core::recording::{
    Client, Quality, RecordingSession, RecordingStore, SessionConfig, SourceSpec,
};
use sagascript_testsupport::TempDir;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(by: usize) {
    let now = LIVE.fetch_add(by, Ordering::Relaxed) + by;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            grow(new_size - layout.size());
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

static SERIAL: Mutex<()> = Mutex::new(());

const FORMAT: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};

struct Report {
    manifest: sagascript_core::recording::Manifest,
    folder: std::path::PathBuf,
    /// Live heap bytes above the start, once a virtual minute.
    samples: Vec<usize>,
    peak_above_start: usize,
    overruns_injected: u64,
    wall: Duration,
    _temp: TempDir,
}

fn soak(virtual_seconds: u64, quality: Quality) -> Report {
    let temp = TempDir::new();
    let (source, handle) = FakeSource::with_clock(FORMAT, Arc::new(ManualClock::new()));
    // Noise with a tone on top would compress better; noise is the worst case.
    handle.set_signal(FakeSignal::Noise {
        amplitude: 0.3,
        seed: 99,
    });
    let mut config = SessionConfig::new(
        "Soak",
        "2026-11-06T09:30:00.000+01:00",
        Client {
            kind: "desktop".into(),
            app_version: "test".into(),
            build: "test".into(),
            extra: Default::default(),
        },
        quality,
    );
    config.clock = handle.clock();
    config.detect_sleep = false;
    config.folder_warnings = Some(Vec::new());
    let session = RecordingSession::new(RecordingStore::new(temp.path().join("r")), config);

    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let started = Instant::now();
    session.start(vec![SourceSpec::mic(source)]).unwrap();

    let mut samples = Vec::new();
    let mut overruns = 0;
    for second in 0..virtual_seconds {
        if second > 0 && second % 120 == 0 {
            // Every two minutes the writer "falls behind" for a moment.
            handle.overrun(960);
            overruns += 1;
        }
        for _ in 0..10 {
            handle.deliver(Duration::from_millis(100));
        }
        // Let the writer drain before the next second, as a real-time device
        // would: the queue never holds more than a second.
        while {
            let track = &session.status().tracks[0];
            track.frames_received + track.dropped_frames
                < handle.frames_delivered() + handle.frames_dropped()
        } {
            std::thread::sleep(Duration::from_micros(100));
        }
        if second % 60 == 59 {
            samples.push(LIVE.load(Ordering::Relaxed).saturating_sub(baseline));
        }
    }
    let outcome = session.stop().unwrap();
    let wall = started.elapsed();
    let peak_above_start = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    Report {
        manifest: outcome.manifest,
        folder: outcome.folder,
        samples,
        peak_above_start,
        overruns_injected: overruns,
        wall,
        _temp: temp,
    }
}

/// Decode the track one packet at a time and count frames; the MD5 in
/// STREAMINFO is verified by the decoder.
fn streaming_decode_frames(path: &std::path::Path) -> u64 {
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let stream = MediaSourceStream::new(Box::new(File::open(path).unwrap()), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut reader = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .unwrap()
        .format;
    let track = reader.default_track().unwrap().clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions { verify: true })
        .unwrap();
    let mut frames = 0u64;
    while let Ok(packet) = reader.next_packet() {
        let decoded = decoder.decode(&packet).expect("every frame decodes");
        frames += decoded.frames() as u64;
    }
    assert_eq!(
        decoder.finalize().verify_ok,
        Some(true),
        "STREAMINFO's MD5 matches"
    );
    frames
}

fn check(report: &Report, virtual_seconds: u64, stored_rate: u32) {
    let track = &report.manifest.tracks[0];
    let seconds = virtual_seconds as f64;
    // Frames are the time: 20 ms prime and the delivered seconds, plus the
    // injected overruns, which are part of the timeline.
    let expected =
        (0.02 + seconds + report.overruns_injected as f64 * 0.02) * f64::from(stored_rate);
    assert!(
        (track.frames as f64 - expected).abs() < 2.0,
        "{} vs {expected}",
        track.frames
    );
    assert_eq!(track.gaps.len() as u64, report.overruns_injected);
    assert!(track.clock.anchors.len() as f64 >= seconds / 10.0 - 1.0);
    assert_eq!(
        streaming_decode_frames(&report.folder.join("mic.flac")),
        track.frames
    );

    // Flat: after two virtual minutes the live heap does not drift.
    let settled = &report.samples[2.min(report.samples.len())..];
    if let (Some(min), Some(max)) = (settled.iter().min(), settled.iter().max()) {
        assert!(
            max - min < 1024 * 1024,
            "the live heap drifted by {} bytes: {:?}",
            max - min,
            report.samples
        );
    }
    // And bounded: the pool, the encoder, the buffers, the thread stacks.
    assert!(
        report.peak_above_start < 16 * 1024 * 1024,
        "peak {} bytes above the start",
        report.peak_above_start
    );
    eprintln!(
        "{virtual_seconds} s of audio in {:.1} s ({:.0}x real time): {} frames, {} gaps, {} anchors, file {} bytes, peak {} KiB above start, heap samples (KiB) {:?}",
        report.wall.as_secs_f64(),
        seconds / report.wall.as_secs_f64(),
        track.frames,
        track.gaps.len(),
        track.clock.anchors.len(),
        std::fs::metadata(report.folder.join("mic.flac")).unwrap().len(),
        report.peak_above_start / 1024,
        report.samples.iter().map(|b| b / 1024).collect::<Vec<_>>(),
    );
}

#[test]
fn five_virtual_minutes_of_lossless_audio_keep_memory_flat() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let report = soak(300, Quality::Lossless);
    check(&report, 300, 48_000);
}

#[test]
fn five_virtual_minutes_of_the_compact_tier_keep_memory_flat() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let report = soak(300, Quality::Compact);
    check(&report, 300, 16_000);
}

/// Two hours of audio. Slow in a debug build: run it in release.
#[test]
#[ignore = "two hours of audio; run with --release"]
fn two_virtual_hours_of_lossless_audio_keep_memory_flat() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let report = soak(2 * 3600, Quality::Lossless);
    check(&report, 2 * 3600, 48_000);
}

#[test]
#[ignore = "two hours of audio; run with --release"]
fn two_virtual_hours_of_the_compact_tier_keep_memory_flat() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let report = soak(2 * 3600, Quality::Compact);
    check(&report, 2 * 3600, 16_000);
}
