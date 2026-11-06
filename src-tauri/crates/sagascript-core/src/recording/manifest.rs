//! `recording.json`: the description of a recording (ADR 0010, design §4.5).
//!
//! # Schema evolution (the rule, in the style of ADR 0007)
//!
//! * `schema_version` is the version of the shape this file was written in.
//!   [`SCHEMA_VERSION`] is the newest shape this build writes.
//! * `min_reader_version` is the lowest reader that may interpret the file. A
//!   writer raises it only for a change an older reader would misread (a field
//!   whose absence changes meaning, a new `state`); a field an older reader can
//!   safely ignore does **not** raise it. A reader refuses a manifest only when
//!   `min_reader_version` is above [`SCHEMA_VERSION`]
//!   ([`RecordingError::ManifestTooNew`]).
//! * **Unknown fields are ignored on read and survive a rewrite**: every
//!   object that can grow keeps what it did not understand in `extra` (the
//!   flattened remainder), so recovery by an older build never drops a field a
//!   newer build wrote. The versions of such a file are left as they were.
//! * **Migration.** An older manifest is brought forward by the ordered steps
//!   in [`MIGRATIONS`], one per version bump (`MIGRATIONS[n - 1]` upgrades
//!   shape `n` to `n + 1`), each idempotent and tested on its own. Version 1
//!   is the first shape, so the list is empty; a step is appended with the
//!   change that bumps [`SCHEMA_VERSION`], together with a fixture of the
//!   previous shape under `tests/fixtures/recording/`.
//! * Fields are only added and never renamed or reused. The golden file
//!   `tests/fixtures/recording/manifest-v1.json` is the schema test: changing
//!   the serialized shape of version 1 fails it.
//!
//! # What the file never holds
//!
//! File names are plain names relative to the recording folder; a name with a
//! separator, `..` or a drive prefix is refused when the manifest is read
//! (it is used to open files). `device` is a local detail (it can hold a
//! personal name) that [`Manifest::export_view`] leaves out.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::RecordingError;

/// The newest manifest shape this build writes.
pub const SCHEMA_VERSION: u32 = 1;
/// The lowest reader version a manifest written by this build requires.
pub const MIN_READER_VERSION: u32 = 1;
/// The manifest's file name inside a recording folder.
pub const MANIFEST_FILE: &str = "recording.json";

/// One migration step: upgrades the JSON of shape `n` to shape `n + 1`.
pub type Migration = fn(&mut Value);

/// The ordered migration steps; see the module documentation.
pub const MIGRATIONS: &[Migration] = &[];

/// Whether the recording is in progress or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    Recording,
    Complete,
}

/// The quality tier a recording was made in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// Native sample rate, 24-bit, never resampled.
    Lossless,
    /// 16 kHz / 16-bit, resampled by definition (`resampled_from` says so).
    Compact,
}

/// Why a track ended. Unknown values from a newer build are kept as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum EndedReason {
    Stopped,
    DeviceChanged,
    FormatChanged,
    SystemSleep,
    DiskFull,
    /// The process died; set by recovery.
    Interrupted,
    Other(String),
}

impl From<String> for EndedReason {
    fn from(value: String) -> Self {
        match value.as_str() {
            "stopped" => Self::Stopped,
            "device_changed" => Self::DeviceChanged,
            "format_changed" => Self::FormatChanged,
            "system_sleep" => Self::SystemSleep,
            "disk_full" => Self::DiskFull,
            "interrupted" => Self::Interrupted,
            _ => Self::Other(value),
        }
    }
}

impl From<EndedReason> for String {
    fn from(value: EndedReason) -> Self {
        match value {
            EndedReason::Stopped => "stopped".into(),
            EndedReason::DeviceChanged => "device_changed".into(),
            EndedReason::FormatChanged => "format_changed".into(),
            EndedReason::SystemSleep => "system_sleep".into(),
            EndedReason::DiskFull => "disk_full".into(),
            EndedReason::Interrupted => "interrupted".into(),
            EndedReason::Other(other) => other,
        }
    }
}

/// Why silence was inserted into a track. Unknown values are kept as written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum GapReason {
    /// The writer queue overflowed; the length comes from a dropped-frame count.
    Overrun,
    /// No block arrived for the watchdog period; the length comes from the anchors.
    Stall,
    /// The device disappeared for a while.
    DeviceLoss,
    Other(String),
}

impl From<String> for GapReason {
    fn from(value: String) -> Self {
        match value.as_str() {
            "overrun" => Self::Overrun,
            "stall" => Self::Stall,
            "device_loss" => Self::DeviceLoss,
            _ => Self::Other(value),
        }
    }
}

impl From<GapReason> for String {
    fn from(value: GapReason) -> Self {
        match value {
            GapReason::Overrun => "overrun".into(),
            GapReason::Stall => "stall".into(),
            GapReason::DeviceLoss => "device_loss".into(),
            GapReason::Other(other) => other,
        }
    }
}

/// Silence the writer inserted so the frame index still maps to time.
/// The silence occupies stored frames `at_frame .. at_frame + length`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub at_frame: u64,
    pub length: u64,
    pub reason: GapReason,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Gap {
    pub fn new(at_frame: u64, length: u64, reason: GapReason) -> Self {
        Self {
            at_frame,
            length,
            reason,
            extra: BTreeMap::new(),
        }
    }
}

/// The host clock a track was stamped with, and its anchors
/// (`[host_ns, frame_index]`), from which offset and drift are measured later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    /// `mach_continuous` (macOS), `qpc` (Windows), ... an open vocabulary.
    pub domain: String,
    pub start_ns: u64,
    pub anchors: Vec<(u64, u64)>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Clock {
    pub fn new(domain: impl Into<String>, start_ns: u64) -> Self {
        Self {
            domain: domain.into(),
            start_ns,
            anchors: vec![(start_ns, 0)],
            extra: BTreeMap::new(),
        }
    }
}

/// What wrote the recording. The vocabulary is SAGA-533's (`kind`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Client {
    /// `desktop`, later `phone`.
    pub kind: String,
    pub app_version: String,
    pub build: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One track: a single audio file and how it relates to time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// The role (`mic`); a later segment after a device change would be `mic.2`.
    pub id: String,
    /// `mic`, `system`, ... an open vocabulary.
    pub role: String,
    /// Plain file name inside the recording folder.
    pub file: String,
    pub codec: String,
    pub lossless: bool,
    pub sample_rate: u32,
    pub bits: u8,
    pub channels: u8,
    /// Sample frames (per channel), not interleaved samples.
    pub frames: u64,
    /// SHA-256 over the FLAC frame bytes after the metadata blocks; `null`
    /// while the track is being recorded.
    pub audio_sha256: Option<String>,
    pub clock: Clock,
    #[serde(default)]
    pub gaps: Vec<Gap>,
    /// `null` while the track is being recorded.
    pub ended_reason: Option<EndedReason>,
    /// Samples clamped when converted to integers. After recovery this is the
    /// last value the journal recorded, a lower bound.
    #[serde(default)]
    pub clipped_samples: u64,
    /// The native rate a compact track was resampled from.
    pub resampled_from: Option<u32>,
    /// The input device. Local detail: left out of exports and sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// `recording.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub min_reader_version: u32,
    pub recording_id: String,
    /// Free text; the folder name is a sanitised form of it.
    pub title: String,
    pub state: RecordingState,
    /// True when recovery finalised the recording.
    #[serde(default)]
    pub recovered: bool,
    /// RFC 3339 with the local offset, for example `2026-11-05T14:02:11.482+01:00`.
    pub started_at: String,
    pub client: Client,
    pub quality: Quality,
    pub tracks: Vec<Track>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Manifest {
    /// The copy that leaves a recording folder (an export, a sync): the local
    /// details of every track are removed.
    pub fn export_view(&self) -> Manifest {
        let mut copy = self.clone();
        for track in &mut copy.tracks {
            track.device = None;
        }
        copy
    }

    /// The track with this id.
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }
}

/// Whether `name` is a plain file name: it is used to open files in the
/// recording folder, so a separator, `..` or a drive prefix is refused.
pub(crate) fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 200
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
}

/// Whether `name` is one the recording keeps for itself: the manifest (and the
/// temporary file it is replaced through), the lock, a journal or a tail. A
/// track file named like one would have recovery write FLAC into it.
pub(crate) fn is_reserved_file_name(name: &str) -> bool {
    let tail_number = name.rsplit_once(".tail.").is_some_and(|(_, number)| {
        !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
    });
    name == MANIFEST_FILE
        || name == format!("{MANIFEST_FILE}.tmp")
        || name == super::store::LOCK_FILE
        || name.ends_with(".events.jsonl")
        || name.ends_with(".tail")
        || tail_number
}

/// A track id is also the stem of its journal file name.
pub(crate) fn is_valid_track_id(id: &str) -> bool {
    is_plain_file_name(id) && !id.ends_with('.') && !id.starts_with('.')
}

/// Check the parts of a manifest that are used to touch the filesystem or to
/// size buffers. Unknown values are not an error; unusable ones are.
pub fn validate(manifest: &Manifest) -> Result<(), RecordingError> {
    let invalid = |reason| Err(RecordingError::ManifestInvalid { reason });
    if manifest.recording_id.is_empty() {
        return invalid("recording_id is empty");
    }
    let mut ids = BTreeSet::new();
    let mut files = BTreeSet::new();
    for track in &manifest.tracks {
        if !is_valid_track_id(&track.id) {
            return invalid("a track id is not a plain name");
        }
        if !is_plain_file_name(&track.file) {
            return invalid("a track file is not a plain relative name");
        }
        if is_reserved_file_name(&track.file) {
            return invalid("a track file is named like a file the recording keeps for itself");
        }
        if !ids.insert(&track.id) || !files.insert(&track.file) {
            return invalid("a track id or file is used twice");
        }
        if track.sample_rate == 0 || track.channels == 0 || track.bits == 0 {
            return invalid("a track has no sample rate, channels or bit depth");
        }
    }
    Ok(())
}

/// Read a manifest: refuse one that needs a newer reader, bring an older one
/// forward, ignore (and keep) unknown fields, and validate the result.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, RecordingError> {
    let mut value: Value = serde_json::from_slice(bytes)
        .map_err(|_| RecordingError::ManifestInvalid { reason: "not JSON" })?;
    let object = value.as_object().ok_or(RecordingError::ManifestInvalid {
        reason: "not an object",
    })?;
    let schema_version = object
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v >= 1)
        .ok_or(RecordingError::ManifestInvalid {
            reason: "no schema_version",
        })?;
    let min_reader = object
        .get("min_reader_version")
        .map(|v| {
            v.as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or(RecordingError::ManifestInvalid {
                    reason: "min_reader_version is not a number",
                })
        })
        .transpose()?
        .unwrap_or(1);
    if min_reader > SCHEMA_VERSION {
        return Err(RecordingError::ManifestTooNew {
            required: min_reader,
            supported: SCHEMA_VERSION,
        });
    }
    migrate(&mut value, schema_version);
    let manifest: Manifest =
        serde_json::from_value(value).map_err(|_| RecordingError::ManifestInvalid {
            reason: "a required field is missing or has the wrong type",
        })?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Run the migration steps from `from` up to [`SCHEMA_VERSION`]. A manifest
/// written by a newer build (`from` above the current version) is left alone.
pub(crate) fn migrate(value: &mut Value, from: u32) {
    migrate_with(value, from, MIGRATIONS, SCHEMA_VERSION);
}

/// [`migrate`] with the steps and the target version given, so the mechanism
/// is tested while [`MIGRATIONS`] is still empty.
pub(crate) fn migrate_with(value: &mut Value, from: u32, steps: &[Migration], target: u32) {
    if from >= target {
        return;
    }
    for (index, step) in steps.iter().enumerate() {
        let step_from = index as u32 + 1;
        if step_from >= from && step_from < target {
            step(value);
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), target.into());
        let min_reader = object
            .get("min_reader_version")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1);
        object.insert("min_reader_version".into(), min_reader.into());
    }
}

/// The bytes of a manifest as written to disk: pretty JSON and a final newline.
pub fn serialize_manifest(manifest: &Manifest) -> Result<Vec<u8>, RecordingError> {
    let mut bytes =
        serde_json::to_vec_pretty(manifest).map_err(|_| RecordingError::ManifestInvalid {
            reason: "could not be serialised",
        })?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
