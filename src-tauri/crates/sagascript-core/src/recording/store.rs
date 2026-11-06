//! Creating and writing a recording folder (ADR 0010, design §4.5).
//!
//! # Write order
//!
//! Every fsync is `fsutil::sync_durably` (`F_FULLFSYNC` on macOS).
//!
//! 1. `create_dir` with the **final** name (exclusive: it fails if the name
//!    exists, so nothing is ever replaced), mode `0700` on Unix; fsync the
//!    parent; create `recording.lock` and take an exclusive advisory lock.
//!    There is no `.part` rename: `fsutil::publish_new` hard-links a *file*
//!    and cannot publish a directory, an exclusive directory rename differs on
//!    every platform, and on Windows renaming a folder fails while a scanner
//!    holds a handle inside it.
//! 2. Write `recording.json` with `state: "recording"` (a temporary file,
//!    fsync, rename, so a reader never sees half a manifest); create the track
//!    files (placeholder header) and journals and fsync them, then the folder.
//! 3. While recording the writer appends frames, journals the start, every
//!    anchor and every gap, and fsyncs tracks and journals every
//!    [`SYNC_INTERVAL`](super::SYNC_INTERVAL) (via [`RecordingWriter::sync_if_due`])
//!    and at each gap.
//! 4. At the end of a track: last frame, fsync, STREAMINFO and SEEKTABLE
//!    rewritten in place, fsync.
//! 5. `recording.json.tmp` with `state: "complete"`, fsync, rename over
//!    `recording.json` (replacing our own file is the intent), fsync the
//!    folder, remove the lock.
//!
//! A recording without a manifest is not a recording: a crash between steps 1
//! and 2 leaves an empty folder that nothing reads. A manifest saying
//! `recording` whose lock is free belongs to a dead process and is finished by
//! [`recover`](super::recover).

use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

use fs2::FileExt;

use super::encoder::{TrackEncoder, TrackSummary};
use super::error::RecordingError;
use super::flac::FlacEncoder;
use super::io::{
    create_dir_private, create_new_private, open_read_write, replace_file_durably, sync_dir,
    sync_file,
};
use super::journal::{journal_file_name, JournalEvent, JournalWriter};
use super::manifest::{
    is_valid_track_id, serialize_manifest, Client, Clock, EndedReason, Gap, GapReason, Manifest,
    Quality, RecordingState, Track, MANIFEST_FILE, MIN_READER_VERSION, SCHEMA_VERSION,
};
use super::naming;
use super::recover::{recover_track, TailPolicy};
use super::SYNC_INTERVAL;

/// The advisory lock file of a recording in progress.
pub const LOCK_FILE: &str = "recording.lock";

/// How many ids are tried for a free folder name.
const MAX_NAME_ATTEMPTS: usize = 8;
/// The longest title stored (UTF-8 bytes).
const MAX_STORED_TITLE_BYTES: usize = 1000;
/// The in-progress manifest is padded with trailing spaces (JSON ignores them)
/// to this size, so that when the disk is full the final manifest can still be
/// written *in place* over the same blocks, which needs no new space (design
/// §4.3). A manifest that has grown past it (many hours of anchors) falls
/// back to recovery.
pub const MANIFEST_RESERVED_BYTES: usize = 32 * 1024;

/// A factory for the encoder of a track: the seam by which tests put a file
/// that fails like a full disk under the real writer.
pub(crate) type EncoderFactory<'a> =
    dyn FnMut(&Path, &NewTrack) -> Result<Box<dyn TrackEncoder>, RecordingError> + 'a;

pub(crate) fn flac_encoder_for(
    path: &Path,
    track: &NewTrack,
) -> Result<Box<dyn TrackEncoder>, RecordingError> {
    let mut encoder = FlacEncoder::create(path, track.sample_rate, track.channels, track.bits)?;
    encoder.sync()?;
    Ok(Box::new(encoder))
}

/// A track to record.
#[derive(Clone, Debug)]
pub struct NewTrack {
    /// The track id, the role (`mic`); also the stem of its files.
    pub id: String,
    pub role: String,
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: u8,
    /// 24 for the lossless tier, 16 for the compact tier.
    pub bits: u8,
    /// The host clock the timestamps are in (`mach_continuous`, `qpc`, ...).
    pub clock_domain: String,
    /// Host time of the track's first frame.
    pub start_ns: u64,
    /// The native rate a compact track is resampled from.
    pub resampled_from: Option<u32>,
    /// The input device; a local detail.
    pub device: Option<String>,
}

/// A recording to start.
#[derive(Clone, Debug)]
pub struct NewRecording {
    /// Free text; the folder name is a sanitised form of it.
    pub title: String,
    /// RFC 3339 with the local offset (`2026-11-05T14:02:11.482+01:00`); its
    /// local date and time open the folder name.
    pub started_at: String,
    pub client: Client,
    pub quality: Quality,
    pub tracks: Vec<NewTrack>,
}

/// A folder of recordings.
#[derive(Clone, Debug)]
pub struct RecordingStore {
    root: PathBuf,
}

impl RecordingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder that holds the recordings.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Start a recording: steps 1 and 2 of the write order. The folder name is
    /// `<date> <time> <title> [<id>]`; if it exists a new id is tried.
    pub fn create(&self, request: NewRecording) -> Result<RecordingWriter, RecordingError> {
        self.create_with_ids(request, &mut || uuid::Uuid::new_v4().to_string())
    }

    /// [`create`](Self::create) with the source of recording ids given, so a
    /// test can force a name collision.
    pub(crate) fn create_with_ids(
        &self,
        request: NewRecording,
        next_id: &mut dyn FnMut() -> String,
    ) -> Result<RecordingWriter, RecordingError> {
        self.create_with(request, next_id, &mut flac_encoder_for)
    }

    /// [`create_with_ids`](Self::create_with_ids) with the encoders given.
    pub(crate) fn create_with(
        &self,
        request: NewRecording,
        next_id: &mut dyn FnMut() -> String,
        encoders: &mut EncoderFactory<'_>,
    ) -> Result<RecordingWriter, RecordingError> {
        validate_request(&request)?;
        let stamp = folder_stamp(&request.started_at)?;
        fs::create_dir_all(&self.root)
            .map_err(|error| RecordingError::io("create the recordings folder", &error))?;

        let (dir, folder_name, recording_id) = self.claim_folder(&request, &stamp, next_id)?;
        match start_in(&dir, folder_name, recording_id, request, encoders) {
            Ok(writer) => Ok(writer),
            Err(error) => {
                // Nothing recorded yet: remove what this call created.
                remove_fresh_folder(&dir);
                Err(error)
            }
        }
    }

    /// `create_dir` with the final name; a collision gets a new id.
    fn claim_folder(
        &self,
        request: &NewRecording,
        stamp: &str,
        next_id: &mut dyn FnMut() -> String,
    ) -> Result<(PathBuf, String, String), RecordingError> {
        for _ in 0..MAX_NAME_ATTEMPTS {
            let recording_id = next_id();
            let name = naming::folder_name(stamp, &request.title, &recording_id);
            let dir = self.root.join(&name);
            match create_dir_private(&dir) {
                Ok(()) => {
                    sync_dir(&self.root).map_err(|error| {
                        RecordingError::io("sync the recordings folder", &error)
                    })?;
                    return Ok((dir, name, recording_id));
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(RecordingError::io("create the recording folder", &error))
                }
            }
        }
        Err(RecordingError::NameUnavailable)
    }
}

fn validate_request(request: &NewRecording) -> Result<(), RecordingError> {
    if request.title.len() > MAX_STORED_TITLE_BYTES {
        return Err(RecordingError::InvalidInput {
            reason: "the title is too long",
        });
    }
    if request.tracks.is_empty() {
        return Err(RecordingError::InvalidInput {
            reason: "a recording needs a track",
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for track in &request.tracks {
        if !is_valid_track_id(&track.id) || !seen.insert(track.id.as_str()) {
            return Err(RecordingError::InvalidInput {
                reason: "track ids must be distinct plain names",
            });
        }
        if track.role.is_empty() || track.clock_domain.is_empty() {
            return Err(RecordingError::InvalidInput {
                reason: "a track needs a role and a clock domain",
            });
        }
    }
    Ok(())
}

/// `YYYY-MM-DD HH.MM` from an RFC 3339 `started_at`.
fn folder_stamp(started_at: &str) -> Result<String, RecordingError> {
    let bytes = started_at.as_bytes();
    let digit = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let ok = bytes.len() >= 16
        && digit(0..4)
        && bytes[4] == b'-'
        && digit(5..7)
        && bytes[7] == b'-'
        && digit(8..10)
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && digit(11..13)
        && bytes[13] == b':'
        && digit(14..16);
    if !ok {
        return Err(RecordingError::InvalidInput {
            reason: "started_at is not an RFC 3339 timestamp",
        });
    }
    Ok(format!(
        "{} {}.{}",
        &started_at[0..10],
        &started_at[11..13],
        &started_at[14..16]
    ))
}

/// Take the exclusive advisory lock of a recording folder without blocking.
/// `create` makes the lock file when it is absent (recovery of a folder whose
/// lock file was removed by hand); it is never truncated.
pub(crate) fn lock_folder(dir: &Path, create: bool) -> Result<File, RecordingError> {
    let path = dir.join(LOCK_FILE);
    let file = if create {
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path)
    } else {
        fs::OpenOptions::new().read(true).write(true).open(&path)
    }
    .map_err(|error| RecordingError::io("open the recording lock", &error))?;
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(file),
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            Err(RecordingError::Locked)
        }
        Err(error) => Err(RecordingError::io("lock the recording", &error)),
    }
}

/// The in-progress manifest, step 2.
fn initial_manifest(recording_id: String, request: &NewRecording) -> Manifest {
    Manifest {
        schema_version: SCHEMA_VERSION,
        min_reader_version: MIN_READER_VERSION,
        recording_id,
        title: request.title.clone(),
        state: RecordingState::Recording,
        recovered: false,
        started_at: request.started_at.clone(),
        client: request.client.clone(),
        quality: request.quality,
        tracks: request
            .tracks
            .iter()
            .map(|track| Track {
                id: track.id.clone(),
                role: track.role.clone(),
                file: format!("{}.flac", track.id),
                codec: "flac".into(),
                lossless: request.quality == Quality::Lossless,
                sample_rate: track.sample_rate,
                bits: track.bits,
                channels: track.channels,
                frames: 0,
                audio_sha256: None,
                clock: Clock::new(track.clock_domain.clone(), track.start_ns),
                gaps: Vec::new(),
                ended_reason: None,
                clipped_samples: 0,
                resampled_from: track.resampled_from,
                device: track.device.clone(),
                extra: Default::default(),
            })
            .collect(),
        extra: Default::default(),
    }
}

/// Steps 1 (lock) and 2 inside the freshly created folder.
fn start_in(
    dir: &Path,
    folder_name: String,
    recording_id: String,
    request: NewRecording,
    encoders: &mut EncoderFactory<'_>,
) -> Result<RecordingWriter, RecordingError> {
    // The lock file is new, so the lock cannot be contended.
    let lock_path = dir.join(LOCK_FILE);
    let lock = create_new_private(&lock_path)
        .map_err(|error| RecordingError::io("create the recording lock", &error))?;
    FileExt::try_lock_exclusive(&lock).map_err(|error| {
        if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
            RecordingError::Locked
        } else {
            RecordingError::io("lock the recording", &error)
        }
    })?;

    let manifest = initial_manifest(recording_id, &request);
    let mut initial = serialize_manifest(&manifest)?;
    if initial.len() < MANIFEST_RESERVED_BYTES {
        initial.resize(MANIFEST_RESERVED_BYTES, b' ');
    }
    replace_file_durably(&dir.join(MANIFEST_FILE), &initial)
        .map_err(|error| RecordingError::io("write the recording manifest", &error))?;

    let mut tracks = Vec::with_capacity(manifest.tracks.len());
    for (new, entry) in request.tracks.iter().zip(&manifest.tracks) {
        let encoder = encoders(&dir.join(&entry.file), new)?;
        let mut journal = JournalWriter::create(&dir.join(journal_file_name(&entry.id)))?;
        journal.append(&JournalEvent::Start {
            domain: new.clock_domain.clone(),
            host_ns: new.start_ns,
            sample_rate: new.sample_rate,
            channels: new.channels,
        })?;
        journal.sync()?;
        tracks.push(TrackWriter {
            id: entry.id.clone(),
            encoder: Some(encoder),
            journal,
            anchors: vec![(new.start_ns, 0)],
            gaps: Vec::new(),
            finished: None,
        });
    }
    sync_dir(dir).map_err(|error| RecordingError::io("sync the recording folder", &error))?;

    Ok(RecordingWriter {
        dir: dir.to_path_buf(),
        folder_name,
        _lock: lock,
        manifest,
        tracks,
        last_sync: Instant::now(),
    })
}

/// Remove the files `start_in` may have created in a folder this call made
/// itself. Nothing in it holds audio yet, so this deletes nothing a user made.
fn remove_fresh_folder(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
    let _ = fs::remove_dir(dir);
}

struct TrackWriter {
    id: String,
    encoder: Option<Box<dyn TrackEncoder>>,
    journal: JournalWriter,
    anchors: Vec<(u64, u64)>,
    gaps: Vec<Gap>,
    finished: Option<FinishedTrack>,
}

/// The gaps and the clock anchors of a track, as the journal says they are.
type RebuiltTrack = (Vec<Gap>, Vec<(u64, u64)>);

/// What the manifest needs from a track that ended.
struct FinishedTrack {
    frames: u64,
    audio_sha256: String,
    clipped_samples: u64,
    reason: EndedReason,
    /// Set when the track was cut back after a failed write: the gaps and
    /// anchors that survive the cut, from the journal. `None` means the
    /// writer's own lists are complete.
    rebuilt: Option<RebuiltTrack>,
}

/// A recording in progress. It is driven by one thread (`&mut self`) and holds
/// the folder's lock until it is finished or dropped. Dropping it without
/// [`finish`](Self::finish) leaves a recording that
/// [`recover`](super::recover) finishes.
pub struct RecordingWriter {
    dir: PathBuf,
    folder_name: String,
    _lock: File,
    manifest: Manifest,
    tracks: Vec<TrackWriter>,
    last_sync: Instant,
}

impl RecordingWriter {
    /// The recording folder (for "reveal in Finder"; never put it in an error).
    pub fn folder(&self) -> &Path {
        &self.dir
    }

    /// The folder's name.
    pub fn folder_name(&self) -> &str {
        &self.folder_name
    }

    pub fn recording_id(&self) -> &str {
        &self.manifest.recording_id
    }

    /// The manifest as written at the start (`state: recording`).
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn track_mut(&mut self, id: &str) -> Result<&mut TrackWriter, RecordingError> {
        self.tracks
            .iter_mut()
            .find(|track| track.id == id)
            .ok_or(RecordingError::UnknownTrack)
    }

    /// Append interleaved samples to a track.
    pub fn push(&mut self, track: &str, interleaved: &[f32]) -> Result<(), RecordingError> {
        let track = self.track_mut(track)?;
        track
            .encoder
            .as_mut()
            .ok_or(RecordingError::TrackFinished)?
            .push(interleaved)
    }

    /// Record that the next frame of the track (index `frames_pushed`) was
    /// captured at `host_ns`: a clock anchor, journalled at once.
    pub fn anchor(&mut self, track: &str, host_ns: u64) -> Result<(), RecordingError> {
        let frame = self
            .tracks
            .iter()
            .find(|t| t.id == track)
            .and_then(|t| t.encoder.as_ref())
            .ok_or(RecordingError::UnknownTrack)?
            .frames_pushed();
        self.anchor_at(track, host_ns, frame)
    }

    /// Record that stored frame `frame` of the track was captured at
    /// `host_ns`. For a resampled track the frame is not the number pushed so
    /// far (the resampler holds a few frames back), so the caller names it.
    pub fn anchor_at(
        &mut self,
        track: &str,
        host_ns: u64,
        frame: u64,
    ) -> Result<(), RecordingError> {
        let track = self.track_mut(track)?;
        let encoder = track
            .encoder
            .as_ref()
            .ok_or(RecordingError::TrackFinished)?;
        let clipped = encoder.clipped_samples();
        track.journal.append(&JournalEvent::Anchor {
            host_ns,
            frame,
            clipped,
        })?;
        track.anchors.push((host_ns, frame));
        Ok(())
    }

    /// Insert `length` frames of silence into the track at its current end so
    /// the frame index still maps to time, and record the gap. The track is
    /// fsynced before the journal says the gap exists, and the journal after.
    pub fn gap(
        &mut self,
        track: &str,
        length: u64,
        reason: GapReason,
    ) -> Result<(), RecordingError> {
        if length == 0 {
            return Ok(());
        }
        let track = self.track_mut(track)?;
        let encoder = track
            .encoder
            .as_mut()
            .ok_or(RecordingError::TrackFinished)?;
        let at_frame = encoder.frames_pushed();
        encoder.push_silence(length)?;
        encoder.sync()?;
        let clipped = encoder.clipped_samples();
        track.journal.append(&JournalEvent::Gap {
            at_frame,
            length,
            reason: reason.clone(),
            clipped,
        })?;
        track.journal.sync()?;
        track.gaps.push(Gap::new(at_frame, length, reason));
        Ok(())
    }

    /// Fsync every live track and journal.
    pub fn sync(&mut self) -> Result<(), RecordingError> {
        for track in &mut self.tracks {
            if let Some(encoder) = track.encoder.as_mut() {
                encoder.sync()?;
                track.journal.sync()?;
            }
        }
        self.last_sync = Instant::now();
        Ok(())
    }

    /// The periodic hook of the writer loop: fsync if [`SYNC_INTERVAL`] has
    /// passed since the last sync. Returns whether it synced.
    pub fn sync_if_due(&mut self, now: Instant) -> Result<bool, RecordingError> {
        if now.saturating_duration_since(self.last_sync) < SYNC_INTERVAL {
            return Ok(false);
        }
        self.sync()?;
        self.last_sync = now;
        Ok(true)
    }

    /// End one track (step 4): the other tracks go on. If the disk is full
    /// while the last block is written, the track is cut back like
    /// [`abort_track`](Self::abort_track) does (ended with `DiskFull`, not
    /// `reason`) and the error is still returned.
    pub fn finish_track(
        &mut self,
        track: &str,
        reason: EndedReason,
    ) -> Result<TrackSummary, RecordingError> {
        let index = self.index_of(track)?;
        let encoder = self.tracks[index]
            .encoder
            .take()
            .ok_or(RecordingError::TrackFinished)?;
        let clipped = encoder.clipped_samples();
        let summary = match encoder.finish() {
            Ok(summary) => summary,
            Err(error) => {
                if error.is_disk_full() {
                    // The encoder is gone; the file holds what reached the disk.
                    let _ = self.cut_back(index, EndedReason::DiskFull, clipped);
                }
                return Err(error);
            }
        };
        let track = &mut self.tracks[index];
        track.journal.sync()?;
        track.finished = Some(FinishedTrack {
            frames: summary.frames,
            audio_sha256: summary.audio_sha256.clone(),
            clipped_samples: summary.clipped_samples,
            reason,
            rebuilt: None,
        });
        Ok(summary)
    }

    /// End a track whose file can no longer be written (the disk is full):
    /// close it, cut it back to its last complete frame, rewrite STREAMINFO
    /// and the SEEKTABLE in place (which needs no new space) and mark it
    /// ended with `reason`. The part of a block that could not be written is
    /// dropped (there is no space to copy it to); everything before it is
    /// kept, and gaps and anchors that lie past the cut are removed. Returns
    /// the frames kept.
    pub fn abort_track(&mut self, track: &str, reason: EndedReason) -> Result<u64, RecordingError> {
        let index = self.index_of(track)?;
        let encoder = self.tracks[index]
            .encoder
            .take()
            .ok_or(RecordingError::TrackFinished)?;
        let clipped = encoder.clipped_samples();
        // Closing the file flushes what the buffer still holds, if it can.
        drop(encoder);
        self.cut_back(index, reason, clipped)
    }

    fn cut_back(
        &mut self,
        index: usize,
        reason: EndedReason,
        clipped: u64,
    ) -> Result<u64, RecordingError> {
        let (updated, _) =
            recover_track(&self.dir, &self.manifest.tracks[index], TailPolicy::Discard)?;
        let track = &mut self.tracks[index];
        let _ = track.journal.sync();
        track.finished = Some(FinishedTrack {
            frames: updated.frames,
            audio_sha256: updated.audio_sha256.clone().unwrap_or_default(),
            clipped_samples: clipped.max(updated.clipped_samples),
            reason,
            rebuilt: Some((updated.gaps, updated.clock.anchors)),
        });
        Ok(updated.frames)
    }

    fn index_of(&self, track: &str) -> Result<usize, RecordingError> {
        self.tracks
            .iter()
            .position(|t| t.id == track)
            .ok_or(RecordingError::UnknownTrack)
    }

    /// Whether the track has ended (finished or cut back).
    pub fn is_track_finished(&self, track: &str) -> bool {
        self.tracks
            .iter()
            .find(|t| t.id == track)
            .is_some_and(|t| t.finished.is_some())
    }

    /// Frames pushed to the track so far, silence included.
    pub fn frames_pushed(&self, track: &str) -> u64 {
        self.tracks
            .iter()
            .find(|t| t.id == track)
            .and_then(|t| t.encoder.as_ref())
            .map_or(0, |e| e.frames_pushed())
    }

    /// Bytes of audio files written so far, headers included, over all tracks
    /// that are still recording plus the finished ones as of their end.
    pub fn bytes_written(&self) -> u64 {
        self.tracks
            .iter()
            .filter_map(|t| t.encoder.as_ref().map(|e| e.bytes_written()))
            .sum()
    }

    /// Bytes of one track's file (zero once it ended).
    pub fn track_bytes(&self, track: &str) -> u64 {
        self.tracks
            .iter()
            .find(|t| t.id == track)
            .and_then(|t| t.encoder.as_ref())
            .map_or(0, |e| e.bytes_written())
    }

    /// End every track still running with `reason` and write the final
    /// manifest (step 5). If a track fails to finish, the manifest keeps
    /// `state: recording` and [`recover`](super::recover) finishes the job.
    pub fn finish(mut self, reason: EndedReason) -> Result<Manifest, RecordingError> {
        let live: Vec<String> = self
            .tracks
            .iter()
            .filter(|track| track.encoder.is_some())
            .map(|track| track.id.clone())
            .collect();
        for id in live {
            self.finish_track(&id, reason.clone())?;
        }

        let mut manifest = self.manifest.clone();
        manifest.state = RecordingState::Complete;
        for (entry, written) in manifest.tracks.iter_mut().zip(&self.tracks) {
            let Some(done) = &written.finished else {
                return Err(RecordingError::Encoder {
                    message: "a track did not finish".into(),
                });
            };
            entry.frames = done.frames;
            entry.audio_sha256 = Some(done.audio_sha256.clone());
            match &done.rebuilt {
                Some((gaps, anchors)) => {
                    entry.clock.anchors = anchors.clone();
                    entry.gaps = gaps.clone();
                }
                None => {
                    entry.clock.anchors = written.anchors.clone();
                    entry.gaps = written.gaps.clone();
                }
            }
            entry.ended_reason = Some(done.reason.clone());
            entry.clipped_samples = done.clipped_samples;
        }
        write_final_manifest(
            &self.dir.join(MANIFEST_FILE),
            &serialize_manifest(&manifest)?,
        )?;
        // The recording is complete whether or not the lock file goes.
        if let Err(error) = fs::remove_file(self.dir.join(LOCK_FILE)) {
            tracing::warn!(kind = ?error.kind(), "could not remove a recording lock file");
        }
        Ok(manifest)
    }
}

/// Step 5's manifest write. The normal way is a temporary file and a rename.
/// When that cannot happen (the disk is full, so there is no room for a
/// second file; or the folder has become read-only) the new manifest is
/// written over the old one, whose blocks are already allocated, if it fits
/// in the space the in-progress manifest was padded to. That write is not
/// atomic, which is why it is only the fallback; when it fails too the
/// manifest keeps `state: recording` and recovery finishes the job.
fn write_final_manifest(path: &Path, bytes: &[u8]) -> Result<(), RecordingError> {
    let Err(first) = replace_file_durably(path, bytes) else {
        return Ok(());
    };
    tracing::warn!(kind = ?first.kind(), "could not replace the manifest; writing it in place");
    let in_place = (|| -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = open_read_write(path)?;
        let old_len = file.metadata()?.len();
        if (bytes.len() as u64) > old_len {
            return Err(std::io::Error::new(
                ErrorKind::StorageFull,
                "the manifest has outgrown its reserved space",
            ));
        }
        let mut padded = bytes.to_vec();
        padded.resize(old_len as usize, b' ');
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&padded)?;
        sync_file(&file)
    })();
    in_place.map_err(|_| RecordingError::io("write the recording manifest", &first))
}

/// Read a file of at most `limit` bytes into memory.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, RecordingError> {
    let file =
        File::open(path).map_err(|error| RecordingError::io("open a recording file", &error))?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| RecordingError::io("read a recording file", &error))?;
    if bytes.len() as u64 > limit {
        return Err(RecordingError::ManifestInvalid {
            reason: "the manifest is too large",
        });
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
