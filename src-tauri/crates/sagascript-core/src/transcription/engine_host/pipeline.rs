//! Long-audio pipeline: one PCM file, planned windows, bounded in-flight
//! windows, strictly ordered merge, progress and cancellation.

use super::client::{batch_lane, EngineHostClient, WindowOutput, WindowRequest, WindowTicket};
use super::{EngineHostError, Result};
use crate::audio::decoder::MAX_DECODE_SAMPLES;
use crate::sync::lock_or_recover;
use crate::transcription::chunk_merge::{
    count_planned_windows, merge_window, offset_tokens, plan_for_total, tokens_to_text,
    tokens_to_words, AlignedToken, Window, WindowPlanner, Word,
};
use sagascript_engine_protocol::Priority;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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
/// PCM dirs from before #366: [`PCM_DIR_PREFIX`] and `tempfile`'s default 6
/// random letters and digits, without an owner file. Nothing can prove their
/// owner dead, so the sweep removes them only once they are clearly abandoned.
const LEGACY_PCM_RAND_LEN: usize = 6;
/// A PCM dir whose owner cannot be proven dead (one from before #366, or one
/// on a file system that cannot lock) is abandoned once neither it nor
/// anything directly in it changed for this long: a dictation or file job
/// keeps its PCM dir for seconds to minutes.
const PCM_ABANDONED_AGE: Duration = Duration::from_secs(24 * 60 * 60);
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
/// still works, and the sweep, unable to prove the owner dead, removes the dir
/// only once it was left untouched for a day (it is still removed on drop).
///
/// The file can grow: [`Self::create`] makes it empty and [`Self::append`]
/// adds samples at the end, with every append written through to the OS before
/// it returns. A reader (the engine host opens the file per request and reads
/// just the requested range) can therefore rely on a range whose append has
/// returned. [`Self::write`] is `create` plus one `append`.
pub struct PcmFile {
    // Field order is drop order: the lock is released before the dir is
    // removed (a sweep that slips in between removes the same dir), and the
    // audio file is closed before it (Windows cannot remove an open file).
    _owner: std::fs::File,
    tail: Mutex<PcmTail>,
    dir: tempfile::TempDir,
    path: PathBuf,
}

/// The write end of a [`PcmFile`].
struct PcmTail {
    file: std::fs::File,
    /// Samples written so far.
    samples: u64,
    /// Reused little-endian encoding buffer.
    bytes: Vec<u8>,
}

/// Samples encoded and written per `write` call (1 MiB).
const PCM_WRITE_CHUNK: usize = 256 * 1024;

impl PcmFile {
    /// A PCM file holding `samples`.
    pub fn write(samples: &[f32], parent: Option<&Path>) -> Result<Self> {
        Self::write_with_lock(samples, parent, fs2::FileExt::lock_exclusive)
    }

    /// An empty PCM file in a new private dir, for [`Self::append`].
    pub fn create(parent: Option<&Path>) -> Result<Self> {
        Self::create_with_lock(parent, fs2::FileExt::lock_exclusive)
    }

    /// [`Self::write`] with the owner lock injectable (tests simulate a file
    /// system that cannot lock).
    fn write_with_lock(
        samples: &[f32],
        parent: Option<&Path>,
        lock: impl FnOnce(&std::fs::File) -> std::io::Result<()>,
    ) -> Result<Self> {
        let file = Self::create_with_lock(parent, lock)?;
        file.append(samples)?;
        Ok(file)
    }

    /// Append `samples` at the end of the file and write them through to the OS
    /// (no user-space buffering), so they are readable by another process once
    /// this returns.
    pub fn append(&self, samples: &[f32]) -> Result<()> {
        Ok(self.append_io(samples)?)
    }

    fn append_io(&self, samples: &[f32]) -> std::io::Result<()> {
        let mut tail = lock_or_recover(&self.tail, "engine_host.pcm_tail");
        let PcmTail {
            file,
            samples: written,
            bytes,
        } = &mut *tail;
        for chunk in samples.chunks(PCM_WRITE_CHUNK) {
            bytes.clear();
            bytes.reserve(chunk.len() * 4);
            for s in chunk {
                bytes.extend_from_slice(&s.to_le_bytes());
            }
            file.write_all(bytes)?;
            *written += chunk.len() as u64;
        }
        Ok(())
    }

    /// Samples appended so far.
    pub fn len_samples(&self) -> u64 {
        lock_or_recover(&self.tail, "engine_host.pcm_tail").samples
    }

    fn create_with_lock(
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
        let file = opts.open(&path)?;
        Ok(Self {
            _owner: owner,
            tail: Mutex::new(PcmTail {
                file,
                samples: 0,
                bytes: Vec::new(),
            }),
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
/// new, so nobody else holds it) the marker is not written: the sweep then
/// cannot prove the owner dead and goes by age instead, and transcription
/// goes on.
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
             so ones left behind by a killed process are swept only after a day"
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
    /// Dirs that were removed: those of dead owners, and dirs whose owner
    /// cannot be proven dead (from before #366, or on a file system that
    /// cannot lock) that were left untouched for a day.
    pub removed: usize,
    /// Dirs whose owner is alive (its lock is held).
    pub kept: usize,
}

fn is_pcm_rand(rand: &str) -> bool {
    rand.len() == PCM_DIR_RAND_LEN && rand.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn is_legacy_pcm_rand(rand: &str) -> bool {
    rand.len() == LEGACY_PCM_RAND_LEN && rand.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Neither `dir` nor anything directly in it was modified within
/// [`PCM_ABANDONED_AGE`] before `now`. Any doubt (an unreadable entry, a
/// timestamp in the future) counts as recent.
fn pcm_dir_untouched(dir: &Path, now: SystemTime) -> bool {
    let old = |path: &Path| {
        std::fs::symlink_metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|modified| {
                now.duration_since(modified)
                    .is_ok_and(|age| age > PCM_ABANDONED_AGE)
            })
    };
    old(dir)
        && std::fs::read_dir(dir).is_ok_and(|mut entries| {
            entries.all(|entry| entry.is_ok_and(|entry| old(&entry.path())))
        })
}

/// A dir from before #366 (named like one, see [`LEGACY_PCM_RAND_LEN`]) that
/// is abandoned: it has no owner file and is [`pcm_dir_untouched`].
fn legacy_pcm_dir_is_abandoned(dir: &Path, now: SystemTime) -> bool {
    std::fs::symlink_metadata(dir.join(PCM_OWNER_FILE)).is_err() && pcm_dir_untouched(dir, now)
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

/// Rename a PCM dir to the dead prefix, then remove it. The rename is atomic
/// and never follows a symlink: a concurrent sweep then skips the dir, and a
/// later one finishes an interrupted removal.
fn retire_pcm_dir(parent: &Path, dir: &Path, rand: &str) -> bool {
    let dead = parent.join(format!("{PCM_DEAD_PREFIX}{rand}"));
    match std::fs::rename(dir, &dead) {
        Ok(()) => remove_dead_pcm_dir(&dead),
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
/// running app or CLI (or of this process) is never touched. Dirs whose owner
/// cannot be proven dead (from before #366, without an owner lock, or on a
/// file system that cannot lock, without the owner marker) are removed once
/// untouched for a day. Logs counts only.
pub fn sweep_stale_pcm_dirs(parent: &Path) -> std::io::Result<PcmSweep> {
    sweep_stale_pcm_dirs_at(parent, SystemTime::now())
}

/// [`sweep_stale_pcm_dirs`] with the clock injectable (tests age legacy dirs).
fn sweep_stale_pcm_dirs_at(parent: &Path, now: SystemTime) -> std::io::Result<PcmSweep> {
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
            // An earlier sweep proved the dir stale but did not finish.
            if (is_pcm_rand(rand) || is_legacy_pcm_rand(rand)) && remove_dead_pcm_dir(&path) {
                sweep.removed += 1;
            }
            continue;
        }
        let Some(rand) = name.strip_prefix(PCM_DIR_PREFIX) else {
            continue;
        };
        if is_legacy_pcm_rand(rand) {
            if legacy_pcm_dir_is_abandoned(&path, now) && retire_pcm_dir(parent, &path, rand) {
                sweep.removed += 1;
            }
            continue;
        }
        if !is_pcm_rand(rand) {
            continue;
        }
        match pcm_owner_is_dead(&path) {
            Some(false) => sweep.kept += 1,
            Some(true) if retire_pcm_dir(parent, &path, rand) => sweep.removed += 1,
            // Owner unknown (no marker: a file system that cannot lock, or an
            // owner killed while setting up): go by age, as for legacy dirs.
            None if pcm_dir_untouched(&path, now) && retire_pcm_dir(parent, &path, rand) => {
                sweep.removed += 1
            }
            Some(true) | None => {}
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
pub(crate) type ProgressFn<'a> = &'a mut dyn FnMut(usize, usize);

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
        progress: Option<ProgressFn<'_>>,
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
        let plan = plan_for_total(
            samples.len(),
            sr,
            caps.max_window_s,
            caps.preferred_window_s,
            caps.preferred_overlap_s,
        );
        let total = plan.len();
        // Batch work leaves one host slot free for dictation (see `batch_lane`).
        let lane = match opts.priority {
            Priority::Batch => batch_lane(caps.max_in_flight),
            Priority::Interactive => caps.max_in_flight.max(1) as usize,
        };
        let max_in_flight = lane.min(total);
        let pcm = PcmFile::write(samples, cfg.temp_dir.as_deref())?;
        let mut source = FixedPlan { windows: plan };
        let done = self.drive_windows(
            &mut source,
            &DriveCtx {
                caps: &caps,
                opts: &opts,
                cancel,
                pcm_path: pcm.path(),
                max_in_flight,
            },
            progress,
        )?;
        Ok(finish(done.tokens, done.timings, samples.len(), sr))
    }

    /// Transcribe 16 kHz mono audio that is still being produced.
    ///
    /// `produce` runs on its own thread and appends the audio to the [`Sink`] it
    /// is given (a decoder, say). The calling thread runs the same scheduling
    /// loop as [`Self::transcribe_samples`] (bounded in-flight windows, ordered
    /// merge, crash retry, cancellation), planning windows incrementally with
    /// [`WindowPlanner`] and submitting each as soon as its audio has been
    /// written, so the engine works on the first seconds while the rest is still
    /// being decoded. The result is the same as for the finished audio: the same
    /// windows, the same merge. The host is started and the model loaded while
    /// the producer already runs.
    ///
    /// Progress is `(merged, estimated_total)`: the total is an estimate until
    /// the audio ends (see [`Sink::set_expected_total`]; without one no progress
    /// is reported), and is never reached before the last window is merged.
    ///
    /// Errors: [`StreamError::Engine`] for a failure of the engine side or of
    /// the PCM file, [`StreamError::Producer`] for the error `produce` returned.
    /// `Engine(Cancelled)` whenever `cancel` fired. Otherwise the error of the
    /// side that failed first wins; the other side is stopped (its next
    /// [`Sink::append`] fails) and always joined before this returns.
    pub fn transcribe_file_stream<E: Send>(
        &self,
        cancel: &CancelToken,
        progress: Option<ProgressFn<'_>>,
        produce: impl FnOnce(&Sink<'_>) -> std::result::Result<(), E> + Send,
    ) -> std::result::Result<Transcription, StreamError<E>> {
        if cancel.is_cancelled() {
            return Err(StreamError::Engine(EngineHostError::Cancelled));
        }
        let opts = JobOptions::default();
        let pcm =
            PcmFile::create(self.config().temp_dir.as_deref()).map_err(StreamError::Engine)?;
        let shared = StreamShared::new(cancel.clone());
        let job = shared.job.clone();
        let over = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let (pcm_ref, shared_ref) = (&pcm, &shared);
            // The caller's cancel reaches the job token within a few ms; the job
            // ends this watcher, however the scope is left.
            let _ends_watcher = SetOnDrop(&over);
            scope.spawn(|| {
                while !over.load(Ordering::SeqCst) {
                    if cancel.is_cancelled() {
                        job.cancel();
                        return;
                    }
                    std::thread::sleep(CANCEL_POLL);
                }
            });
            let producer = scope.spawn(move || {
                // A panic in `produce` must not leave the scheduler waiting.
                let mut guard = ProducerGuard {
                    shared: shared_ref,
                    finished: false,
                };
                let sink = Sink::new(pcm_ref, shared_ref);
                let result = produce(&sink);
                match &result {
                    Ok(()) => {
                        if let Err(e) = sink.finish() {
                            shared_ref.fail_with(Side::Engine, Some(sink_error_to_engine(e)));
                        }
                    }
                    Err(_) => shared_ref.fail_with(Side::Producer, None),
                }
                guard.finished = true;
                result
            });
            let engine = self.stream_engine_side(&shared, &pcm, &opts, &job, progress);
            if engine.is_err() {
                shared.fail_with(Side::Engine, None);
            }
            // Whatever happened, the producer stops and is joined.
            shared.stop();
            let produced = match producer.join() {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            };
            let io_error = shared.lock().io_error.take();
            resolve_stream(engine, produced, shared.first_failure(), io_error, cancel)
        })
    }

    /// The engine side of [`Self::transcribe_file_stream`], on the calling thread.
    fn stream_engine_side(
        &self,
        shared: &StreamShared,
        pcm: &PcmFile,
        opts: &JobOptions,
        cancel: &CancelToken,
        progress: Option<ProgressFn<'_>>,
    ) -> Result<(Driven, usize, u32)> {
        // The model loads while the producer already decodes.
        let caps = self.warm_with_cancel(cancel)?;
        let sr = caps.sample_rate;
        if sr != STREAM_SAMPLE_RATE {
            return Err(EngineHostError::InvalidInput(format!(
                "streamed audio is {STREAM_SAMPLE_RATE} Hz but the engine takes {sr} Hz"
            )));
        }
        let lane = match opts.priority {
            Priority::Batch => batch_lane(caps.max_in_flight),
            Priority::Interactive => caps.max_in_flight.max(1) as usize,
        };
        let mut source = StreamPlan::new(shared, &caps);
        // Start only when every lane has a window, or the audio is all there.
        source.prime(lane, cancel)?;
        let done = self.drive_windows(
            &mut source,
            &DriveCtx {
                caps: &caps,
                opts,
                cancel,
                pcm_path: pcm.path(),
                max_in_flight: lane,
            },
            progress,
        )?;
        Ok((done, source.samples(), sr))
    }

    /// The scheduling loop shared by the finished-audio and streaming entry
    /// points: keep up to `max_in_flight` windows of `source` in flight, merge
    /// their outputs strictly in order, report progress, observe `cancel`, and
    /// resubmit the in-flight windows when the host crashes (each at most
    /// `max_crash_retries` times).
    fn drive_windows(
        &self,
        source: &mut dyn WindowSource,
        ctx: &DriveCtx<'_>,
        mut progress: Option<ProgressFn<'_>>,
    ) -> Result<Driven> {
        let DriveCtx {
            caps,
            opts,
            cancel,
            pcm_path,
            max_in_flight,
        } = *ctx;
        let sr = caps.sample_rate;
        let max_retries = self.config().max_crash_retries;
        let req = |window: Window| WindowRequest {
            pcm_path: pcm_path.to_path_buf(),
            offset_samples: window.start_sample as u64,
            num_samples: (window.end_sample - window.start_sample) as u64,
            sample_rate: sr,
            priority: opts.priority,
            boost: opts.boost.clone().or_else(|| self.boost()),
        };

        let mut queue: VecDeque<(usize, WindowTicket)> = VecDeque::new();
        let mut retries: Vec<u32> = Vec::new();
        let mut next = 0usize;
        let mut merged = 0usize;
        let mut acc: Vec<AlignedToken> = Vec::new();
        let mut timings: Vec<WindowTiming> = Vec::with_capacity(source.windows().len());

        loop {
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled); // queued tickets cancel on drop
            }
            source.check_alive()?;
            if source.is_complete() && merged >= source.windows().len() {
                break;
            }
            let mut failure: Option<EngineHostError> = None;
            while queue.len() < max_in_flight {
                if next >= source.windows().len() {
                    source.plan_more();
                    if next >= source.windows().len() {
                        break;
                    }
                }
                retries.resize(source.windows().len(), 0);
                let wait = if queue.is_empty() {
                    Duration::from_millis(50)
                } else {
                    Duration::ZERO
                };
                match self.submit_window(&req(source.windows()[next]), cancel, wait) {
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
            // A submit failure, or else the oldest window's answer: merged
            // (next turn) or failed (handled below, with its index).
            let (err, failed_idx) = match failure {
                Some(e) => (e, None),
                None => {
                    let Some((idx, ticket)) = queue.pop_front() else {
                        // Nothing in flight. Either no slot yet (an interactive
                        // request is running), or the audio has not reached the
                        // next window: wait for it, bounded so cancel is noticed.
                        if next >= source.windows().len() && !source.is_complete() {
                            source.wait_for_data(cancel)?;
                        }
                        continue;
                    };
                    match ticket.wait(cancel) {
                        Ok(out) => {
                            debug_assert_eq!(idx, merged);
                            let window = source.windows()[idx];
                            acc = merge_output(acc, &window, idx, sr, caps, out, &mut timings);
                            merged += 1;
                            if let Some(p) = progress.as_mut() {
                                if let Some(total) = source.progress_total(merged) {
                                    p(merged, total);
                                }
                            }
                            continue;
                        }
                        Err(e) => (e, Some(idx)),
                    }
                }
            };
            // A dictation window that timed out had its host killed
            // (`WindowTicket::wait`), so it is recovered like a crash. A file
            // job's timeout leaves the host alone and fails the job as before.
            let stalled = opts.priority == Priority::Interactive && err.is_window_timeout();
            if !err.is_crash() && !stalled {
                return Err(err); // queued tickets cancel on drop
            }
            // The host died: every window in flight is affected. Resubmit them
            // in order, each at most `max_crash_retries` times. The next
            // `submit_window` restarts the host and reloads the model (under
            // the `load` timeout, not the window timeout).
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
            tracing::warn!(windows = ?affected, stalled, "engine host crashed or stalled; resubmitting in-flight windows");
            next = affected[0];
            queue.clear();
        }
        Ok(Driven {
            tokens: acc,
            timings,
        })
    }
}

/// What the scheduling loop runs against.
#[derive(Clone, Copy)]
struct DriveCtx<'a> {
    caps: &'a sagascript_engine_protocol::Capabilities,
    opts: &'a JobOptions,
    cancel: &'a CancelToken,
    pcm_path: &'a Path,
    /// Windows in flight at most.
    max_in_flight: usize,
}

/// What the scheduling loop produced.
struct Driven {
    tokens: Vec<AlignedToken>,
    timings: Vec<WindowTiming>,
}

/// Where the scheduling loop gets its windows: a plan that is complete from the
/// start, or one that grows while audio arrives.
trait WindowSource {
    /// The windows planned so far, each with all its samples already readable.
    fn windows(&self) -> &[Window];
    /// Plan the windows the audio so far determines (never blocks).
    fn plan_more(&mut self);
    /// No window will ever be added.
    fn is_complete(&self) -> bool;
    /// Fails when the source can no longer deliver (its producer failed, or
    /// stopped delivering); `Ok` for a source that cannot fail.
    fn check_alive(&self) -> Result<()> {
        Ok(())
    }
    /// Block until more audio may have arrived, for a bounded time (so the
    /// caller can notice cancellation). Only called while incomplete.
    fn wait_for_data(&mut self, _cancel: &CancelToken) -> Result<()> {
        unreachable!("a complete plan never waits for data")
    }
    /// `(merged, total)` to report after `merged` windows: `None` when there is
    /// no estimate. Unless the plan is complete, `total > merged`.
    fn progress_total(&self, merged: usize) -> Option<usize>;

    /// Wait until `want` windows are planned or the plan is complete.
    fn prime(&mut self, want: usize, cancel: &CancelToken) -> Result<()> {
        loop {
            self.plan_more();
            if self.windows().len() >= want || self.is_complete() {
                return Ok(());
            }
            if cancel.is_cancelled() {
                return Err(EngineHostError::Cancelled);
            }
            self.check_alive()?;
            self.wait_for_data(cancel)?;
        }
    }
}

/// The plan of audio that is all there: [`WindowSource`] over a finished plan.
struct FixedPlan {
    windows: Vec<Window>,
}

impl WindowSource for FixedPlan {
    fn windows(&self) -> &[Window] {
        &self.windows
    }
    fn plan_more(&mut self) {}
    fn is_complete(&self) -> bool {
        true
    }
    fn progress_total(&self, _merged: usize) -> Option<usize> {
        Some(self.windows.len())
    }
}

// ---- streaming input -------------------------------------------------------------

/// Streamed audio is 16 kHz mono f32, what every decoder here produces.
const STREAM_SAMPLE_RATE: u32 = 16_000;
/// Audio reaches the scheduler in batches of at least this many samples (1 s),
/// not per resampler chunk: fewer file writes and wake-ups.
const PUBLISH_MIN_SAMPLES: usize = STREAM_SAMPLE_RATE as usize;
/// The longest the scheduler sleeps waiting for audio before it checks for
/// cancellation again.
const DATA_WAIT: Duration = Duration::from_millis(50);

/// Why [`EngineHostClient::transcribe_file_stream`] failed.
#[derive(Debug)]
pub enum StreamError<E> {
    /// The engine side failed (host, model, windows, the PCM file), or the job
    /// was cancelled (`EngineHostError::Cancelled`).
    Engine(EngineHostError),
    /// The producer returned this error.
    Producer(E),
}

impl<E: std::fmt::Display> std::fmt::Display for StreamError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(e) => e.fmt(f),
            Self::Producer(e) => e.fmt(f),
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for StreamError<E> {}

/// Why [`Sink::append`] failed.
#[derive(Debug)]
pub enum SinkError {
    /// The transcription no longer takes audio (it was cancelled, or failed):
    /// the producer should stop and return.
    Closed,
    /// The audio could not be written to the PCM file.
    Io(std::io::Error),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("the transcription stopped taking audio"),
            Self::Io(e) => write!(f, "cannot write the audio for the engine: {e}"),
        }
    }
}

impl std::error::Error for SinkError {}

/// Which side of a streamed job failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Engine = 1,
    Producer = 2,
}

/// State shared by the producer thread and the scheduler.
struct StreamShared {
    state: Mutex<StreamState>,
    wake: std::sync::Condvar,
    /// The caller's token: cancelling it cancels the job.
    cancel: CancelToken,
    /// The job's own token, which every wait on the host observes (loading the
    /// model, submitting a window, waiting for its result). It is cancelled when
    /// the caller cancels (a watcher bridges the two) and when the producer
    /// fails, so neither can leave the scheduler in a long host wait. The
    /// caller's token is never touched: it is the caller's.
    job: CancelToken,
    /// Set when either side failed or the job is over: the producer must stop.
    stop: AtomicBool,
    /// The side that failed first (`Side as u8`), 0 while none did.
    first_failure: std::sync::atomic::AtomicU8,
    /// Advisory total length in samples, for progress; 0 = unknown.
    expected_total: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct StreamState {
    /// Samples written to the PCM file and flushed to the OS.
    available: usize,
    /// The producer returned `Ok`: `available` is the whole length.
    finished: bool,
    /// An engine-side failure that happened on the producer thread.
    io_error: Option<EngineHostError>,
}

impl StreamShared {
    fn new(cancel: CancelToken) -> Self {
        Self {
            state: Mutex::new(StreamState::default()),
            wake: std::sync::Condvar::new(),
            cancel,
            job: CancelToken::new(),
            stop: AtomicBool::new(false),
            first_failure: std::sync::atomic::AtomicU8::new(0),
            expected_total: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StreamState> {
        lock_or_recover(&self.state, "engine_host.stream_state")
    }

    /// Make `samples` more samples (already written) visible to the scheduler.
    fn publish(&self, samples: usize, finished: bool) {
        let mut state = self.lock();
        state.available += samples;
        state.finished |= finished;
        drop(state);
        self.wake.notify_all();
    }

    fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.cancel.is_cancelled()
    }

    /// The job is over: the producer's next append fails.
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        drop(self.lock());
        self.wake.notify_all();
    }

    /// Record a failure of `side` (the first one is remembered) and stop the other.
    fn fail_with(&self, side: Side, error: Option<EngineHostError>) {
        let _ =
            self.first_failure
                .compare_exchange(0, side as u8, Ordering::SeqCst, Ordering::SeqCst);
        if let Some(error) = error {
            self.lock().io_error.get_or_insert(error);
        }
        // Whatever the scheduler is waiting for on the host is no longer wanted.
        self.job.cancel();
        self.stop();
    }

    fn first_failure(&self) -> Option<Side> {
        match self.first_failure.load(Ordering::SeqCst) {
            1 => Some(Side::Engine),
            2 => Some(Side::Producer),
            _ => None,
        }
    }
}

/// How often the watcher passes the caller's cancel on to the job token.
const CANCEL_POLL: Duration = Duration::from_millis(20);

/// Sets a flag when dropped, so a watcher thread ends however its scope exits.
struct SetOnDrop<'a>(&'a AtomicBool);

impl Drop for SetOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Marks the producer as failed if its thread unwinds, so the scheduler is not
/// left waiting for audio that will never come.
struct ProducerGuard<'a> {
    shared: &'a StreamShared,
    finished: bool,
}

impl Drop for ProducerGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.shared.fail_with(Side::Producer, None);
        }
    }
}

/// Where a producer puts the audio of [`EngineHostClient::transcribe_file_stream`]:
/// 16 kHz mono samples, in order.
///
/// Samples are buffered and written to the PCM file in batches of at least one
/// second of audio; a batch becomes visible to the scheduler (and so to the
/// engine) only after its bytes were written and flushed to the OS, which is
/// what lets the host read a window from the growing file.
pub struct Sink<'a> {
    pcm: &'a PcmFile,
    shared: &'a StreamShared,
    pending: Mutex<Vec<f32>>,
}

impl<'a> Sink<'a> {
    fn new(pcm: &'a PcmFile, shared: &'a StreamShared) -> Self {
        Self {
            pcm,
            shared,
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Add `samples` to the audio. Fails with [`SinkError::Closed`] once the
    /// transcription was cancelled or failed (stop producing), or with
    /// [`SinkError::Io`] if the PCM file cannot be written.
    pub fn append(&self, samples: &[f32]) -> std::result::Result<(), SinkError> {
        if self.shared.is_stopped() {
            return Err(SinkError::Closed);
        }
        let mut pending = lock_or_recover(&self.pending, "engine_host.stream_pending");
        pending.extend_from_slice(samples);
        if pending.len() >= PUBLISH_MIN_SAMPLES {
            self.publish(&mut pending, false)?;
        }
        Ok(())
    }

    /// Tell the transcription how long the audio is expected to be, in samples,
    /// to estimate progress. Advisory: it may be revised (call again) and need
    /// not be exact; zero means unknown. The value usually comes from a file
    /// header, which cannot be trusted, so it is capped at the longest audio the
    /// decoder accepts (about 4 hours).
    pub fn set_expected_total(&self, samples: usize) {
        self.shared
            .expected_total
            .store(samples.min(MAX_DECODE_SAMPLES), Ordering::Relaxed);
    }

    /// Write the buffered samples through to the PCM file, then let the
    /// scheduler see them (and, with `finished`, that there are no more).
    fn publish(
        &self,
        pending: &mut Vec<f32>,
        finished: bool,
    ) -> std::result::Result<(), SinkError> {
        if !pending.is_empty() {
            self.pcm.append_io(pending).map_err(SinkError::Io)?;
        }
        self.shared.publish(pending.len(), finished);
        pending.clear();
        Ok(())
    }

    /// The producer is done: flush what is buffered and mark the end of the audio.
    fn finish(&self) -> std::result::Result<(), SinkError> {
        if self.shared.is_stopped() {
            return Err(SinkError::Closed);
        }
        let mut pending = lock_or_recover(&self.pending, "engine_host.stream_pending");
        self.publish(&mut pending, true)
    }
}

/// An engine-side error for a sink failure that happened after the producer returned.
fn sink_error_to_engine(error: SinkError) -> EngineHostError {
    match error {
        SinkError::Io(e) => EngineHostError::Io(e),
        SinkError::Closed => EngineHostError::Cancelled,
    }
}

/// [`WindowSource`] over audio that is still arriving: windows are planned
/// incrementally and only once all their samples are in the PCM file.
struct StreamPlan<'a> {
    shared: &'a StreamShared,
    planner: WindowPlanner,
    windows: Vec<Window>,
    /// `(available, finished)` the planner was last fed.
    seen: (usize, bool),
    /// The audio ended without a single sample.
    empty: bool,
    sample_rate: u32,
    max_window_s: f64,
    preferred_window_s: f64,
    overlap_s: f64,
}

impl<'a> StreamPlan<'a> {
    fn new(shared: &'a StreamShared, caps: &sagascript_engine_protocol::Capabilities) -> Self {
        Self {
            shared,
            planner: WindowPlanner::new(
                caps.sample_rate,
                caps.max_window_s,
                caps.preferred_window_s,
                caps.preferred_overlap_s,
            ),
            windows: Vec::new(),
            seen: (0, false),
            empty: false,
            sample_rate: caps.sample_rate,
            max_window_s: caps.max_window_s,
            preferred_window_s: caps.preferred_window_s,
            overlap_s: caps.preferred_overlap_s,
        }
    }

    /// Total samples, once the audio ended.
    fn samples(&self) -> usize {
        self.seen.0
    }
}

impl WindowSource for StreamPlan<'_> {
    fn windows(&self) -> &[Window] {
        &self.windows
    }

    fn plan_more(&mut self) {
        let (available, finished) = {
            let state = self.shared.lock();
            (state.available, state.finished)
        };
        self.seen = (available, finished);
        // No audio at all has no window (like an empty slice in `transcribe_samples`).
        self.empty = finished && available == 0;
        if self.empty {
            return;
        }
        while let Some(window) = self.planner.next_window(available, finished) {
            self.windows.push(window);
        }
    }

    fn is_complete(&self) -> bool {
        self.empty || self.planner.is_done()
    }

    fn check_alive(&self) -> Result<()> {
        if let Some(error) = self.shared.lock().io_error.take() {
            return Err(error);
        }
        if self.shared.first_failure() == Some(Side::Producer) {
            // The producer's own error is reported by the caller; this only
            // unwinds the scheduler.
            return Err(EngineHostError::Cancelled);
        }
        Ok(())
    }

    fn wait_for_data(&mut self, _cancel: &CancelToken) -> Result<()> {
        let state = self.shared.lock();
        let unchanged = (state.available, state.finished) == self.seen
            && state.io_error.is_none()
            && !self.shared.is_stopped();
        if unchanged {
            // Woken by a publish or a failure; the timeout keeps cancel responsive.
            let _ = self
                .shared
                .wake
                .wait_timeout(state, DATA_WAIT)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Ok(())
    }

    fn progress_total(&self, merged: usize) -> Option<usize> {
        if self.is_complete() {
            return Some(self.windows.len());
        }
        let hint = self.shared.expected_total.load(Ordering::Relaxed);
        if hint == 0 {
            return None;
        }
        let samples = hint.max(self.seen.0);
        let estimate = count_planned_windows(
            samples,
            self.sample_rate,
            self.max_window_s,
            self.preferred_window_s,
            self.overlap_s,
        );
        // An estimate is never the end: more than what is merged and planned.
        Some(estimate.max(self.windows.len()).max(merged + 1))
    }
}

/// Combine the two sides' outcomes of a streamed job. Cancellation wins;
/// otherwise the error of whichever side failed first.
fn resolve_stream<E>(
    engine: Result<(Driven, usize, u32)>,
    produced: std::result::Result<(), E>,
    first_failure: Option<Side>,
    producer_side_io_error: Option<EngineHostError>,
    cancel: &CancelToken,
) -> std::result::Result<Transcription, StreamError<E>> {
    match engine {
        Ok((done, samples, sr)) => match produced {
            Ok(()) => Ok(finish(done.tokens, done.timings, samples, sr)),
            Err(e) => Err(StreamError::Producer(e)),
        },
        Err(_) if cancel.is_cancelled() => Err(StreamError::Engine(EngineHostError::Cancelled)),
        Err(error) => match (first_failure, produced) {
            (Some(Side::Producer), Err(e)) => Err(StreamError::Producer(e)),
            // The scheduler may only have unwound; then the failure is the PCM file's.
            _ => Err(StreamError::Engine(producer_side_io_error.unwrap_or(error))),
        },
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
    /// used without the owner marker, so a sweep keeps it while it is recent
    /// (nobody holds its lock, yet its owner cannot be proven dead).
    #[test]
    fn pcm_file_without_file_locking_still_works_and_is_kept_by_the_sweep() {
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

    /// Dirs from before #366 (6 random characters, no owner file) are removed
    /// once they were left untouched for a day; recent ones, ones with an owner
    /// file, near-miss names and symlinks stay.
    #[test]
    fn sweep_removes_abandoned_legacy_dirs_only() {
        let parent = tempfile::tempdir().unwrap();
        let legacy = pcm_dir(parent.path(), "AbC123");
        let with_owner = pcm_dir(parent.path(), "XyZ789");
        std::fs::write(with_owner.join(PCM_OWNER_FILE), PCM_OWNER_MARKER).unwrap();
        let near_miss = pcm_dir(parent.path(), "AbC12_");
        // An interrupted removal of a legacy dir.
        let leftover = parent.path().join(format!("{PCM_DEAD_PREFIX}Old123"));
        std::fs::create_dir(&leftover).unwrap();
        std::fs::write(leftover.join(PCM_AUDIO_FILE), b"pcm").unwrap();
        #[cfg(unix)]
        let outside = {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join(PCM_AUDIO_FILE), b"pcm").unwrap();
            std::os::unix::fs::symlink(
                outside.path(),
                parent.path().join(format!("{PCM_DIR_PREFIX}Link12")),
            )
            .unwrap();
            outside
        };

        // Recent: possibly still in use by a running older version.
        let now = SystemTime::now();
        let sweep = sweep_stale_pcm_dirs_at(parent.path(), now).unwrap();
        assert_eq!(
            sweep,
            PcmSweep {
                removed: 1,
                kept: 0
            }
        );
        assert!(!leftover.exists());
        assert!(legacy.join(PCM_AUDIO_FILE).is_file());

        // A day later it is abandoned.
        let later = now + PCM_ABANDONED_AGE + Duration::from_secs(60);
        let sweep = sweep_stale_pcm_dirs_at(parent.path(), later).unwrap();
        assert_eq!(
            sweep,
            PcmSweep {
                removed: 1,
                kept: 0
            }
        );
        assert!(!legacy.exists());
        assert!(with_owner.join(PCM_AUDIO_FILE).is_file());
        assert!(near_miss.join(PCM_AUDIO_FILE).is_file());
        #[cfg(unix)]
        assert!(
            outside.path().join(PCM_AUDIO_FILE).is_file(),
            "symlink followed"
        );
        let names: Vec<_> = std::fs::read_dir(parent.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            !names.iter().any(|n| n.starts_with(PCM_DEAD_PREFIX)),
            "{names:?}"
        );
    }

    /// Dirs in the current format whose owner cannot be proven dead (no
    /// marker: a file system that cannot lock, or an owner killed while
    /// setting up) do not leak: they are removed once untouched for a day.
    #[test]
    fn sweep_removes_unprovable_dirs_only_after_a_day() {
        let parent = tempfile::tempdir().unwrap();
        let unlocked = pcm_dir(parent.path(), "NoLock123456");
        std::fs::write(unlocked.join(PCM_OWNER_FILE), b"").unwrap();
        let no_owner_file = pcm_dir(parent.path(), "NoOwner12345");
        // A live owner (this process) is kept however old its dir looks.
        let live = PcmFile::write(&[0.0; 16], Some(parent.path())).unwrap();

        let now = SystemTime::now();
        let sweep = sweep_stale_pcm_dirs_at(parent.path(), now).unwrap();
        assert_eq!(
            sweep,
            PcmSweep {
                removed: 0,
                kept: 1
            }
        );
        assert!(unlocked.join(PCM_AUDIO_FILE).is_file());
        assert!(no_owner_file.join(PCM_AUDIO_FILE).is_file());

        let later = now + PCM_ABANDONED_AGE + Duration::from_secs(60);
        let sweep = sweep_stale_pcm_dirs_at(parent.path(), later).unwrap();
        assert_eq!(
            sweep,
            PcmSweep {
                removed: 2,
                kept: 1
            }
        );
        assert!(!unlocked.exists() && !no_owner_file.exists());
        assert!(live.path().is_file());
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

#[cfg(test)]
mod stream_tests {
    use super::*;

    fn le_bytes(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    /// An empty PCM file that grows by appends is the same private dir, owner
    /// lock and 0600 file as a written one, and holds exactly the appended bytes.
    #[test]
    fn a_pcm_file_is_created_empty_and_grows_by_append() {
        let parent = tempfile::tempdir().unwrap();
        let file = PcmFile::create(Some(parent.path())).unwrap();
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
        assert_eq!(file.len_samples(), 0);
        // The owner lock is held while the file lives. Windows' LockFileEx makes
        // the locked range unreadable to every other handle, so only Unix (an
        // advisory flock) can read the marker back while the lock is held.
        assert_eq!(pcm_owner_is_dead(file.dir()), Some(false));
        #[cfg(unix)]
        {
            let owner = std::fs::read(file.dir().join(PCM_OWNER_FILE)).unwrap();
            assert!(owner.starts_with(PCM_OWNER_MARKER.as_bytes()));
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(file.path()), 0o600);
            assert_eq!(mode(file.dir()), 0o700);
        }

        let first = [0.5f32, -1.25, 3.0];
        let second: Vec<f32> = (0..PCM_WRITE_CHUNK + 7).map(|i| i as f32).collect();
        file.append(&first).unwrap();
        assert_eq!(
            std::fs::read(file.path()).unwrap(),
            le_bytes(&first),
            "readable as soon as append returns"
        );
        file.append(&second).unwrap();
        file.append(&[]).unwrap();
        let mut want = le_bytes(&first);
        want.extend(le_bytes(&second));
        assert_eq!(std::fs::read(file.path()).unwrap(), want);
        assert_eq!(file.len_samples(), (first.len() + second.len()) as u64);

        // `write` is `create` plus one append.
        let written = PcmFile::write(&second, Some(parent.path())).unwrap();
        assert_eq!(std::fs::read(written.path()).unwrap(), le_bytes(&second));
        let dir = file.dir().to_path_buf();
        drop(file);
        assert!(!dir.exists(), "removed on drop");
    }

    fn stream_fixture() -> (tempfile::TempDir, PcmFile, StreamShared) {
        let parent = tempfile::tempdir().unwrap();
        let file = PcmFile::create(Some(parent.path())).unwrap();
        (parent, file, StreamShared::new(CancelToken::new()))
    }

    #[test]
    fn the_sink_publishes_in_batches_of_at_least_one_second_after_the_write() {
        let (_parent, file, shared) = stream_fixture();
        let sink = Sink::new(&file, &shared);
        let available = || shared.lock().available;
        sink.append(&vec![0.25; 100]).unwrap();
        sink.append(&vec![0.5; 15_899]).unwrap();
        assert_eq!(available(), 0, "just under a second: still buffered");
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 0);
        sink.append(&[0.75]).unwrap();
        assert_eq!(available(), 16_000);
        assert_eq!(
            std::fs::metadata(file.path()).unwrap().len(),
            16_000 * 4,
            "the bytes are in the file when they are published"
        );
        sink.append(&[1.0; 5]).unwrap();
        assert_eq!(available(), 16_000);
        assert!(!shared.lock().finished);
        sink.finish().unwrap();
        assert_eq!(available(), 16_005);
        assert!(shared.lock().finished);
        assert_eq!(std::fs::metadata(file.path()).unwrap().len(), 16_005 * 4);
    }

    #[test]
    fn a_published_range_is_always_readable() {
        // The invariant the host relies on: whatever the scheduler sees as
        // available has already been written.
        let (_parent, file, shared) = stream_fixture();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let sink = Sink::new(&file, &shared);
                for _ in 0..300 {
                    sink.append(&vec![0.5; 7_000]).unwrap();
                }
                sink.finish().unwrap();
                done.store(true, Ordering::SeqCst);
            });
            let mut checks = 0;
            while !done.load(Ordering::SeqCst) {
                let available = shared.lock().available;
                let len = std::fs::metadata(file.path()).unwrap().len();
                assert!(
                    len >= available as u64 * 4,
                    "{len} bytes for {available} samples"
                );
                checks += 1;
            }
            assert!(checks > 0);
        });
        assert_eq!(shared.lock().available, 300 * 7_000);
    }

    fn pianissimo_like_caps() -> sagascript_engine_protocol::Capabilities {
        sagascript_engine_protocol::Capabilities {
            sample_rate: 16_000,
            max_window_s: 15.0,
            preferred_window_s: 15.0,
            preferred_overlap_s: 2.0,
            max_in_flight: 4,
            token_timestamps: true,
            languages: vec!["sv".into()],
            compute_units: vec!["ane".into()],
            min_macos: None,
            context_biasing: false,
        }
    }

    /// The length hint comes from a file header and cannot be trusted: it is
    /// capped where it is set, and the estimate built from it is arithmetic, so
    /// a hostile value costs nothing; the estimate stays above what is merged.
    #[test]
    fn an_untrusted_length_hint_is_capped_and_the_estimate_never_reaches_the_end() {
        let (_parent, file, shared) = stream_fixture();
        let sink = Sink::new(&file, &shared);
        let plan = StreamPlan::new(&shared, &pianissimo_like_caps());
        let hint = || shared.expected_total.load(Ordering::Relaxed);

        assert_eq!(plan.progress_total(0), None, "no hint, no estimate");
        sink.set_expected_total(0);
        assert_eq!(plan.progress_total(0), None, "zero is unknown");

        sink.set_expected_total(usize::MAX);
        assert_eq!(hint(), MAX_DECODE_SAMPLES);
        let cap_windows = count_planned_windows(MAX_DECODE_SAMPLES, 16_000, 15.0, 15.0, 2.0);
        assert_eq!(cap_windows, 1_108, "4 hours in 13 s steps");
        let started = std::time::Instant::now();
        assert_eq!(plan.progress_total(3), Some(cap_windows));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "an estimate is arithmetic"
        );

        // 40 s: windows start at 0, 13 and 26 s. The estimate stays above what is
        // merged, so progress is never 100% before the end.
        sink.set_expected_total(40 * 16_000);
        assert_eq!(plan.progress_total(0), Some(3));
        assert_eq!(plan.progress_total(2), Some(3));
        assert_eq!(plan.progress_total(3), Some(4));
        assert_eq!(plan.progress_total(10), Some(11));
    }

    #[test]
    fn a_stopped_or_cancelled_job_closes_the_sink() {
        let (_parent, file, shared) = stream_fixture();
        let sink = Sink::new(&file, &shared);
        sink.append(&[0.0; 10]).unwrap();
        shared.stop();
        assert!(matches!(sink.append(&[0.0]), Err(SinkError::Closed)));
        assert!(matches!(sink.finish(), Err(SinkError::Closed)));

        let (_parent, file, shared) = stream_fixture();
        let sink = Sink::new(&file, &shared);
        shared.cancel.cancel();
        assert!(matches!(sink.append(&[0.0]), Err(SinkError::Closed)));
    }

    #[test]
    fn the_first_failure_is_remembered_and_stops_the_other_side() {
        let shared = StreamShared::new(CancelToken::new());
        assert_eq!(shared.first_failure(), None);
        assert!(!shared.is_stopped());
        shared.fail_with(Side::Producer, None);
        shared.fail_with(Side::Engine, Some(EngineHostError::Cancelled));
        assert_eq!(shared.first_failure(), Some(Side::Producer));
        assert!(shared.is_stopped());
    }

    #[test]
    fn errors_resolve_to_cancel_first_then_the_side_that_failed_first() {
        let engine_failed = || Err(EngineHostError::NotRunning);
        let cancel = CancelToken::new();
        let r = resolve_stream::<&str>(
            engine_failed(),
            Err("decode"),
            Some(Side::Engine),
            None,
            &cancel,
        );
        assert!(matches!(
            r,
            Err(StreamError::Engine(EngineHostError::NotRunning))
        ));
        let r = resolve_stream::<&str>(
            engine_failed(),
            Err("decode"),
            Some(Side::Producer),
            None,
            &cancel,
        );
        assert!(matches!(r, Err(StreamError::Producer("decode"))));
        // The scheduler's own failure while the producer finished fine.
        let r = resolve_stream::<&str>(engine_failed(), Ok(()), Some(Side::Engine), None, &cancel);
        assert!(matches!(
            r,
            Err(StreamError::Engine(EngineHostError::NotRunning))
        ));
        // The scheduler only unwound because the producer side hit a PCM file error.
        let r = resolve_stream::<&str>(
            Err(EngineHostError::Cancelled),
            Ok(()),
            Some(Side::Engine),
            Some(EngineHostError::Io(std::io::Error::other("disk full"))),
            &cancel,
        );
        assert!(matches!(
            r,
            Err(StreamError::Engine(EngineHostError::Io(_)))
        ));
        cancel.cancel();
        for first in [None, Some(Side::Engine), Some(Side::Producer)] {
            let r = resolve_stream::<&str>(engine_failed(), Err("decode"), first, None, &cancel);
            assert!(
                matches!(r, Err(StreamError::Engine(EngineHostError::Cancelled))),
                "{first:?}"
            );
        }
    }
}
