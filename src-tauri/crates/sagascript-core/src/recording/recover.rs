//! Finishing the recordings a process left behind (design §4.5).
//!
//! Recovery looks only at folders whose manifest says `recording` **and**
//! whose `recording.lock` it can take without blocking, so a recording owned
//! by another live process (the CLI while the app starts, a second app
//! instance) is never touched. The lock is taken first and the manifest read
//! again under it, so a recording that finished in between is left alone.
//!
//! For each track it
//!
//! 1. walks the frames from the first one (header CRC-8, subframe structure,
//!    frame CRC-16, the next frame number) and keeps **exactly** the valid
//!    ones; a frame that does not decode is not kept either;
//! 2. copies every byte after the last valid frame to `<track>.tail` (fsynced)
//!    and only then truncates the track file, so no audio is ever deleted. A
//!    torn final frame is real: `flac -t` rejects it while other decoders skip
//!    it;
//! 3. decodes the kept frames to recompute the MD5 of the PCM, hashes their
//!    bytes for `audio_sha256`, and rewrites STREAMINFO and the SEEKTABLE in
//!    place. The audio always starts at the same offset ([`HEADER_LEN`]), so a
//!    header that does not read (a torn rewrite at the end of a recording,
//!    damage) is rebuilt from the manifest and the frames, never taken for
//!    "the file ends in its header": only a file *shorter* than a header
//!    has never held audio. A file that cannot be shown to be this track's
//!    (no readable stream parameters and no valid first frame) is refused
//!    and left as it is;
//! 4. rebuilds the gaps and clock anchors from the journal, dropping what
//!    refers to audio that was not kept (a gap that straddles the end is
//!    shortened).
//!
//! The folder comes from anywhere, so nothing in it is trusted to be what its
//! name says: the folder itself must not be a link, and the manifest, the
//! lock, the journal and every track file are opened with
//! [`open_member`](super::io::open_member), which refuses a link (never
//! follows it), a directory, a pipe or a device. The track file named by the
//! manifest can therefore never make recovery truncate or rewrite a file
//! outside the folder.
//!
//! The manifest is then written with `state: complete`, `recovered: true` and
//! `ended_reason: interrupted`. Recovery of a recovered folder finds nothing
//! to do ([`RecordingError::NotRecording`]); a run that was itself interrupted
//! can simply be run again.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::error::RecordingError;
use super::flac::decode::Md5Calculator;
use super::flac::format::{
    build_header, parse_header, parse_prefix, HeaderError, ParsedHeader, SeekIndex, StreamParams,
    StreamTotals, BLOCK_SIZE, HEADER_LEN, RESERVED_LEN,
};
use super::flac::scan::{scan_frames, walk_frame, Walk};
use super::io::{create_new_private, open_member, replace_file_durably, sync_dir, sync_file, Open};
use super::journal::{journal_file_name, read_journal};
use super::manifest::{
    is_plain_file_name, manifest_integrity, parse_manifest, serialize_manifest_sealed, Clock,
    EndedReason, Gap, Integrity, Manifest, RecordingState, Track, MANIFEST_FILE,
};
use super::store::{
    lock_folder, read_bounded, RecordingStore, LOCK_FILE, MAX_MANIFEST_BYTES, START_COPY_FILE,
};
use crate::hash::StreamingSha256;

/// How many `.tail` files of one track are tried.
const MAX_TAIL_FILES: usize = 1000;

/// What recovery did to one track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackRecovery {
    pub id: String,
    /// Sample frames kept (the sum of the valid blocks).
    pub frames_kept: u64,
    /// Bytes moved out of the track file.
    pub tail_bytes: u64,
    /// The file the tail went to, relative to the recording folder.
    pub tail_file: Option<String>,
    pub gaps_kept: usize,
    /// Journalled gaps that refer to audio that was not kept.
    pub gaps_dropped: usize,
    /// Journal lines that were torn or not understood.
    pub journal_lines_ignored: usize,
    /// The track's metadata header was missing or damaged and was written
    /// again from the manifest and the frames.
    pub header_rebuilt: bool,
}

/// A recording that recovery finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredRecording {
    /// The manifest now on disk.
    pub manifest: Manifest,
    pub tracks: Vec<TrackRecovery>,
}

/// What happened to one folder during a scan of the recordings folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Recovered(Box<RecoveredRecording>),
    /// Another process owns the recording; it was not touched.
    SkippedLocked,
    /// The manifest says the recording is finished.
    SkippedNotRecording,
    /// Recovery failed; the folder was not marked complete and can be
    /// retried. The code is a [`RecordingError::code`].
    Failed {
        code: &'static str,
    },
}

/// A folder and what recovery did there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    /// The folder's name inside the recordings folder.
    pub folder: String,
    pub outcome: RecoveryOutcome,
}

impl RecordingStore {
    /// Recover every folder in the store whose manifest says `recording`. One
    /// folder's failure never stops the others. A missing recordings folder
    /// has nothing to recover.
    pub fn recover_pending(&self) -> Result<Vec<RecoveryReport>, RecordingError> {
        let entries = match fs::read_dir(self.root()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(RecordingError::io("list the recordings folder", &error)),
        };
        let mut folders: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            // Whatever is named like the manifest makes the folder a candidate;
            // `recover_folder` then reports a link or a directory there.
            .filter(|entry| fs::symlink_metadata(entry.path().join(MANIFEST_FILE)).is_ok())
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .collect();
        folders.sort();
        Ok(folders
            .into_iter()
            .map(|(folder, path)| RecoveryReport {
                folder,
                outcome: match recover_folder(&path) {
                    Ok(recovered) => RecoveryOutcome::Recovered(Box::new(recovered)),
                    Err(RecordingError::Locked) => RecoveryOutcome::SkippedLocked,
                    Err(RecordingError::NotRecording) => RecoveryOutcome::SkippedNotRecording,
                    Err(error) => RecoveryOutcome::Failed { code: error.code() },
                },
            })
            .collect())
    }

    /// Recover one folder of the store by name.
    pub fn recover(&self, folder_name: &str) -> Result<RecoveredRecording, RecordingError> {
        if !is_plain_file_name(folder_name) {
            return Err(RecordingError::InvalidInput {
                reason: "a recording folder is a plain name",
            });
        }
        recover_folder(&self.root().join(folder_name))
    }
}

/// Recover the recording in `dir`; see the module documentation. Fails with
/// [`RecordingError::Locked`] while another process (or handle) owns it and
/// with [`RecordingError::NotRecording`] when there is nothing to do.
pub fn recover_folder(dir: &Path) -> Result<RecoveredRecording, RecordingError> {
    // A symbolic link is never followed: the folder must be the real thing.
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| RecordingError::io("open the recording folder", &error))?;
    if !metadata.is_dir() {
        return Err(RecordingError::InvalidInput {
            reason: "the recording folder is not a directory",
        });
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    // A finished recording needs no lock and no write access.
    let early = read_manifest_for_recovery(dir)?;
    if early.state != RecordingState::Recording {
        return Err(RecordingError::NotRecording);
    }

    let lock = match lock_folder(dir, false) {
        Err(RecordingError::Io {
            kind: std::io::ErrorKind::NotFound,
            ..
        }) => lock_folder(dir, true),
        result => result,
    }?;
    // Under the lock: the recording may have finished since the first look.
    let manifest = read_manifest_for_recovery(dir)?;
    if manifest.state != RecordingState::Recording {
        return Err(RecordingError::NotRecording);
    }

    let mut recovered = manifest.clone();
    let mut reports = Vec::with_capacity(manifest.tracks.len());
    for (entry, track) in recovered.tracks.iter_mut().zip(&manifest.tracks) {
        let (updated, report) = recover_track(dir, track, TailPolicy::Keep)?;
        *entry = updated;
        reports.push(report);
    }
    recovered.state = RecordingState::Complete;
    recovered.recovered = true;
    replace_file_durably(&manifest_path, &serialize_manifest_sealed(&recovered)?)
        .map_err(|error| RecordingError::io("write the recording manifest", &error))?;
    let _ = fs::remove_file(dir.join(START_COPY_FILE));
    if let Err(error) = fs::remove_file(dir.join(LOCK_FILE)) {
        tracing::warn!(kind = ?error.kind(), "could not remove a recording lock file");
    }
    drop(lock);
    Ok(RecoveredRecording {
        manifest: recovered,
        tracks: reports,
    })
}

/// Read `recording.json`; if it cannot be trusted (it is not a manifest, or it
/// is sealed and the seal fails: a rewrite in place that a crash cut short),
/// read the copy of the start manifest instead. That copy says `recording` and
/// describes the tracks; everything else recovery needs it takes from the
/// track files and journals.
fn read_manifest_for_recovery(dir: &Path) -> Result<Manifest, RecordingError> {
    let bytes = read_bounded(&dir.join(MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    let primary = match manifest_integrity(&bytes) {
        Integrity::Invalid => Err(RecordingError::ManifestInvalid {
            reason: "the manifest does not match its seal",
        }),
        _ => parse_manifest(&bytes),
    };
    let Err(error @ RecordingError::ManifestInvalid { .. }) = primary else {
        return primary;
    };
    let copy = read_bounded(&dir.join(START_COPY_FILE), MAX_MANIFEST_BYTES);
    match copy {
        Ok(bytes) if manifest_integrity(&bytes) != Integrity::Invalid => {
            match parse_manifest(&bytes) {
                Ok(manifest) => {
                    tracing::warn!("recovering from the start copy of the manifest");
                    Ok(manifest)
                }
                Err(_) => Err(error),
            }
        }
        _ => Err(error),
    }
}

/// What recovery does with the bytes after the last valid frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TailPolicy {
    /// Copy them to `<track>.tail` first (a recording left by a dead process).
    Keep,
    /// Drop them. Used by the live session when the disk is full: the bytes
    /// are the part of one block that could not be written, there is no space
    /// for a copy, and the process knows exactly what it had accepted.
    Discard,
}

/// Recover one track: the new manifest entry and what was done.
pub(crate) fn recover_track(
    dir: &Path,
    track: &Track,
    policy: TailPolicy,
) -> Result<(Track, TrackRecovery), RecordingError> {
    let params = StreamParams {
        sample_rate: track.sample_rate,
        channels: track.channels,
        bits: track.bits,
    };
    params
        .validate_for_encoding()
        .map_err(|_| RecordingError::CorruptTrack {
            reason: "the manifest describes a format this store does not write",
        })?;
    // The journal first: if it cannot be read in full (too many events, an I/O
    // error) nothing of the track has been touched yet and a retry starts clean.
    let journal = read_journal(&dir.join(journal_file_name(&track.id)))?;
    let path = dir.join(&track.file);
    let mut tail_bytes = 0u64;
    let mut tail_file = None;

    let mut file = match open_member(&path, Open::Rewrite, "open a track file") {
        Ok(file) => file,
        // The process died before the track file existed: nothing was lost.
        Err(RecordingError::Io {
            kind: std::io::ErrorKind::NotFound,
            ..
        }) => {
            let file = create_new_private(&path)
                .map_err(|error| RecordingError::io("create a track file", &error))?;
            sync_dir(dir)
                .map_err(|error| RecordingError::io("sync the recording folder", &error))?;
            file
        }
        Err(error) => return Err(error),
    };
    let length = file
        .metadata()
        .map_err(|error| RecordingError::io("inspect a track file", &error))?
        .len();

    let mut header_rebuilt = false;
    let (header, length) = match classify_start(&mut file, length, params)? {
        TrackStart::Intact(header) => (header, length),
        TrackStart::Unfinished => {
            // The process died while the file was being created: whatever
            // bytes there are go to a tail and the file starts over.
            if length > 0 {
                let (name, bytes) = save_tail(dir, &track.id, &mut file, 0, length)?;
                tail_file = Some(name);
                tail_bytes += bytes;
            }
            file.set_len(0)
                .map_err(|error| RecordingError::io("truncate a track file", &error))?;
            file.seek(SeekFrom::Start(0))
                .map_err(|error| RecordingError::io("rewind a track file", &error))?;
            let fresh = build_header(params, None, &[], RESERVED_LEN)?;
            file.write_all(&fresh)
                .map_err(|error| RecordingError::io("write a track header", &error))?;
            sync_file(&file).map_err(|error| RecordingError::io("sync a track file", &error))?;
            header_rebuilt = true;
            (fresh_header(params)?, HEADER_LEN as u64)
        }
        TrackStart::Damaged => {
            // The metadata is unreadable but the file is ours, so the audio
            // starts where the layout says it does. The header is rebuilt in
            // place at the end, from the manifest and the frames; nothing
            // about the audio depends on what the old one said. (A recovery
            // interrupted before that finds the same damage and does the
            // same.)
            header_rebuilt = true;
            (fresh_header(params)?, length)
        }
    };

    // Walk the frames, keeping the valid ones.
    file.seek(SeekFrom::Start(header.first_frame as u64))
        .map_err(|error| RecordingError::io("read a track file", &error))?;
    let mut md5 = Md5Calculator::new(&header.streaminfo, params)?;
    let mut sha256 = StreamingSha256::new();
    let mut seek = SeekIndex::new(params.sample_rate);
    let (mut min_frame, mut max_frame) = (u32::MAX, 0u32);
    let first_frame = header.first_frame as u64;
    let summary = scan_frames(&mut file, first_frame, params, &mut |frame| {
        if !md5.add_frame(frame.bytes) {
            return false;
        }
        sha256.update(frame.bytes);
        let size = frame.bytes.len() as u32;
        min_frame = min_frame.min(size);
        max_frame = max_frame.max(size);
        seek.observe(
            frame.frame_number,
            frame.frame_number * BLOCK_SIZE as u64,
            frame.offset - first_frame,
            frame.block_size,
        );
        true
    })
    .map_err(|error| RecordingError::io("read a track file", &error))?;
    let (md5_digest, decoded_samples) = md5.finish();
    debug_assert_eq!(decoded_samples, summary.samples);

    // Anything after the last valid frame goes to the tail, then the file is cut.
    if length > summary.end_offset {
        if policy == TailPolicy::Keep {
            let (name, bytes) = save_tail(dir, &track.id, &mut file, summary.end_offset, length)?;
            tail_file = Some(name);
            tail_bytes += bytes;
        } else {
            tail_bytes += length - summary.end_offset;
        }
        file.set_len(summary.end_offset)
            .map_err(|error| RecordingError::io("truncate a track file", &error))?;
    }

    // STREAMINFO and the SEEKTABLE, in place.
    let totals = StreamTotals {
        total_samples: summary.samples,
        md5: md5_digest,
        min_frame_size: if summary.frames == 0 { 0 } else { min_frame },
        max_frame_size: max_frame,
    };
    let rewritten = build_header(params, Some(&totals), seek.points(), header.region_len())?;
    if rewritten.len() != header.first_frame {
        return Err(RecordingError::CorruptTrack {
            reason: "the track header changed size",
        });
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| RecordingError::io("rewind a track file", &error))?;
    file.write_all(&rewritten)
        .map_err(|error| RecordingError::io("write a track header", &error))?;
    sync_file(&file).map_err(|error| RecordingError::io("sync a track file", &error))?;
    drop(file);
    if tail_file.is_some() {
        sync_dir(dir).map_err(|error| RecordingError::io("sync the recording folder", &error))?;
    }

    // The journal: the clock and the gaps, limited to the audio that is kept.
    let frames = summary.samples;
    // Only events inside the kept audio count: an event past it counted
    // samples that were lost with the end of the file.
    let clipped = journal.clipped_within(frames);
    let mut gaps = Vec::new();
    let mut gaps_dropped = 0;
    for gap in journal.gaps {
        if gap.at_frame >= frames {
            gaps_dropped += 1;
            continue;
        }
        let length = gap.length.min(frames - gap.at_frame);
        gaps.push(Gap { length, ..gap });
    }
    let mut clock: Clock = track.clock.clone();
    if let Some((domain, start_ns)) = journal.start {
        clock.domain = domain;
        clock.start_ns = start_ns;
        clock.anchors = journal
            .anchors
            .iter()
            .copied()
            .filter(|(_, frame)| *frame == 0 || *frame < frames)
            .collect();
    }

    let mut updated = track.clone();
    updated.frames = frames;
    updated.audio_sha256 = Some(sha256.finish_hex());
    updated.clock = clock;
    updated.gaps = gaps;
    updated.ended_reason = Some(EndedReason::Interrupted);
    updated.clipped_samples = clipped.max(track.clipped_samples);
    let report = TrackRecovery {
        id: track.id.clone(),
        frames_kept: frames,
        tail_bytes,
        tail_file,
        gaps_kept: updated.gaps.len(),
        gaps_dropped,
        journal_lines_ignored: journal.ignored_lines,
        header_rebuilt,
    };
    Ok((updated, report))
}

/// What the first bytes of a track file say about it.
enum TrackStart {
    /// A header of the layout this store writes, for the manifest's format.
    Intact(ParsedHeader),
    /// The file is empty or ends inside the header it was created with: it
    /// never held a frame.
    Unfinished,
    /// The file is this track's, but its metadata header does not read: a torn
    /// rewrite at the end of a recording or of an earlier recovery, or damage.
    /// The audio starts at [`HEADER_LEN`], where this store always puts it.
    Damaged,
}

/// How much of a file is looked at to find the first frame: more than any
/// frame this store writes (4096 stereo 24-bit samples stored verbatim).
const FIRST_FRAME_PROBE_BYTES: usize = 64 * 1024;

/// Decide what a track file is, reading at most [`HEADER_LEN`] bytes of it
/// and one frame. A header that does not parse is **not** taken to mean "the
/// file ends inside its header": that is true only of a file shorter than a
/// header. A file at least that long has had its header written whole, and an
/// unreadable one is damage (a torn in-place rewrite, say) in front of audio
/// that is still there. Anything that cannot be shown to be this track's is
/// refused and left exactly as it is.
fn classify_start(
    file: &mut File,
    length: u64,
    params: StreamParams,
) -> Result<TrackStart, RecordingError> {
    if length == 0 {
        return Ok(TrackStart::Unfinished);
    }
    let read_error = |error: std::io::Error| RecordingError::io("read a track file", &error);
    let mut bytes = vec![0u8; length.min(HEADER_LEN as u64) as usize];
    file.seek(SeekFrom::Start(0)).map_err(read_error)?;
    file.read_exact(&mut bytes).map_err(read_error)?;

    let refuse = |reason| Err(RecordingError::CorruptTrack { reason });
    let mismatch = "the track file does not match the manifest";
    // The stream parameters are the first bytes ever written and the same in
    // every version of the header: if they are readable, they must be right.
    let prefix = parse_prefix(&bytes);
    if prefix.is_some_and(|prefix| prefix != params) {
        return refuse(mismatch);
    }
    let whole_header = length >= HEADER_LEN as u64;
    match parse_header(&bytes) {
        Ok(header) if header.params != params => refuse(mismatch),
        Ok(header) if header.first_frame == HEADER_LEN => Ok(TrackStart::Intact(header)),
        // A well-formed header of another layout is not damage: only audio
        // where this store puts it shows that the file is ours.
        Ok(_) if first_frame_is_valid(file, length, params)? => Ok(TrackStart::Damaged),
        Ok(_) => refuse("the track file does not have the layout this store writes"),
        Err(HeaderError::Truncated) if !whole_header => Ok(TrackStart::Unfinished),
        Err(_) if !whole_header => {
            refuse("the track file is not a FLAC file written by this store")
        }
        // A whole header's worth of bytes that does not read.
        Err(_) if prefix.is_some() || first_frame_is_valid(file, length, params)? => {
            Ok(TrackStart::Damaged)
        }
        Err(_) => refuse("the track file is not a FLAC file written by this store"),
    }
}

/// Is a valid frame number 0 of this stream at [`HEADER_LEN`]?
fn first_frame_is_valid(
    file: &mut File,
    length: u64,
    params: StreamParams,
) -> Result<bool, RecordingError> {
    let available = length.saturating_sub(HEADER_LEN as u64);
    let mut bytes = vec![0u8; available.min(FIRST_FRAME_PROBE_BYTES as u64) as usize];
    if bytes.is_empty() {
        return Ok(false);
    }
    let read_error = |error: std::io::Error| RecordingError::io("read a track file", &error);
    file.seek(SeekFrom::Start(HEADER_LEN as u64))
        .map_err(read_error)?;
    file.read_exact(&mut bytes).map_err(read_error)?;
    Ok(matches!(walk_frame(&bytes, params, 0), Walk::Frame(_)))
}

/// The placeholder header for `params`, parsed: its STREAMINFO is what the
/// MD5 decoder is configured with.
fn fresh_header(params: StreamParams) -> Result<ParsedHeader, RecordingError> {
    let fresh = build_header(params, None, &[], RESERVED_LEN)?;
    parse_header(&fresh).map_err(|_| RecordingError::CorruptTrack {
        reason: "the track header could not be rebuilt",
    })
}

/// Copy `from..to` of `file` to a new `<track>.tail` file (fsynced) and return
/// its name and size. A name that exists gets a number.
fn save_tail(
    dir: &Path,
    track_id: &str,
    file: &mut File,
    from: u64,
    to: u64,
) -> Result<(String, u64), RecordingError> {
    let (name, mut tail) = (0..MAX_TAIL_FILES)
        .find_map(|n| {
            let name = if n == 0 {
                format!("{track_id}.tail")
            } else {
                format!("{track_id}.tail.{n}")
            };
            match create_new_private(&dir.join(&name)) {
                Ok(tail) => Some(Ok((name, tail))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(RecordingError::io("create a tail file", &error))),
            }
        })
        .unwrap_or(Err(RecordingError::NameUnavailable))?;
    file.seek(SeekFrom::Start(from))
        .map_err(|error| RecordingError::io("read a track file", &error))?;
    let mut remaining = to - from;
    let mut buffer = vec![0u8; 1024 * 1024];
    while remaining > 0 {
        let want = buffer.len().min(remaining as usize);
        let read = file
            .read(&mut buffer[..want])
            .map_err(|error| RecordingError::io("read a track file", &error))?;
        if read == 0 {
            break;
        }
        tail.write_all(&buffer[..read])
            .map_err(|error| RecordingError::io("write a tail file", &error))?;
        remaining -= read as u64;
    }
    sync_file(&tail).map_err(|error| RecordingError::io("sync a tail file", &error))?;
    sync_dir(dir).map_err(|error| RecordingError::io("sync the recording folder", &error))?;
    Ok((name, to - from - remaining))
}

#[cfg(test)]
#[path = "recover_tests.rs"]
mod tests;
