//! The unit of audio that crosses from the realtime callback to the writer.

/// The format a source delivers: what a track stores (design D7). A device with
/// more than two channels is reduced to a chosen pair by the source, so a block
/// never has more than two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeFormat {
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u8,
}

/// Interleaved `f32` samples with the host-clock time of the first frame.
///
/// Blocks are allocated once, when a session starts (see
/// [`track_channel`](super::track_channel)), and travel back and forth between
/// the callback and the writer for the whole recording. Nothing about a block
/// allocates after that: [`Block::prepare`] only moves the valid length.
pub struct Block {
    samples: Vec<f32>,
    channels: usize,
    frames: usize,
    pub(crate) host_ns: u64,
    pub(crate) dropped_before: u64,
}

impl Block {
    /// A block that can hold `capacity_frames` frames of `channels` channels.
    pub(crate) fn with_capacity(capacity_frames: usize, channels: usize) -> Self {
        Self {
            samples: vec![0.0; capacity_frames * channels],
            channels,
            frames: 0,
            host_ns: 0,
            dropped_before: 0,
        }
    }

    /// The most frames the block holds.
    pub fn capacity_frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// Frames in the block.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// The valid interleaved samples.
    pub fn samples(&self) -> &[f32] {
        &self.samples[..self.frames * self.channels]
    }

    /// Host-clock time (see [`host_now_ns`](super::host_now_ns)) of the first frame.
    pub fn host_ns(&self) -> u64 {
        self.host_ns
    }

    /// Frames the callback had to drop immediately before this block because
    /// the pool was empty (the writer was behind): an overrun.
    pub fn dropped_before(&self) -> u64 {
        self.dropped_before
    }

    /// Make the block hold `frames` frames and return them to be filled.
    pub(crate) fn prepare(&mut self, frames: usize) -> &mut [f32] {
        debug_assert!(frames <= self.capacity_frames());
        self.frames = frames;
        let len = frames * self.channels;
        &mut self.samples[..len]
    }
}
