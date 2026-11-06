//! The recordings folder as a library (SAGA-564): list, look up, delete.
//!
//! Everything here works on the folders [`RecordingStore`](super::RecordingStore)
//! writes. A **name** the caller passes is untrusted (a CLI argument, an IPC
//! string): it must be one plain path component, so it can never leave the
//! recordings folder, and a folder is only touched if it has a
//! `recording.json`. The recording id (a UUID, shown short in the folder name)
//! may be given instead, as a prefix of at least [`MIN_ID_PREFIX`] characters
//! that matches exactly one recording.
//!
//! Errors never carry a path; a folder that cannot be read while listing is
//! skipped and logged without its name.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::error::RecordingError;
use super::manifest::{is_plain_file_name, Manifest, Quality, RecordingState, MANIFEST_FILE};
use super::store::{load_manifest, lock_folder, LOCK_FILE};

/// The shortest recording-id prefix accepted as a name.
pub const MIN_ID_PREFIX: usize = 6;

/// What a list needs to know about a recording.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecordingSummary {
    /// The folder's name inside the recordings folder; the handle for every
    /// other operation.
    pub folder_name: String,
    pub recording_id: String,
    pub title: String,
    pub state: RecordingState,
    /// True when recovery finalised the recording.
    pub recovered: bool,
    pub started_at: String,
    pub quality: Quality,
    /// The longest track. Zero for a recording that is not finished (the
    /// manifest holds the frame counts only once it is).
    pub duration_seconds: f64,
    /// Bytes of the files in the folder.
    pub bytes: u64,
    pub track_count: usize,
    pub gap_count: usize,
    pub gap_seconds: f64,
    /// Why recording ended: the first reason other than `stopped`, else
    /// `stopped`; `None` while it is not finished.
    pub ended_reason: Option<String>,
    /// Of the first track (the microphone).
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    /// A process died while recording: the manifest still says `recording`
    /// and nothing owns the folder. Recovery finishes it.
    pub needs_recovery: bool,
    /// Complete, so it can be played, exported and transcribed.
    pub playable: bool,
}

/// The recordings in `root`, newest first (by start time, then name). A
/// folder without a readable manifest is skipped. A missing `root` is empty.
pub fn list_recordings(root: &Path) -> Vec<RecordingSummary> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<(Option<i64>, RecordingSummary)> = Vec::new();
    for entry in entries.flatten() {
        // `DirEntry::file_type` does not follow a symbolic link.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        if !dir.join(MANIFEST_FILE).is_file() {
            continue;
        }
        match load_manifest(&dir) {
            Ok(manifest) => {
                let key = epoch_seconds(&manifest.started_at);
                found.push((key, summarize(&name, &dir, &manifest)));
            }
            Err(error) => {
                tracing::debug!(code = error.code(), "a recording folder was skipped");
            }
        }
    }
    found.sort_by(|(a_key, a), (b_key, b)| {
        b_key
            .cmp(a_key)
            .then_with(|| b.started_at.cmp(&a.started_at))
            .then_with(|| a.folder_name.cmp(&b.folder_name))
    });
    found.into_iter().map(|(_, summary)| summary).collect()
}

/// Find a recording by folder name or by recording-id prefix; the folder and
/// its manifest.
pub fn read_recording(
    root: &Path,
    name_or_id: &str,
) -> Result<(PathBuf, Manifest), RecordingError> {
    let dir = resolve(root, name_or_id, false)?;
    let manifest = load_manifest(&dir)?;
    Ok((dir, manifest))
}

/// The summary of the recording in `folder` (a folder [`read_recording`] or a
/// session returned), for a caller that holds the folder rather than the list.
pub fn summarize_folder(folder: &Path) -> Result<RecordingSummary, RecordingError> {
    let name = folder
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| invalid("a recording folder has a name"))?;
    let manifest = load_manifest(folder)?;
    Ok(summarize(&name, folder, &manifest))
}

/// The exclusive lock a caller holds while it removes a recording folder
/// (deleting it, moving it to the trash). While it exists no writer, no
/// recovery and no second remover can own the recording, so the folder is not
/// moved out from under a process that is still writing it. Dropping it
/// releases the lock.
#[derive(Debug)]
pub struct RemovalLock {
    folder: PathBuf,
    _file: fs::File,
}

impl RemovalLock {
    /// The locked recording folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }
}

/// Take the recording lock of `name_or_id` for a removal, without blocking.
///
/// Ownership has to be established, not assumed: a recording another process
/// is writing gives [`RecordingError::Locked`], and so does one being removed
/// or recovered; a lock file that cannot be opened or locked for any other
/// reason (a permission error, a link where the file belongs) fails with that
/// error and never counts as "nobody owns it". A recording whose lock file is
/// missing (finished and tidied by hand) gets one made, so the removal is
/// held to the same rule.
pub fn lock_recording_for_removal(
    root: &Path,
    name_or_id: &str,
) -> Result<RemovalLock, RecordingError> {
    let folder = resolve(root, name_or_id, true)?;
    let file = lock_folder(&folder, true)?;
    Ok(RemovalLock { folder, _file: file })
}

/// Remove a recording folder and everything in it. Refuses a folder that is
/// not directly under `root`, has no `recording.json`, or whose lock is not
/// ours ([`lock_recording_for_removal`]). The app moves folders to the trash
/// instead; this is the CLI's `delete --yes`.
///
/// The lock is held while the contents go, manifest first, so a recovery that
/// takes it afterwards finds no recording. The lock file is removed last,
/// after the lock is released (Windows cannot delete an open file).
pub fn delete_recording_folder(root: &Path, name_or_id: &str) -> Result<(), RecordingError> {
    let lock = lock_recording_for_removal(root, name_or_id)?;
    let dir = lock.folder.clone();
    remove_all_but_the_lock(&dir)?;
    drop(lock);
    // Only the lock file is left; with nothing to recover, a stray taker of
    // the lock finds an empty folder.
    fs::remove_file(dir.join(LOCK_FILE))
        .and_then(|()| fs::remove_dir(&dir))
        .map_err(|error| RecordingError::io("delete the recording", &error))
}

/// Remove every entry of `dir` but the lock file, the manifest first. Links
/// are removed, never followed.
fn remove_all_but_the_lock(dir: &Path) -> Result<(), RecordingError> {
    let io = |error: std::io::Error| RecordingError::io("delete the recording", &error);
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        let name = entry.map_err(io)?.file_name();
        if name != LOCK_FILE {
            names.push(name);
        }
    }
    names.sort_by_key(|name| name != MANIFEST_FILE);
    for name in names {
        let path = dir.join(&name);
        let metadata = fs::symlink_metadata(&path).map_err(io)?;
        if metadata.is_dir() {
            fs::remove_dir_all(&path).map_err(io)?;
        } else {
            fs::remove_file(&path).map_err(io)?;
        }
    }
    Ok(())
}

/// The FLAC file of track `track_id`, for playback and export.
pub fn audio_track_path(
    folder: &Path,
    manifest: &Manifest,
    track_id: &str,
) -> Result<PathBuf, RecordingError> {
    let track = manifest
        .track(track_id)
        .ok_or(RecordingError::UnknownTrack)?;
    let path = folder.join(&track.file);
    if !path.is_file() {
        return Err(RecordingError::Io {
            action: "find a track file",
            kind: std::io::ErrorKind::NotFound,
            os_code: None,
        });
    }
    Ok(path)
}

fn invalid(reason: &'static str) -> RecordingError {
    RecordingError::InvalidInput { reason }
}

/// The recording folder `name_or_id` names. With `by_name_needs_manifest`
/// the folder only has to hold a `recording.json` (deleting a recording whose
/// manifest has gone bad is allowed); otherwise the manifest must parse.
fn resolve(
    root: &Path,
    name_or_id: &str,
    by_name_needs_manifest: bool,
) -> Result<PathBuf, RecordingError> {
    if !is_plain_file_name(name_or_id) {
        return Err(invalid("a recording is named by one plain folder name"));
    }
    let direct = root.join(name_or_id);
    if let Ok(metadata) = fs::symlink_metadata(&direct) {
        if metadata.is_dir() && direct.join(MANIFEST_FILE).is_file() {
            if by_name_needs_manifest || load_manifest(&direct).is_ok() {
                return Ok(direct);
            }
            return Err(invalid("the folder holds no readable recording"));
        }
    }
    if name_or_id.chars().count() < MIN_ID_PREFIX {
        return Err(invalid("no such recording"));
    }
    let mut matches = Vec::new();
    let entries =
        fs::read_dir(root).map_err(|error| RecordingError::io("list the recordings", &error))?;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let dir = entry.path();
        if let Ok(manifest) = load_manifest(&dir) {
            if manifest.recording_id.starts_with(name_or_id) {
                matches.push(dir);
            }
        }
    }
    match matches.len() {
        0 => Err(invalid("no such recording")),
        1 => Ok(matches.remove(0)),
        _ => Err(invalid("the id matches more than one recording")),
    }
}

fn summarize(folder_name: &str, dir: &Path, manifest: &Manifest) -> RecordingSummary {
    let finished = manifest.state == RecordingState::Complete;
    let seconds = |frames: u64, rate: u32| frames as f64 / f64::from(rate.max(1));
    let duration_seconds = manifest
        .tracks
        .iter()
        .map(|track| seconds(track.frames, track.sample_rate))
        .fold(0.0, f64::max);
    let gaps = manifest.tracks.iter().flat_map(|track| {
        track
            .gaps
            .iter()
            .map(move |gap| seconds(gap.length, track.sample_rate))
    });
    let gap_count = gaps.clone().count();
    let gap_seconds = gaps.sum();
    let reasons: Vec<String> = manifest
        .tracks
        .iter()
        .filter_map(|track| track.ended_reason.clone())
        .map(String::from)
        .collect();
    let ended_reason = if finished {
        reasons
            .iter()
            .find(|reason| reason.as_str() != "stopped")
            .or_else(|| reasons.first())
            .cloned()
    } else {
        None
    };
    let first = manifest.tracks.first();
    RecordingSummary {
        folder_name: folder_name.to_string(),
        recording_id: manifest.recording_id.clone(),
        title: manifest.title.clone(),
        state: manifest.state,
        recovered: manifest.recovered,
        started_at: manifest.started_at.clone(),
        quality: manifest.quality,
        duration_seconds,
        bytes: folder_bytes(dir),
        track_count: manifest.tracks.len(),
        gap_count,
        gap_seconds,
        ended_reason,
        codec: first.map(|t| t.codec.clone()).unwrap_or_default(),
        sample_rate: first.map_or(0, |t| t.sample_rate),
        channels: first.map_or(0, |t| t.channels),
        needs_recovery: manifest.state == RecordingState::Recording && !is_owned(dir),
        playable: finished,
    }
}

/// Whether a live process holds the folder's lock.
fn is_owned(dir: &Path) -> bool {
    matches!(lock_folder(dir, false), Err(RecordingError::Locked))
}

/// The size of the files directly in the folder.
fn folder_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Seconds since the Unix epoch of an RFC 3339 time
/// (`2026-11-05T14:02:11.482+01:00`); `None` if it is not one. Only used to
/// order recordings made in different time zones.
fn epoch_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = text.get(range)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
    {
        return None;
    }
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
    }
    let offset = match bytes.get(at)? {
        b'Z' | b'z' if at + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if at + 6 == bytes.len() && bytes[at + 3] == b':' => {
            let seconds = number(at + 1..at + 3)? * 3600 + number(at + 4..at + 6)? * 60;
            if *sign == b'-' {
                -seconds
            } else {
                seconds
            }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
