//! Records the system-audio track: native chunks -> 16 kHz mono -> a temporary
//! spool file on disk (so long meetings do not pile up in RAM while recording),
//! with wall-clock gap filling so the track stays aligned with the microphone.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use tempfile::NamedTempFile;

use super::convert::{silence_to_insert, StreamingConverter};
use super::{CaptureTarget, NativeFormat, SystemAudioCapture};
use crate::error::DictationError;

/// Gaps shorter than this are scheduling jitter, not idle silence.
const GAP_TOLERANCE_MS: u64 = 250;
const SPOOL_PREFIX: &str = "sagascript-system-";
const SPOOL_SUFFIX: &str = ".spool";
const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);

/// User-visible location for in-progress recordings.
pub fn recording_temp_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Sagascript")
        .join("recordings")
}

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Remove spool files left behind by a crashed run. Returns how many were removed.
pub fn clean_stale_spools(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with(SPOOL_PREFIX) && name.ends_with(SPOOL_SUFFIX)) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if old && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The captured system track at 16 kHz mono.
#[derive(Debug, Clone)]
pub struct SystemTrack {
    pub samples: Vec<f32>,
    pub native: NativeFormat,
    /// True when every sample is (near) zero: nothing was playing, or the OS
    /// delivered silence because permission was denied.
    pub silent: bool,
}

pub struct SystemRecorder {
    capture: SystemAudioCapture,
    worker: JoinHandle<Result<SystemTrack, String>>,
    started: Instant,
}

impl SystemRecorder {
    /// Start capturing. May trigger the OS permission prompt (macOS).
    pub fn start(target: &CaptureTarget) -> Result<Self, DictationError> {
        let dir = recording_temp_dir();
        ensure_private_dir(&dir).map_err(|e| {
            DictationError::AudioCaptureError(format!("Cannot create {}: {e}", dir.display()))
        })?;
        clean_stale_spools(&dir);
        let (tx, rx) = mpsc::channel::<Vec<f32>>();
        let started = Instant::now();
        // The callback only copies and sends; conversion happens on the worker.
        let capture = SystemAudioCapture::start(
            target,
            Box::new(move |chunk| {
                let _ = tx.send(chunk.to_vec());
            }),
        )?;
        let native = capture.format();
        let worker = thread::Builder::new()
            .name("sagascript-system-spool".into())
            .spawn(move || spool_worker(rx, native, started, &dir))
            .map_err(|e| DictationError::AudioCaptureError(format!("Spawning spool thread: {e}")))?;
        Ok(Self { capture, worker, started })
    }

    pub fn started_at(&self) -> Instant {
        self.started
    }

    pub fn stop(self) -> Result<SystemTrack, DictationError> {
        self.capture.stop(); // drops the sink, which ends the worker
        self.worker
            .join()
            .map_err(|_| DictationError::AudioCaptureError("System-audio spool thread panicked".into()))?
            .map_err(|e| DictationError::AudioCaptureError(format!("System audio: {e}")))
    }
}

fn write_i16(w: &mut impl Write, samples: &[f32]) -> std::io::Result<()> {
    for s in samples {
        w.write_all(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes())?;
    }
    Ok(())
}

/// Consume native chunks until the sender is dropped; returns the finished track.
pub(crate) fn spool_worker(
    rx: mpsc::Receiver<Vec<f32>>,
    native: NativeFormat,
    started: Instant,
    dir: &Path,
) -> Result<SystemTrack, String> {
    let spool = tempfile::Builder::new()
        .prefix(SPOOL_PREFIX)
        .suffix(SPOOL_SUFFIX)
        .tempfile_in(dir)
        .map_err(|e| format!("cannot create spool file in {}: {e}", dir.display()))?;
    let mut out = BufWriter::new(spool.reopen().map_err(|e| e.to_string())?);
    let mut converter = StreamingConverter::new(native.sample_rate, native.channels)?;
    let mut delivered: u64 = 0;
    let mut peak = 0f32;

    let mut emit = |samples: &[f32], out: &mut BufWriter<File>, delivered: &mut u64| -> Result<(), String> {
        peak = samples.iter().fold(peak, |m, s| m.max(s.abs()));
        write_i16(out, samples).map_err(|e| format!("spool write failed: {e}"))?;
        *delivered += samples.len() as u64;
        Ok(())
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(chunk) => {
                let mono = converter.push(&chunk)?;
                emit(&mono, &mut out, &mut delivered)?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let gap = silence_to_insert(started.elapsed().as_millis() as u64, delivered, GAP_TOLERANCE_MS);
        if gap > 0 {
            emit(&vec![0.0; gap as usize], &mut out, &mut delivered)?;
        }
    }
    let tail = converter.finish()?;
    emit(&tail, &mut out, &mut delivered)?;
    let gap = silence_to_insert(started.elapsed().as_millis() as u64, delivered, GAP_TOLERANCE_MS);
    if gap > 0 {
        emit(&vec![0.0; gap as usize], &mut out, &mut delivered)?;
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);

    let samples = read_spool(&spool)?;
    // `spool` (NamedTempFile) deletes the file on drop.
    Ok(SystemTrack { samples, native, silent: peak < 1e-4 })
}

fn read_spool(spool: &NamedTempFile) -> Result<Vec<f32>, String> {
    let mut f = spool.reopen().map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes(*b) as f32 / i16::MAX as f32)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_converts_spools_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let native = NativeFormat { sample_rate: 48_000, channels: 2 };
        for _ in 0..50 {
            tx.send(vec![0.5f32; 2 * 960]).unwrap(); // 20 ms stereo each, 1 s total
        }
        drop(tx);
        let started = Instant::now();
        let track = spool_worker(rx, native, started, dir.path()).unwrap();
        assert!((track.samples.len() as i64 - 16_000).abs() < 800, "len {}", track.samples.len());
        assert!(!track.silent);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "spool must be deleted");
    }

    #[test]
    fn gap_is_filled_after_the_audio_not_before() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        // Capture "started" 2 s ago but only 0.5 s of audio ever arrived.
        let started = Instant::now() - Duration::from_secs(2);
        tx.send(vec![0.5f32; 8_000]).unwrap(); // 0.5 s mono at 16 kHz
        drop(tx);
        let t = spool_worker(rx, NativeFormat { sample_rate: 16_000, channels: 1 }, started, dir.path()).unwrap();
        assert!((t.samples.len() as i64 - 32_000).abs() < 1_600, "len {}", t.samples.len());
        assert!(t.samples[1_000] > 0.4, "audio must come first");
        assert!(t.samples[t.samples.len() - 1_000].abs() < 1e-3, "silence must trail the audio");
    }

    #[test]
    fn silent_input_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(vec![0.0f32; 2 * 4800]).unwrap();
        drop(tx);
        let t = spool_worker(rx, NativeFormat { sample_rate: 48_000, channels: 2 }, Instant::now(), dir.path()).unwrap();
        assert!(t.silent);
    }

    #[test]
    fn stale_spools_only() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join(format!("{SPOOL_PREFIX}a{SPOOL_SUFFIX}"));
        fs::write(&fresh, b"x").unwrap();
        let other = dir.path().join("keep.wav");
        fs::write(&other, b"x").unwrap();
        assert_eq!(clean_stale_spools(dir.path()), 0);
        assert!(fresh.exists() && other.exists());
    }

    /// Live: records 3 s of system audio into the spool. Owner-approved runs only:
    ///   cargo test -p sagascript-core --features record recorder_live -- --ignored --nocapture
    #[test]
    #[ignore = "live system-audio capture; requires owner approval"]
    fn recorder_live_three_seconds() {
        let r = SystemRecorder::start(&CaptureTarget::All).unwrap();
        thread::sleep(Duration::from_secs(3));
        let track = r.stop().unwrap();
        assert!((track.samples.len() as i64 - 48_000).abs() < 8_000);
    }
}
