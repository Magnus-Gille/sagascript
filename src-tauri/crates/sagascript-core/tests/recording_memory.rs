//! Memory stays flat while recording and while recovering (SAGA-562, design §7):
//! the peak of live heap bytes is measured with a counting allocator, over
//! recordings of very different lengths, so the test asserts a bound and not a
//! wall-clock time. The allocator counts the whole process, so only one test
//! of this file may run at a time: the other is ignored and run by itself.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{self, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use sagascript_core::recording::{
    Client, FlacEncoder, NewRecording, NewTrack, Quality, RecordingStore, TrackEncoder, TrackSink,
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

/// Peak live bytes above the level at the start of `work`.
fn peak_during<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let result = work();
    (
        result,
        PEAK.load(Ordering::Relaxed).saturating_sub(baseline),
    )
}

/// A sink that keeps nothing.
#[derive(Default)]
struct Discard(u64);

impl Write for Discard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Discard {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if let SeekFrom::Start(p) = position {
            self.0 = p;
        }
        Ok(self.0)
    }
}

impl TrackSink for Discard {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Audio that never repeats: a tone that sweeps with the block number, a
/// varying level and a noise floor, and a block of full-band noise every tenth
/// block (the study's soak looped one clip; design §11 finding 23).
struct Source {
    state: u64,
    block: usize,
    buffer: Vec<f32>,
}

impl Source {
    fn new() -> Self {
        Self {
            state: 0x2545_F491_4F6C_DD1D,
            block: 0,
            buffer: vec![0.0; 4096],
        }
    }

    fn next_block(&mut self) -> &[f32] {
        let block = self.block;
        self.block += 1;
        let frequency = 100.0 + (block % 9973) as f64 * 1.7;
        let amplitude = 0.05 + 0.4 * ((block * 7919) % 100) as f64 / 100.0;
        let noisy = block % 10 == 9;
        for (i, slot) in self.buffer.iter_mut().enumerate() {
            self.state ^= self.state >> 12;
            self.state ^= self.state << 25;
            self.state ^= self.state >> 27;
            let noise =
                (self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64 / 16_777_216.0 - 0.5;
            let turns = (block * 4096 + i) as f64 * frequency / 48_000.0;
            let tone = amplitude * (turns.fract() * std::f64::consts::TAU).sin();
            *slot = if noisy {
                noise as f32
            } else {
                (tone + 0.01 * noise) as f32
            };
        }
        &self.buffer
    }
}

/// `blocks` blocks of that audio into a sink that keeps nothing; the bytes of
/// FLAC frames written.
fn encode(blocks: usize) -> u64 {
    let mut source = Source::new();
    let mut encoder = FlacEncoder::new(Discard::default(), 48_000, 1, 24).unwrap();
    for _ in 0..blocks {
        encoder.push(source.next_block()).unwrap();
    }
    Box::new(encoder).finish().unwrap().frame_bytes
}

#[test]
fn encoding_and_recovering_use_memory_that_does_not_depend_on_the_length() {
    // Warm up anything lazily allocated (symphonia's tables, the thread locals).
    encode(12);

    let (short_bytes, short_peak) = peak_during(|| encode(30));
    let (long_bytes, long_peak) = peak_during(|| encode(3000));
    assert!(
        long_bytes > 50 * short_bytes,
        "the long run really is about 100 times longer"
    );
    assert!(long_bytes > 8 * 1024 * 1024, "{long_bytes} bytes of FLAC");
    assert!(short_peak < 2 * 1024 * 1024, "{short_peak}");
    assert!(
        long_peak <= short_peak + 128 * 1024,
        "the peak grew with the length: {short_peak} for 30 blocks, {long_peak} for 3000"
    );

    // A killed recording of that length: recovery reads a bounded window.
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store
        .create(NewRecording {
            title: "Long".into(),
            started_at: "2026-11-06T09:30:00.000+01:00".into(),
            client: Client {
                kind: "desktop".into(),
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
                clock_domain: "mach_continuous".into(),
                start_ns: 0,
                resampled_from: None,
                device: None,
            }],
        })
        .unwrap();
    let mut source = Source::new();
    for _ in 0..1500 {
        writer.push("mic", source.next_block()).unwrap();
    }
    let folder = writer.folder().to_path_buf();
    drop(writer);
    let file_size = std::fs::metadata(folder.join("mic.flac")).unwrap().len();
    assert!(file_size > 3 * 1024 * 1024, "{file_size} bytes");

    let (recovered, recover_peak) =
        peak_during(|| sagascript_core::recording::recover_folder(&folder));
    assert_eq!(recovered.unwrap().tracks[0].frames_kept, 1500 * 4096);
    assert!(
        recover_peak < file_size as usize / 2,
        "recovery of a {file_size}-byte file peaked at {recover_peak} bytes"
    );
    assert!(recover_peak < 2 * 1024 * 1024, "{recover_peak}");
}

/// The multi-hour case of SAGA-562: hours of non-repeating audio in the same
/// few hundred kilobytes. Three hours take about five minutes on one core even
/// in a debug build (measured: 126 562 blocks, 1.2 GB of FLAC, peak 244 578
/// bytes against 242 764 for 100 blocks), so it is run by hand:
/// `cargo test -p sagascript-core --test recording_memory -- --ignored --test-threads=1 --no-capture`
/// (`SAGASCRIPT_SOAK_HOURS` changes the three hours).
#[test]
#[ignore = "a multi-hour soak (about five minutes); run it with --ignored --test-threads=1"]
fn hours_of_audio_in_flat_memory() {
    let hours: f64 = std::env::var("SAGASCRIPT_SOAK_HOURS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3.0);
    let blocks = (hours * 3600.0 * 48_000.0 / 4096.0) as usize;
    encode(12);
    let (_, short_peak) = peak_during(|| encode(100));
    let (bytes, long_peak) = peak_during(|| encode(blocks));
    println!(
        "{hours} h = {blocks} blocks, {} MB of FLAC, peak {long_peak} bytes (100 blocks: {short_peak})",
        bytes / 1_000_000
    );
    assert!(
        long_peak <= short_peak + 128 * 1024,
        "{short_peak} -> {long_peak}"
    );
}
