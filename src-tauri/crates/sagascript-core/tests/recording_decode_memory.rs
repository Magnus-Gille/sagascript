//! `decode_tracks` holds no whole track (SAGA-564): the peak of live heap
//! bytes while iterating a short and a long recording is measured with a
//! counting allocator and must not grow with the length. The allocator counts
//! the whole process, so this file holds one test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use sagascript_core::recording::{
    decode_tracks, Client, EndedReason, NewRecording, NewTrack, Quality, RecordingStore,
    CHUNK_SAMPLES,
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

fn peak_during<T>(work: impl FnOnce() -> T) -> (T, usize) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let result = work();
    (
        result,
        PEAK.load(Ordering::Relaxed).saturating_sub(baseline),
    )
}

/// A finished recording of `blocks` blocks of 4096 frames at 48 kHz mono.
fn recording_of(root: &TempDir, blocks: usize) -> std::path::PathBuf {
    let store = RecordingStore::new(root.path().join(format!("r{blocks}")));
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
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut block = vec![0.0f32; 4096];
    for index in 0..blocks {
        for (i, slot) in block.iter_mut().enumerate() {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let noise = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / 16_777_216.0;
            let tone = ((index * 4096 + i) as f32 * 0.03).sin();
            *slot = 0.3 * tone + 0.05 * (noise - 0.5);
        }
        writer.push("mic", &block).unwrap();
    }
    let folder = writer.folder().to_path_buf();
    writer.finish(EndedReason::Stopped).unwrap();
    folder
}

/// Decode everything, keeping nothing: (samples, chunks, largest chunk).
fn drain(folder: &std::path::Path) -> (usize, usize, usize) {
    let (mut samples, mut chunks, mut largest) = (0, 0, 0);
    for chunk in decode_tracks(folder).unwrap() {
        let chunk = chunk.unwrap();
        samples += chunk.samples.len();
        chunks += 1;
        largest = largest.max(chunk.samples.len());
    }
    (samples, chunks, largest)
}

#[test]
fn decoding_uses_memory_that_does_not_depend_on_the_length() {
    let root = TempDir::new();
    let short = recording_of(&root, 40); // 3.4 s
    let long = recording_of(&root, 1500); // 128 s
    drain(&short); // warm up lazily allocated tables

    let ((short_samples, _, _), short_peak) = peak_during(|| drain(&short));
    let ((long_samples, long_chunks, largest), long_peak) = peak_during(|| drain(&long));

    assert!(long_samples > 30 * short_samples, "about 37 times longer");
    assert!(long_chunks > 60, "{long_chunks} chunks");
    assert!(largest <= CHUNK_SAMPLES);
    // The whole long track as f32 would be 8 MB; a chunk is 128 KB.
    assert!(long_samples * 4 > 7_000_000);
    assert!(long_peak < 2 * 1024 * 1024, "{long_peak} bytes");
    assert!(
        long_peak <= short_peak + 128 * 1024,
        "the peak grew with the length: {short_peak} for 40 blocks, {long_peak} for 1500"
    );
}
