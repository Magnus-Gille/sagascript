//! Free disk space (design §4.3): the worst-case rate, the thresholds, and the
//! seam for reading the free space.
//!
//! The estimate is **uncompressed**: FLAC of noise is nearly as large as PCM,
//! so a recording must not assume its ratio. The thresholds are time-based
//! (warn when ten minutes of uncompressed audio would no longer fit, stop when
//! one minute would not), with floors so that a tiny mono track still stops
//! well before the disk is actually full. Polling can miss the moment a disk
//! fills, which is why a write error is handled as a case of its own (the
//! session cuts the track back; see `RecordingWriter::abort_track`).

use std::io;
use std::path::Path;
use std::time::Duration;

/// Reads the space a recording can still use.
pub trait FreeSpace: Send + Sync {
    /// Bytes available to this user on the volume that holds `dir` (or the
    /// nearest existing parent of it).
    fn available_bytes(&self, dir: &Path) -> io::Result<u64>;
}

/// The real volume, through `fs2`.
#[derive(Debug, Default, Clone, Copy)]
pub struct VolumeFreeSpace;

impl FreeSpace for VolumeFreeSpace {
    fn available_bytes(&self, dir: &Path) -> io::Result<u64> {
        let mut probe = Some(dir);
        while let Some(path) = probe {
            if path.exists() {
                return fs2::available_space(path);
            }
            probe = path.parent();
        }
        fs2::available_space(Path::new("."))
    }
}

/// Bytes per second of the uncompressed PCM of one track.
pub fn uncompressed_bytes_per_second(sample_rate: u32, channels: u8, bits: u8) -> u64 {
    u64::from(sample_rate) * u64::from(channels) * u64::from(bits.div_ceil(8))
}

/// How full is too full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskPolicy {
    /// Warn below this many free bytes.
    pub low_bytes: u64,
    /// Stop and finalize below this many.
    pub critical_bytes: u64,
    /// How often the session looks (session clock).
    pub poll_interval: Duration,
}

/// What the free space means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiskLevel {
    Ok,
    Low,
    Critical,
}

const MIB: u64 = 1024 * 1024;

impl DiskPolicy {
    /// Thresholds for a recording whose tracks together write
    /// `bytes_per_second` uncompressed: ten minutes (at least 512 MiB) to warn,
    /// one minute (at least 64 MiB) to stop.
    pub fn for_rate(bytes_per_second: u64) -> Self {
        Self {
            low_bytes: (bytes_per_second * 600).max(512 * MIB),
            critical_bytes: (bytes_per_second * 60).max(64 * MIB),
            poll_interval: Duration::from_secs(5),
        }
    }

    pub fn level(&self, free_bytes: u64) -> DiskLevel {
        if free_bytes < self.critical_bytes {
            DiskLevel::Critical
        } else if free_bytes < self.low_bytes {
            DiskLevel::Low
        } else {
            DiskLevel::Ok
        }
    }
}

#[cfg(test)]
#[path = "disk_tests.rs"]
mod tests;
