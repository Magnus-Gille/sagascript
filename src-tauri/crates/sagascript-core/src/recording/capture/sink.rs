//! The two ends of a track's capture queue: [`BlockSink`] for the audio
//! callback and [`TrackReceiver`] for the writer thread.
//!
//! # What the callback may do
//!
//! Everything the sink does in a callback is plain arithmetic, a copy into a
//! block that already exists, and atomic loads and stores. It never
//! allocates, takes a lock, waits, logs or emits an event:
//!
//! * the pool of blocks is allocated before the stream starts and recycled
//!   through a second [`spsc`](super::spsc) queue, so a block is *popped from
//!   the pool, filled, and pushed to the writer* without moving any heap data;
//! * when the pool is empty (the writer is behind) the callback does not
//!   wait: it counts the frames it has to drop and puts that count on the next
//!   block that gets through (`dropped_before`), which is how the writer
//!   learns the exact length and position of an overrun, never from a clock;
//! * the peak level is a `fetch_max` on an atomic that the writer reads;
//! * a source reports a device or format change by setting a bit in an atomic
//!   word ([`SignalHandle`]); the writer polls it.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use super::block::{Block, NativeFormat};
use super::spsc::{self, Consumer, Producer};

/// Sizes of one track's block pool.
#[derive(Clone, Copy, Debug)]
pub struct ChannelConfig {
    /// Frames one block holds. A callback that delivers more is split.
    pub block_frames: usize,
    /// Blocks in the pool; the most that can wait for the writer.
    pub blocks: usize,
}

impl ChannelConfig {
    /// A pool that holds about `seconds` of audio when callbacks fill their
    /// blocks half way, at least eight blocks.
    pub fn for_seconds(format: NativeFormat, block_frames: usize, seconds: f64) -> Self {
        let block_frames = block_frames.max(64);
        let per_second = f64::from(format.sample_rate) / (block_frames as f64 / 2.0);
        Self {
            block_frames,
            blocks: ((per_second * seconds).ceil() as usize).max(8),
        }
    }
}

/// An event a source reports besides audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceSignal {
    /// The device stopped delivering and may come back (a stream error that is
    /// not a removal). If audio resumes, the gap is recorded as `device_loss`.
    DeviceLost,
    /// The device was removed or replaced: the track ends (`device_changed`).
    DeviceChanged,
    /// The device now delivers another rate or channel count: the track ends
    /// (`format_changed`), because a FLAC stream cannot change either.
    FormatChanged(NativeFormat),
    /// The system is going to sleep or has just woken: the track ends
    /// (`system_sleep`).
    SystemSleep,
}

const DEVICE_LOST: u32 = 1;
const DEVICE_CHANGED: u32 = 2;
const FORMAT_CHANGED: u32 = 4;
const SYSTEM_SLEEP: u32 = 8;

/// What the writer finds when it asks for the pending signals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    pub device_lost: bool,
    pub device_changed: bool,
    pub format_changed: Option<NativeFormat>,
    pub system_sleep: bool,
}

impl Signals {
    pub fn any(&self) -> bool {
        self.device_lost
            || self.device_changed
            || self.format_changed.is_some()
            || self.system_sleep
    }
}

struct TrackShared {
    /// Frames the callback dropped, in total (for the status snapshot).
    dropped_total: AtomicU64,
    /// Frames dropped after the last block that got through, left by a sink
    /// that was dropped with a count pending.
    trailing_dropped: AtomicU64,
    /// Peak absolute sample of each channel since the writer last took it, as
    /// `f32` bits (non-negative floats order like their bit patterns).
    peaks: [AtomicU32; 2],
    signals: AtomicU32,
    new_rate: AtomicU32,
    new_channels: AtomicU32,
    sink_closed: AtomicBool,
}

/// A cloneable, `Send + Sync` handle for reporting [`SourceSignal`]s from any
/// thread (a stream's error callback runs on another thread than its data
/// callback). It only sets an atomic.
#[derive(Clone)]
pub struct SignalHandle {
    shared: Arc<TrackShared>,
}

impl SignalHandle {
    /// Report an event. Safe to call from a realtime callback.
    pub fn signal(&self, signal: SourceSignal) {
        let bits = match signal {
            SourceSignal::DeviceLost => DEVICE_LOST,
            SourceSignal::DeviceChanged => DEVICE_CHANGED,
            SourceSignal::SystemSleep => SYSTEM_SLEEP,
            SourceSignal::FormatChanged(format) => {
                self.shared
                    .new_rate
                    .store(format.sample_rate, Ordering::Relaxed);
                self.shared
                    .new_channels
                    .store(u32::from(format.channels), Ordering::Relaxed);
                FORMAT_CHANGED
            }
        };
        // Release: everything the callback pushed before reporting is visible
        // to a writer that sees the bit.
        self.shared.signals.fetch_or(bits, Ordering::Release);
    }
}

/// The audio-callback end. One per track; it is `Send` (it moves into the
/// callback) and used from one thread.
pub struct BlockSink {
    format: NativeFormat,
    queue: Producer<Block>,
    pool: Consumer<Block>,
    shared: Arc<TrackShared>,
    pending_dropped: u64,
}

/// The writer end: blocks in, blocks back, plus the counters and signals.
pub struct TrackReceiver {
    format: NativeFormat,
    queue: Consumer<Block>,
    pool: Producer<Block>,
    shared: Arc<TrackShared>,
    capacity_blocks: usize,
}

/// Create a track's queue, pool and shared counters. All allocation happens
/// here, before any stream runs.
pub fn track_channel(format: NativeFormat, config: ChannelConfig) -> (BlockSink, TrackReceiver) {
    let (mut pool_tx, pool_rx) = spsc::channel(config.blocks);
    let (queue_tx, queue_rx) = spsc::channel(config.blocks);
    for _ in 0..config.blocks {
        let block = Block::with_capacity(config.block_frames, usize::from(format.channels));
        if pool_tx.try_push(block).is_err() {
            unreachable!("the pool queue holds at least `blocks` blocks");
        }
    }
    let shared = Arc::new(TrackShared {
        dropped_total: AtomicU64::new(0),
        trailing_dropped: AtomicU64::new(0),
        peaks: [AtomicU32::new(0), AtomicU32::new(0)],
        signals: AtomicU32::new(0),
        new_rate: AtomicU32::new(0),
        new_channels: AtomicU32::new(0),
        sink_closed: AtomicBool::new(false),
    });
    (
        BlockSink {
            format,
            queue: queue_tx,
            pool: pool_rx,
            shared: Arc::clone(&shared),
            pending_dropped: 0,
        },
        TrackReceiver {
            format,
            queue: queue_rx,
            pool: pool_tx,
            shared,
            capacity_blocks: config.blocks,
        },
    )
}

impl BlockSink {
    /// The format of every block this sink produces.
    pub fn format(&self) -> NativeFormat {
        self.format
    }

    /// A handle for reporting [`SourceSignal`]s from any thread.
    pub fn handle(&self) -> SignalHandle {
        SignalHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Deliver `frames` frames captured starting at `host_ns`; `fill` writes
    /// frame `i` (counted from the start of this call) into the slice it is
    /// given, which has `channels` samples. Splits across blocks when the
    /// callback is larger than a block and drops (and counts) what does not fit.
    ///
    /// Realtime-safe: no allocation, lock or wait.
    pub fn push_with(
        &mut self,
        frames: usize,
        host_ns: u64,
        mut fill: impl FnMut(usize, &mut [f32]),
    ) {
        let channels = usize::from(self.format.channels);
        let rate = u64::from(self.format.sample_rate);
        let mut done = 0;
        while done < frames {
            let Some(mut block) = self.pool.try_pop() else {
                self.drop_frames((frames - done) as u64);
                return;
            };
            let take = (frames - done).min(block.capacity_frames());
            let out = block.prepare(take);
            let mut peaks = [0.0f32; 2];
            for (offset, frame) in out.chunks_exact_mut(channels).enumerate() {
                fill(done + offset, frame);
                for (peak, sample) in peaks.iter_mut().zip(frame.iter()) {
                    let magnitude = sample.abs();
                    // `>` is false for NaN, so a NaN never becomes the peak.
                    if magnitude > *peak {
                        *peak = magnitude;
                    }
                }
            }
            for (slot, peak) in self.shared.peaks.iter().zip(peaks).take(channels) {
                slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
            }
            block.host_ns = host_ns + (done as u64 * 1_000_000_000) / rate;
            block.dropped_before = std::mem::take(&mut self.pending_dropped);
            if let Err(rejected) = self.queue.try_push(block) {
                // Cannot happen: the queue holds as many blocks as the pool.
                // Count the frames as lost rather than panic in a callback.
                self.pending_dropped = rejected.dropped_before + take as u64;
                self.shared
                    .dropped_total
                    .fetch_add(take as u64, Ordering::Relaxed);
                // The block is released here, which frees memory: a path that
                // exists only so that a logic error cannot crash the callback.
                drop(rejected);
            }
            done += take;
        }
    }

    /// Deliver interleaved samples that are already in the track's format.
    pub fn push(&mut self, interleaved: &[f32], host_ns: u64) {
        let channels = usize::from(self.format.channels);
        self.push_with(interleaved.len() / channels, host_ns, |i, out| {
            out.copy_from_slice(&interleaved[i * channels..(i + 1) * channels]);
        });
    }

    /// Deliver a device's interleaved samples, keeping the channel pair
    /// `pair` (zero-based indexes; for a mono track only `pair.0` is used).
    /// The device must have more channels than either index.
    pub fn push_pair(
        &mut self,
        interleaved: &[f32],
        device_channels: usize,
        pair: (usize, usize),
        host_ns: u64,
    ) {
        let stored = usize::from(self.format.channels);
        self.push_with(interleaved.len() / device_channels, host_ns, |i, out| {
            let frame = &interleaved[i * device_channels..(i + 1) * device_channels];
            out[0] = frame[pair.0];
            if stored == 2 {
                out[1] = frame[pair.1];
            }
        });
    }

    /// Count `frames` as lost before the next block: the source knows it
    /// dropped them (or a test injects an overrun).
    pub fn drop_frames(&mut self, frames: u64) {
        self.pending_dropped += frames;
        self.shared
            .dropped_total
            .fetch_add(frames, Ordering::Relaxed);
    }
}

impl Drop for BlockSink {
    fn drop(&mut self) {
        self.shared
            .trailing_dropped
            .store(self.pending_dropped, Ordering::Relaxed);
        self.shared.sink_closed.store(true, Ordering::Release);
    }
}

impl TrackReceiver {
    pub fn format(&self) -> NativeFormat {
        self.format
    }

    /// The next block, if any. Give it back with [`recycle`](Self::recycle).
    pub fn pop(&mut self) -> Option<Block> {
        self.queue.try_pop()
    }

    /// Return a block to the pool.
    pub fn recycle(&mut self, block: Block) {
        if self.pool.try_push(block).is_err() {
            unreachable!("the pool queue holds at least as many blocks as exist");
        }
    }

    /// Blocks waiting for the writer.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Blocks the pool was created with.
    pub fn capacity_blocks(&self) -> usize {
        self.capacity_blocks
    }

    /// Take the signals reported since the last call.
    pub fn take_signals(&self) -> Signals {
        let bits = self.shared.signals.swap(0, Ordering::Acquire);
        Signals {
            device_lost: bits & DEVICE_LOST != 0,
            device_changed: bits & DEVICE_CHANGED != 0,
            system_sleep: bits & SYSTEM_SLEEP != 0,
            format_changed: (bits & FORMAT_CHANGED != 0).then(|| NativeFormat {
                sample_rate: self.shared.new_rate.load(Ordering::Relaxed),
                channels: self.shared.new_channels.load(Ordering::Relaxed) as u8,
            }),
        }
    }

    /// The peak of each channel since the last call (0.0 to 1.0 and above for
    /// a clipping float device), resetting it.
    pub fn take_peaks(&self) -> [f32; 2] {
        [
            f32::from_bits(self.shared.peaks[0].swap(0, Ordering::Relaxed)),
            f32::from_bits(self.shared.peaks[1].swap(0, Ordering::Relaxed)),
        ]
    }

    /// Frames the callback dropped in total.
    pub fn dropped_total(&self) -> u64 {
        self.shared.dropped_total.load(Ordering::Relaxed)
    }

    /// Whether the sink was dropped (the stream is gone) and, if so, the
    /// frames it dropped after its last delivered block.
    pub fn closed(&self) -> Option<u64> {
        self.shared
            .sink_closed
            .load(Ordering::Acquire)
            .then(|| self.shared.trailing_dropped.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
#[path = "sink_tests.rs"]
mod tests;
