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
//!    [`SYNC_INTERVAL`](super::SYNC_INTERVAL) (via [`RecordingWriter::sync_if_due`]).
//!    A gap is journaled and the journal fsynced **before** its silence is
//!    written, and the track is fsynced after. A gap that fails part-way ends
//!    its track until recovery (see [`RecordingWriter::gap`]).
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
    create_dir_private, create_new_private, open_member, replace_file_durably, sync_dir, Open,
};
use super::journal::{journal_file_name, JournalEvent, JournalWriter};
use super::manifest::{
    is_valid_track_id, serialize_manifest, Client, Clock, EndedReason, Gap, GapReason, Manifest,
    Quality, RecordingState, Track, MANIFEST_FILE, MIN_READER_VERSION, SCHEMA_VERSION,
};
use super::naming;
use super::SYNC_INTERVAL;

/// The advisory lock file of a recording in progress.
pub const LOCK_FILE: &str = "recording.lock";

/// How many ids are tried for a free folder name.
const MAX_NAME_ATTEMPTS: usize = 8;
/// The longest title stored (UTF-8 bytes).
const MAX_STORED_TITLE_BYTES: usize = 1000;

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
        validate_request(&request)?;
        let stamp = folder_stamp(&request.started_at)?;
        fs::create_dir_all(&self.root)
            .map_err(|error| RecordingError::io("create the recordings folder", &error))?;

        let (dir, folder_name, recording_id) = self.claim_folder(&request, &stamp, next_id)?;
        match start_in(&dir, folder_name, recording_id, request) {
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
    let mode = if create {
        Open::ReadWriteCreate
    } else {
        Open::ReadWrite
    };
    let file = open_member(&dir.join(LOCK_FILE), mode, "open the recording lock")?;
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
    replace_file_durably(&dir.join(MANIFEST_FILE), &serialize_manifest(&manifest)?)
        .map_err(|error| RecordingError::io("write the recording manifest", &error))?;

    let mut tracks = Vec::with_capacity(manifest.tracks.len());
    for (new, entry) in request.tracks.iter().zip(&manifest.tracks) {
        let mut encoder = FlacEncoder::create(
            &dir.join(&entry.file),
            new.sample_rate,
            new.channels,
            new.bits,
        )?;
        encoder.sync()?;
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
            encoder: Some(Box::new(encoder)),
            journal,
            anchors: vec![(new.start_ns, 0)],
            gaps: Vec::new(),
            finished: None,
            failed: false,
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
        #[cfg(test)]
        crash_after: None,
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

/// The steps of [`RecordingWriter::gap`] at which a test can stop the
/// "process", in the order the code takes them.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GapStep {
    /// The gap was checked; nothing is written yet.
    Checked,
    /// The journal line is written (not yet synced).
    JournalAppended,
    /// The journal line is durable; no silence has been encoded.
    JournalSynced,
    /// The silence is encoded (full blocks may be on their way to the file).
    SilencePushed,
    /// The writer knows of the gap.
    GapRecorded,
    /// The track is fsynced: the whole gap is durable.
    TrackSynced,
}

#[cfg(test)]
impl GapStep {
    pub(crate) const ALL: [GapStep; 6] = [
        GapStep::Checked,
        GapStep::JournalAppended,
        GapStep::JournalSynced,
        GapStep::SilencePushed,
        GapStep::GapRecorded,
        GapStep::TrackSynced,
    ];
}

/// A test's crash: fails with an I/O error when `armed` is `step`.
#[cfg(test)]
fn failpoint(armed: Option<GapStep>, step: GapStep) -> Result<(), RecordingError> {
    if armed == Some(step) {
        return Err(RecordingError::io(
            "continue after an injected crash",
            &std::io::Error::other("injected"),
        ));
    }
    Ok(())
}

struct TrackWriter {
    id: String,
    encoder: Option<Box<dyn TrackEncoder>>,
    journal: JournalWriter,
    anchors: Vec<(u64, u64)>,
    gaps: Vec<Gap>,
    finished: Option<(TrackSummary, EndedReason)>,
    /// A gap failed part-way (see [`RecordingWriter::gap`]): the journal may
    /// hold an intent for silence that was never written, and audio written
    /// now would fall inside it. The track takes nothing more and cannot be
    /// finished by the writer; recovery finishes it.
    failed: bool,
}

impl TrackWriter {
    /// The encoder, if the track can still be written to.
    fn live(&mut self) -> Result<&mut Box<dyn TrackEncoder>, RecordingError> {
        if self.failed {
            return Err(RecordingError::TrackFinished);
        }
        self.encoder.as_mut().ok_or(RecordingError::TrackFinished)
    }

    /// The part of a gap that cannot be undone: the intent, then the silence.
    /// Every failure in here leaves the track in doubt.
    fn commit_gap(
        &mut self,
        length: u64,
        reason: &GapReason,
        #[cfg(test)] crash_after: Option<GapStep>,
    ) -> Result<(), RecordingError> {
        let encoder = self.encoder.as_mut().ok_or(RecordingError::TrackFinished)?;
        let at_frame = encoder.frames_pushed();
        let clipped = encoder.clipped_samples();
        // The intention is durable **before** any of the silence is written:
        // the operating system may flush a written frame at any moment, and
        // silence that reached the disk with no journal line would be
        // presented as captured audio. A gap whose silence never arrives is
        // harmless: recovery drops what lies beyond the kept audio.
        self.journal.append(&JournalEvent::Gap {
            at_frame,
            length,
            reason: reason.clone(),
            clipped,
        })?;
        #[cfg(test)]
        failpoint(crash_after, GapStep::JournalAppended)?;
        self.journal.sync()?;
        #[cfg(test)]
        failpoint(crash_after, GapStep::JournalSynced)?;
        encoder.push_silence(length)?;
        #[cfg(test)]
        failpoint(crash_after, GapStep::SilencePushed)?;
        self.gaps.push(Gap::new(at_frame, length, reason.clone()));
        #[cfg(test)]
        failpoint(crash_after, GapStep::GapRecorded)?;
        Ok(())
    }
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
    /// Tests: stop `gap` after this step, as a killed process would.
    #[cfg(test)]
    pub(crate) crash_after: Option<GapStep>,
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
        self.track_mut(track)?.live()?.push(interleaved)
    }

    /// Record that the next frame of the track (index `frames_pushed`) was
    /// captured at `host_ns`: a clock anchor, journalled at once.
    pub fn anchor(&mut self, track: &str, host_ns: u64) -> Result<(), RecordingError> {
        let track = self.track_mut(track)?;
        let encoder = track.live()?;
        let frame = encoder.frames_pushed();
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
    /// the frame index still maps to time, and record the gap. The journal
    /// line is fsynced **before** the silence is written and the track after,
    /// so a crash at any point leaves either no silence or silence the
    /// journal accounts for. A gap the track cannot hold is refused first,
    /// and the track goes on.
    ///
    /// If the journal line cannot be appended or synced, or the silence cannot
    /// be written, the outcome is in doubt (the line may be on disk for
    /// silence that is not), so the track **ends until recovery**: `push`,
    /// `anchor`, `gap` and `finish_track` then fail with
    /// [`RecordingError::TrackFinished`], `sync` skips it, and the recording
    /// stays `state: recording` for [`recover`](super::recover) to finish.
    /// Nothing written before the failure is lost. A failing fsync of the
    /// finished gap is not in doubt; it is reported and the track goes on.
    pub fn gap(
        &mut self,
        track: &str,
        length: u64,
        reason: GapReason,
    ) -> Result<(), RecordingError> {
        if length == 0 {
            return Ok(());
        }
        #[cfg(test)]
        let crash_after = self.crash_after;
        let track = self.track_mut(track)?;
        // Refused here, before a byte is journaled or written: a gap the
        // track cannot hold must leave no line that claims it exists, and the
        // track goes on as it was.
        track.live()?.ensure_room(length)?;
        #[cfg(test)]
        failpoint(crash_after, GapStep::Checked)?;
        // From here on a failure leaves the track in doubt: the intent may or
        // may not be on disk and the silence may or may not be written. The
        // track ends (until recovery), because a caller that went on writing
        // audio would put it where the journal says there is silence.
        if let Err(error) = track.commit_gap(
            length,
            &reason,
            #[cfg(test)]
            crash_after,
        ) {
            track.failed = true;
            return Err(error);
        }
        // The gap is committed. A failing fsync now is the same failure a
        // periodic sync can have: reported, and the track is not in doubt.
        track.live()?.sync()?;
        #[cfg(test)]
        failpoint(crash_after, GapStep::TrackSynced)?;
        Ok(())
    }

    /// Fsync every live track and journal.
    pub fn sync(&mut self) -> Result<(), RecordingError> {
        for track in &mut self.tracks {
            // A failed track writes nothing more; its error was reported when
            // it happened.
            if track.failed {
                continue;
            }
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

    /// End one track (step 4): the other tracks go on.
    pub fn finish_track(
        &mut self,
        track: &str,
        reason: EndedReason,
    ) -> Result<TrackSummary, RecordingError> {
        let track = self.track_mut(track)?;
        // A track in doubt is finished by recovery, not here: its journal may
        // hold an intent that its audio does not.
        if track.failed {
            return Err(RecordingError::TrackFinished);
        }
        let encoder = track.encoder.take().ok_or(RecordingError::TrackFinished)?;
        let summary = encoder.finish()?;
        track.journal.sync()?;
        track.finished = Some((summary.clone(), reason));
        Ok(summary)
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
            let Some((summary, ended)) = &written.finished else {
                return Err(RecordingError::Encoder {
                    message: "a track did not finish".into(),
                });
            };
            entry.frames = summary.frames;
            entry.audio_sha256 = Some(summary.audio_sha256.clone());
            entry.clock.anchors = written.anchors.clone();
            entry.gaps = written.gaps.clone();
            entry.ended_reason = Some(ended.clone());
            entry.clipped_samples = summary.clipped_samples;
        }
        replace_file_durably(
            &self.dir.join(MANIFEST_FILE),
            &serialize_manifest(&manifest)?,
        )
        .map_err(|error| RecordingError::io("write the recording manifest", &error))?;
        // The recording is complete whether or not the lock file goes.
        if let Err(error) = fs::remove_file(self.dir.join(LOCK_FILE)) {
            tracing::warn!(kind = ?error.kind(), "could not remove a recording lock file");
        }
        Ok(manifest)
    }
}

/// Read a file of at most `limit` bytes into memory.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, RecordingError> {
    let file = open_member(path, Open::Read, "open a recording file")?;
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
