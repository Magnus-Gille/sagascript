//! The append-only per-track journal `<track>.events.jsonl` (design §4.3, §4.5).
//!
//! The manifest of a recording in progress is written once, at the start. What
//! happens later and that recovery must be able to rebuild (when the track
//! started on the host clock, the clock anchors, every gap) goes to this file
//! the moment it happens, one JSON object per line:
//!
//! ```text
//! {"t":"start","domain":"mach_continuous","host_ns":123456789,"sample_rate":48000,"channels":1}
//! {"t":"anchor","host_ns":10123456789,"frame":480000,"clipped":0}
//! {"t":"gap","at_frame":1234567,"length":480,"reason":"overrun","clipped":0}
//! ```
//!
//! A line is one `write` of the complete object and its newline. A line that
//! does not end in a newline (a torn last write) or does not parse (an event
//! of a newer build) is skipped and counted, never fatal. The journal is kept
//! after the recording finishes.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::RecordingError;
use super::io::{create_new_private, sync_file};
use super::manifest::{Gap, GapReason};

/// The journal's file name for a track id.
pub fn journal_file_name(track_id: &str) -> String {
    format!("{track_id}.events.jsonl")
}

/// The most a journal is read from: a day of anchors is a few hundred kilobytes.
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

/// One journal line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum JournalEvent {
    /// The first frame of the track and the host time it was captured at.
    Start {
        domain: String,
        host_ns: u64,
        sample_rate: u32,
        channels: u8,
    },
    /// `frame` was captured at `host_ns`. `clipped` is the number of samples
    /// clamped so far, so recovery can report a lower bound.
    Anchor {
        host_ns: u64,
        frame: u64,
        #[serde(default)]
        clipped: u64,
    },
    /// Silence of `length` frames was inserted at stored frame `at_frame`.
    Gap {
        at_frame: u64,
        length: u64,
        reason: GapReason,
        #[serde(default)]
        clipped: u64,
    },
}

/// Appends events to a track's journal.
pub struct JournalWriter {
    file: File,
}

impl JournalWriter {
    /// Create the journal (it must not exist yet), mode `0600` on Unix.
    pub fn create(path: &Path) -> Result<Self, RecordingError> {
        let file = create_new_private(path)
            .map_err(|error| RecordingError::io("create a track journal", &error))?;
        Ok(Self { file })
    }

    /// Append one event as a single write of a complete line.
    pub fn append(&mut self, event: &JournalEvent) -> Result<(), RecordingError> {
        let mut line = serde_json::to_vec(event).map_err(RecordingError::encoder)?;
        line.push(b'\n');
        self.file
            .write_all(&line)
            .map_err(|error| RecordingError::io("append to a track journal", &error))
    }

    /// Flush the journal to stable storage.
    pub fn sync(&mut self) -> Result<(), RecordingError> {
        sync_file(&self.file).map_err(|error| RecordingError::io("sync a track journal", &error))
    }
}

/// What a journal says about a track.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalSummary {
    /// The clock domain and the host time of the first frame.
    pub start: Option<(String, u64)>,
    /// `(host_ns, frame_index)`, the start anchor first.
    pub anchors: Vec<(u64, u64)>,
    pub gaps: Vec<Gap>,
    /// The largest clipped-sample count any event recorded.
    pub clipped: u64,
    /// Lines that were torn or not understood.
    pub ignored_lines: usize,
}

/// Read a journal. A journal that does not exist is an empty summary (a crash
/// before the first line); other read failures are errors.
pub fn read_journal(path: &Path) -> Result<JournalSummary, RecordingError> {
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(file) => {
            file.take(MAX_JOURNAL_BYTES)
                .read_to_end(&mut bytes)
                .map_err(|error| RecordingError::io("read a track journal", &error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(JournalSummary::default());
        }
        Err(error) => return Err(RecordingError::io("open a track journal", &error)),
    }
    Ok(summarize(&bytes))
}

/// [`read_journal`] over bytes already in memory.
pub fn summarize(bytes: &[u8]) -> JournalSummary {
    let mut summary = JournalSummary::default();
    let mut rest = bytes;
    while !rest.is_empty() {
        let Some(newline) = rest.iter().position(|b| *b == b'\n') else {
            // No newline: the last write was torn.
            summary.ignored_lines += 1;
            break;
        };
        let line = &rest[..newline];
        rest = &rest[newline + 1..];
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<JournalEvent>(line) {
            Ok(JournalEvent::Start {
                domain, host_ns, ..
            }) => {
                if summary.start.is_none() {
                    summary.anchors.insert(0, (host_ns, 0));
                    summary.start = Some((domain, host_ns));
                }
            }
            Ok(JournalEvent::Anchor {
                host_ns,
                frame,
                clipped,
            }) => {
                summary.anchors.push((host_ns, frame));
                summary.clipped = summary.clipped.max(clipped);
            }
            Ok(JournalEvent::Gap {
                at_frame,
                length,
                reason,
                clipped,
            }) => {
                summary.gaps.push(Gap::new(at_frame, length, reason));
                summary.clipped = summary.clipped.max(clipped);
            }
            Err(_) => summary.ignored_lines += 1,
        }
    }
    summary
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
