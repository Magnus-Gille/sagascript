//! The audio callback's path does not allocate (SAGA-563, design §4.1).
//!
//! A counting global allocator counts allocations per thread, so the other
//! tests of this binary (and the harness) cannot disturb the measurement. The
//! callback side is [`BlockSink`]; the writer side recycles through
//! [`TrackReceiver`] on the same thread here so that nothing needs a second
//! thread. Allocation on the *writer* side is allowed and not measured.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use sagascript_core::recording::capture::{track_channel, ChannelConfig, NativeFormat};

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    // `try_with` so that an allocation during thread teardown cannot panic.
    let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_during(work: impl FnOnce()) -> u64 {
    let before = ALLOCATIONS.with(Cell::get);
    work();
    ALLOCATIONS.with(Cell::get) - before
}

#[test]
fn pushing_dropping_and_signalling_in_a_callback_allocate_nothing() {
    let format = NativeFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let (mut sink, mut receiver) = track_channel(
        format,
        ChannelConfig {
            block_frames: 512,
            blocks: 16,
        },
    );
    let handle = sink.handle();
    let wide: Vec<f32> = (0..4 * 480).map(|i| (i % 100) as f32 / 100.0).collect();
    let stereo = vec![0.25f32; 2 * 480];

    // Warm up anything lazy in std.
    sink.push(&stereo, 0);
    while let Some(block) = receiver.pop() {
        receiver.recycle(block);
    }

    let count = allocations_during(|| {
        for round in 0..2_000u64 {
            // The normal path, a device with more channels, an overrun (the
            // pool is not drained for a while), and a signal.
            sink.push(&stereo, round * 10_000_000);
            sink.push_pair(&wide, 4, (2, 3), round * 10_000_000);
            if round % 7 == 0 {
                sink.drop_frames(480);
                handle.signal(sinkless_signal());
            }
            if round % 5 == 0 {
                // The writer is slow now and then: the pool runs dry.
                continue;
            }
            while let Some(block) = receiver.pop() {
                receiver.recycle(block);
            }
        }
    });
    assert_eq!(count, 0, "the callback side allocated");
    assert!(receiver.dropped_total() > 0, "the overrun path was taken");
}

fn sinkless_signal() -> sagascript_core::recording::capture::SourceSignal {
    sagascript_core::recording::capture::SourceSignal::DeviceLost
}
