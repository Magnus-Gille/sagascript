//! Long-audio pipeline: one PCM file, planned windows, bounded in-flight
//! windows, strictly ordered merge, progress and cancellation.

use super::client::{batch_lane, EngineHostClient, WindowOutput, WindowRequest, WindowTicket};
use super::{EngineHostError, Result};
use crate::sync::lock_or_recover;
use crate::transcription::chunk_merge::{
    merge_window, offset_tokens, plan_windows, tokens_to_text, tokens_to_words, AlignedToken,
    Window, Word,
};
use sagascript_engine_protocol::Priority;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Cooperative cancellation flag shared between the caller and a running job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct JobOptions {
    pub priority: Priority,
    pub boost: Option<std::sync::Arc<super::client::BoostSpec>>,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            priority: Priority::Batch,
            boost: None,
        }
    }
}

/// Per-window timing report.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowTiming {
    pub index: usize,
    pub start_sample: usize,
    pub end_sample: usize,
    pub tokens: usize,
    pub preprocess_ms: u64,
    pub encode_ms: u64,
    pub decode_ms: u64,
    /// Context-biasing trie build time the host reported for this window (0: cached or unused).
    pub boost_us: u64,
    /// Client-observed submit-to-result time.
    pub round_trip_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    pub words: Vec<Word>,
    /// Merged tokens on the global timeline (text with a leading space per word).
    pub tokens: Vec<AlignedToken>,
    pub windows: Vec<WindowTiming>,
    pub audio_s: f64,
}

/// Name of a PCM temp dir: this prefix, then exactly [`PCM_DIR_RAND_LEN`]
/// random ASCII letters and digits. The sweep matches nothing else.
const PCM_DIR_PREFIX: &str = "sagascript-pcm-";
const PCM_DIR_RAND_LEN: usize = 12;
/// A sweep renames a dead owner's dir to this prefix (same random part)
/// before removing it, so an interrupted removal is finished by a later sweep.
const PCM_DEAD_PREFIX: &str = "sagascript-pcm-dead-";
const PCM_AUDIO_FILE: &str = "audio.f32le";
/// Locked by the owning process for the dir's whole lifetime.
const PCM_OWNER_FILE: &str = "owner.lock";
/// First bytes of the owner file, written after the lock is taken.
const PCM_OWNER_MARKER: &str = "sagascript-pcm-owner";

/// Raw f32le PCM in a private (0700) temp dir, removed on drop.
///
/// The dir holds `owner.lock`, exclusively locked (`flock` / `LockFileEx`)
/// for as long as this value lives. The OS drops the lock when the process
/// ends, however it ends, so a dir whose lock can be taken has no live owner:
/// [`sweep_stale_pcm_dirs`] removes such dirs left behind by a killed process.
/// Where the file system cannot lock (some network, FUSE or container temp
/// dirs) the dir is used unlocked and without the owner marker: transcription
/// still works, and the sweep, unable to prove the owner dead, never removes
/// it (it is still removed on drop).
pub struct PcmFile {
    // Field order is drop order: the lock is released before the dir is
    // removed (a sweep that slips in between removes the same dir).
    _owner: std::fs::File,
    dir: tempfile::TempDir,
    path: PathBuf,
}

impl PcmFile {
    pub fn write(samples: &[f32], parent: Option<&Path>) -> Result<Self> {
        Self::write_with_lock(samples, parent, fs2::FileExt::lock_exclusive)
    }

    /// [`Self::write`] with the owner lock injectable (tests simulate a file
    /// system that cannot lock).
    fn write_with_lock(
        samples: &[f32],
        parent: Option<&Path>,
        lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    ) -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix(PCM_DIR_PREFIX).rand_bytes(PCM_DIR_RAND_LEN);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = match parent {
            Some(p) => builder.tempdir_in(p)?,
            None => builder.tempdir()?,
        };
        let owner = lock_pcm_owner(dir.path(), lock)?;
        let path = dir.path().join(PCM_AUDIO_FILE);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = std::io::BufWriter::with_capacity(1 << 20, opts.open(&path)?);
        for chunk in samples.chunks(16 * 1024) {
            let mut bytes = Vec::with_capacity(chunk.len() * 4);
            for s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            f.write_all(&bytes)?;
        }
        f.flush()?;
        Ok(Self {
            _owner: owner,
            dir,
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// Create `owner.lock` in a new PCM dir, lock it, then write the marker. A
/// sweep that takes the lock but finds no marker yet (the owner is between
/// creating and locking the file) leaves the dir alone.
///
/// If the lock cannot be taken (a file system without locking; the file is
/// new, so nobody else holds it) the marker is not written: the dir is then
/// one the sweep cannot prove abandoned, and transcription goes on.
fn lock_pcm_owner(
    dir: &Path,
    lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(dir.join(PCM_OWNER_FILE))?;
    if let Err(e) = lock(&file) {
        warn_pcm_lock_unavailable(&e);
        return Ok(file);
    }
    writeln!(file, "{PCM_OWNER_MARKER} pid={}", std::process::id())?;
    file.flush()?;
    Ok(file)
}

/// Once per process: PCM temp dirs cannot be locked here.
fn warn_pcm_lock_unavailable(error: &std::io::Error) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            error_kind = ?error.kind(),
            %error,
            "cannot lock PCM temp dirs (file system without locking?); using them unlocked, \
             so ones left behind by a killed process are not swept"
        );
    }
}

/// True when a failed `try_lock` means another handle holds the lock, as
/// opposed to a file system that cannot lock at all.
fn owner_lock_is_held(error: &std::io::Error) -> bool {
    error.raw_os_error().is_some()
        && error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

/// Outcome of [`sweep_stale_pcm_dirs`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PcmSweep {
    /// Dirs of dead owners that were removed.
    pub removed: usize,
    /// Dirs whose owner is alive (its lock is held).
    pub kept: usize,
}

fn is_pcm_rand(rand: &str) -> bool {
    rand.len() == PCM_DIR_RAND_LEN && rand.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `Some(true)` when the dir's owner is provably gone: its owner file is a
/// regular file carrying the marker and its lock can be taken. `Some(false)`
/// while the lock is held. `None` when it cannot tell (no owner file, as in
/// dirs from before #366, an owner still setting up, or a file system that
/// cannot lock, whose dirs carry no marker).
fn pcm_owner_is_dead(dir: &Path) -> Option<bool> {
    let owner = dir.join(PCM_OWNER_FILE);
    if !std::fs::symlink_metadata(&owner).is_ok_and(|m| m.is_file()) {
        return None;
    }
    let file = std::fs::File::open(&owner).ok()?;
    if let Err(e) = fs2::FileExt::try_lock_exclusive(&file) {
        return owner_lock_is_held(&e).then_some(false);
    }
    let mut marker = String::new();
    (&file)
        .take(PCM_OWNER_MARKER.len() as u64)
        .read_to_string(&mut marker)
        .ok()?;
    // Dropping `file` releases the lock; nothing can own the dir again.
    (marker == PCM_OWNER_MARKER).then_some(true)
}

/// Remove `dir` (already renamed to the dead prefix). Never follows symlinks.
fn remove_dead_pcm_dir(dir: &Path) -> bool {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            tracing::warn!(error_kind = ?e.kind(), "could not remove a stale PCM temp dir");
            false
        }
    }
}

/// Remove PCM temp dirs in `parent` (not below it) that belong to no live
/// process, typically audio left behind by an app or CLI that was killed.
///
/// Only real directories (never symlinks) named exactly like a PCM dir are
/// considered, and only those whose owner lock can be taken, so a dir of a
/// running app or CLI (or of this process) is never touched. Logs counts only.
pub fn sweep_stale_pcm_dirs(parent: &Path) -> std::io::Result<PcmSweep> {
    let mut sweep = PcmSweep::default();
    for entry in std::fs::read_dir(parent)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = entry.path();
        // `symlink_metadata` does not follow a symlink, which is then skipped.
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        if let Some(rand) = name.strip_prefix(PCM_DEAD_PREFIX) {
            // An earlier sweep proved the owner dead but did not finish.
            if is_pcm_rand(rand) && remove_dead_pcm_dir(&path) {
                sweep.removed += 1;
            }
            continue;
        }
        let Some(rand) = name.strip_prefix(PCM_DIR_PREFIX).filter(|r| is_pcm_rand(r)) else {
            continue;
        };
        match pcm_owner_is_dead(&path) {
            Some(false) => sweep.kept += 1,
            None => {}
            Some(true) => {
                // Rename first (atomic, never follows a symlink): a concurrent
                // sweep then skips it, and an interrupted removal is retried.
                let dead = parent.join(format!("{PCM_DEAD_PREFIX}{rand}"));
                match std::fs::rename(&path, &dead) {
                    Ok(()) => {
                        if remove_dead_pcm_dir(&dead) {
                            sweep.removed += 1;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        tracing::warn!(error_kind = ?e.kind(), "could not remove a stale PCM temp dir")
                    }
                }
            }
        }
    }
    if sweep.removed > 0 {
        tracing::info!(
            removed = sweep.removed,
            kept = sweep.kept,
            "removed PCM temp dirs left behind by processes that ended without cleanup"
        );
    }
    Ok(sweep)
}

/// Run [`sweep_stale_pcm_dirs`] for `parent` on a background thread, at most
/// once per process and directory.
pub fn sweep_stale_pcm_dirs_in_background(parent: PathBuf) {
    // Consistent after any panic: the critical section is one `Vec` lookup and push.
    static SWEPT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    {
        let mut swept = lock_or_recover(&SWEPT, "engine_host.pcm_sweep");
        if swept.contains(&parent) {
            return;
        }
        swept.push(parent.clone());
    }
    let spawned = std::thread::Builder::new()
        .name("engine-host-pcm-sweep".into())
        .spawn(move || {
            if let Err(e) = sweep_stale_pcm_dirs(&parent) {
                tracing::warn!(error_kind = ?e.kind(), "could not sweep stale PCM temp dirs");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the PCM temp dir sweep");
    }
}

/// Startup sweep of the system temp dir, where PCM dirs live unless a client
/// sets [`super::EngineHostConfig::temp_dir`]. Background, once per process.
pub fn sweep_stale_pcm_dirs_at_startup() {
    sweep_stale_pcm_dirs_in_background(std::env::temp_dir());
}

/// Progress callback: `(merged_windows, total_windows)`.
pub type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize);

impl EngineHostClient {
    /// Dictation: one window with `interactive` priority when the utterance
    /// fits `max_window_s`, otherwise the windowed path (still interactive).
    pub fn transcribe_dictation(
        &self,
        samples: &[f32],
        cancel: &CancelToken,
    ) -> Result<Transcription> {
        self.transcribe_samples(
            samples,
            JobOptions {
                priority: Priority::Interactive,
                boost: None,
            },
            cancel,
            None,
        )
    }

    /// File job: batch priority, windows merged in order.
    pub fn transcribe_file_samples(
        &self,
        samples: &[f32],
        cancel: &CancelToken,
        progress: Option<ProgressFn<'_>>,
    ) -> Result<Transcription> {
        self.transcribe_samples(samples, JobOptions::default(), cancel, progress)
    }

    /// Transcribe 16 kHz mono f32 samples through the host.
    pub fn transcribe_samples(
        &self,
        samples: &[f32],
        opts: JobOptions,
        cancel: &CancelToken,
        mut progress: Option<ProgressFn<'_>>,
    ) -> Result<Transcription> {
        if cancel.is_cancelled() {
            return Err(EngineHostError::Cancelled);
        }
        if samples.is_empty() {
            return Ok(Transcription {
                text: String::new(),
                words: vec![],
                tokens: vec![],
                windows: vec![],
                audio_s: 0.0,
            });
        }
        let cfg = self.config();
        let caps = self.warm_with_cancel(cancel)?;
        let sr = caps.sample_rate;
        let max_window = (caps.max_window_s * f64::from(sr)) as usize;
        let plan: Vec<Window> = if samples.len() <= max_window {
            vec![Window {
                start_sample: 0,
                end_sample: samples.len(),
            }]
        } else {
            plan_windows(
                samples.len(),
                sr,
                caps.preferred_window_s.min(caps.max_window_s),
                caps.preferred_overlap_s,
            )
        };
        let total = plan.len();
        // Batch work leaves one host slot free for dictation (see `batch_lane`).
        let lane = match opts.priority {
            Priority::Batch => batch_lane(caps.max_in_flight),
            Priority::Interactive => caps.max_in_flight.max(1) as usize,
        };
        let max_in_flight = lane.min(total);
        let pcm = PcmFile::write(samples, cfg.temp_dir.as_deref())?;
        let max_retries = cfg.max_crash_retries;

        let req = |i: usize| WindowRequest {
            pcm_path: pcm.path().to_path_buf(),
            offset_samples: plan[i].start_sample as u64,
            num_samples: (plan[i].end_sample - plan[i].start_sample) as u64,
            sample_rate: sr,
            priority: opts.priority,
            boost: opts.boost.clone().or_else(|| self.boost()),
        };

        let mut queue: VecDeque<(usize, WindowTicket)> = VecDeque::new();
        let mut retries = vec![0u32; total];
        let mut next = 0usize;
        let mut merged = 0usize;
        let mut acc: Vec<AlignedToken> = Vec::new();
        let mut timings: Vec<WindowTiming> = Vec::with_capacity(total);

        while merged < total {
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled); // queued tickets cancel on drop
            }
            let mut failure: Option<EngineHostError> = None;
            let mut failed_idx: Option<usize> = None;
            while queue.len() < max_in_flight && next < total {
                let wait = if queue.is_empty() {
                    Duration::from_millis(50)
                } else {
                    Duration::ZERO
                };
                match self.submit_window(&req(next), cancel, wait) {
                    Ok(Some(t)) => {
                        queue.push_back((next, t));
                        next += 1;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            if failure.is_none() {
                let Some((idx, ticket)) = queue.pop_front() else {
                    continue; // no slot yet (an interactive request is running)
                };
                match ticket.wait(cancel) {
                    Ok(out) => {
                        debug_assert_eq!(idx, merged);
                        acc = merge_output(acc, &plan[idx], idx, sr, &caps, out, &mut timings);
                        merged += 1;
                        if let Some(p) = progress.as_mut() {
                            p(merged, total);
                        }
                        continue;
                    }
                    Err(e) => {
                        failed_idx = Some(idx);
                        failure = Some(e);
                    }
                }
            }
            let err = failure.expect("failure set on this path");
            if !err.is_crash() {
                return Err(err); // queued tickets cancel on drop
            }
            // The host died: every window in flight is affected. Resubmit them
            // in order, each at most `max_crash_retries` times.
            let mut affected: Vec<usize> = failed_idx.into_iter().collect();
            affected.extend(queue.iter().map(|(i, _)| *i));
            if failed_idx.is_none() {
                affected.push(next);
            }
            for &i in &affected {
                retries[i] += 1;
                if retries[i] > max_retries {
                    return Err(err);
                }
            }
            tracing::warn!(windows = ?affected, "engine host crashed; resubmitting in-flight windows");
            next = affected[0];
            queue.clear();
        }
        Ok(finish(acc, timings, samples.len(), sr))
    }
}

fn merge_output(
    acc: Vec<AlignedToken>,
    window: &Window,
    index: usize,
    sr: u32,
    caps: &sagascript_engine_protocol::Capabilities,
    out: WindowOutput,
    timings: &mut Vec<WindowTiming>,
) -> Vec<AlignedToken> {
    let mut tokens: Vec<AlignedToken> = out
        .tokens
        .into_iter()
        .map(|t| {
            let mut a = AlignedToken::new(t.id, t.text.replace('▁', " "), t.start, t.duration);
            a.confidence = t.confidence;
            a
        })
        .collect();
    offset_tokens(&mut tokens, window.start_sample as f64 / f64::from(sr));
    timings.push(WindowTiming {
        index,
        start_sample: window.start_sample,
        end_sample: window.end_sample,
        tokens: tokens.len(),
        preprocess_ms: out.timings.preprocess_ms,
        encode_ms: out.timings.encode_ms,
        decode_ms: out.timings.decode_ms,
        boost_us: out.timings.boost_us,
        round_trip_ms: out.round_trip_ms,
    });
    if tokens.is_empty() {
        acc
    } else if acc.is_empty() {
        tokens
    } else {
        merge_window(&acc, &tokens, caps.preferred_overlap_s)
    }
}

fn finish(
    tokens: Vec<AlignedToken>,
    windows: Vec<WindowTiming>,
    n_samples: usize,
    sr: u32,
) -> Transcription {
    Transcription {
        text: tokens_to_text(&tokens),
        words: tokens_to_words(&tokens),
        tokens,
        windows,
        audio_s: n_samples as f64 / f64::from(sr),
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;

    fn pcm_dir(parent: &Path, rand: &str) -> PathBuf {
        let dir = parent.join(format!("{PCM_DIR_PREFIX}{rand}"));
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join(PCM_AUDIO_FILE), b"pcm").unwrap();
        dir
    }

    /// Dirs the sweep cannot prove dead stay; an interrupted removal is finished.
    #[test]
    fn sweep_keeps_unprovable_dirs_and_finishes_interrupted_removals() {
        let parent = tempfile::tempdir().unwrap();
        // Owner between creating and locking its owner file: no marker yet.
        let setting_up = pcm_dir(parent.path(), "SettingUp123");
        std::fs::write(setting_up.join(PCM_OWNER_FILE), b"").unwrap();
        // Name not in the exact format (11 random characters).
        let short = pcm_dir(parent.path(), "Short123456");
        std::fs::write(short.join(PCM_OWNER_FILE), PCM_OWNER_MARKER).unwrap();
        // Dead owner (marker, nobody holds the lock), and an earlier sweep's leftover.
        let dead = pcm_dir(parent.path(), "DeadOwner123");
        std::fs::write(dead.join(PCM_OWNER_FILE), PCM_OWNER_MARKER).unwrap();
        let leftover = parent.path().join(format!("{PCM_DEAD_PREFIX}Leftover1234"));
        std::fs::create_dir(&leftover).unwrap();
        std::fs::write(leftover.join(PCM_AUDIO_FILE), b"pcm").unwrap();
        // A live dir of this process.
        let live = PcmFile::write(&[0.0; 16], Some(parent.path())).unwrap();

        let sweep = sweep_stale_pcm_dirs(parent.path()).unwrap();
        assert_eq!(
            sweep,
            PcmSweep {
                removed: 2,
                kept: 1
            }
        );
        assert!(!dead.exists() && !leftover.exists());
        assert!(setting_up.join(PCM_AUDIO_FILE).is_file());
        assert!(short.join(PCM_AUDIO_FILE).is_file());
        assert!(live.path().is_file());
        let names: Vec<_> = std::fs::read_dir(parent.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            !names.iter().any(|n| n.starts_with(PCM_DEAD_PREFIX)),
            "{names:?}"
        );
    }

    /// A file system that cannot lock must not fail transcription: the dir is
    /// used without the owner marker, so a sweep never removes it (nobody
    /// holds its lock, yet its owner cannot be proven dead).
    #[test]
    fn pcm_file_without_file_locking_still_works_and_is_never_swept() {
        let parent = tempfile::tempdir().unwrap();
        let file = PcmFile::write_with_lock(&[0.5; 16], Some(parent.path()), |_| {
            Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
        })
        .unwrap();
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 16 * 4);
        let owner = std::fs::read(file.dir().join(PCM_OWNER_FILE)).unwrap();
        assert!(owner.is_empty(), "no owner marker without a lock");

        assert_eq!(
            sweep_stale_pcm_dirs(parent.path()).unwrap(),
            PcmSweep::default()
        );
        assert!(file.path().is_file());
        let dir = file.dir().to_path_buf();
        drop(file);
        assert!(!dir.exists(), "still removed on drop");
    }

    #[test]
    fn only_a_contended_lock_counts_as_a_live_owner() {
        assert!(owner_lock_is_held(&fs2::lock_contended_error()));
        assert!(!owner_lock_is_held(&std::io::Error::from(
            std::io::ErrorKind::Unsupported
        )));
        assert!(!owner_lock_is_held(&std::io::Error::other("no locks here")));
    }
}
