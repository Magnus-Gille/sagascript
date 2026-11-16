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
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::RecordingError;
use super::io::{create_new_private, open_member, sync_file, Open};
use super::manifest::{Gap, GapReason};

/// The journal's file name for a track id.
pub fn journal_file_name(track_id: &str) -> String {
    format!("{track_id}.events.jsonl")
}

/// The longest line that is parsed. Our events are a few hundred bytes; a
/// longer line is skipped (and counted) without being held in memory.
const MAX_LINE_BYTES: usize = 64 * 1024;
/// Anchors and gaps one journal may hold. A manifest carries all of them and
/// is read back with a size limit, so a journal beyond this cannot be turned
/// into one: recovery then stops with
/// [`RecordingError::JournalTooLarge`] instead of finishing with events
/// missing. A day of anchors is under 10 000.
pub const MAX_JOURNAL_EVENTS: usize = 100_000;

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

/// A failure a test makes the next journal call have, once.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JournalFault {
    /// The append fails before a byte is written.
    AppendNothing,
    /// The append writes half of the line and fails, as a full disk can.
    AppendTorn,
    /// The append works; the sync fails.
    SyncFails,
}

/// Appends events to a track's journal.
pub struct JournalWriter {
    file: File,
    /// An append failed, so the file may end in half a line. The next append
    /// closes that line first, so it cannot swallow the next event.
    torn: bool,
    #[cfg(test)]
    pub(crate) fault: Option<JournalFault>,
}

impl JournalWriter {
    /// Create the journal (it must not exist yet), mode `0600` on Unix.
    pub fn create(path: &Path) -> Result<Self, RecordingError> {
        let file = create_new_private(path)
            .map_err(|error| RecordingError::io("create a track journal", &error))?;
        Ok(Self {
            file,
            torn: false,
            #[cfg(test)]
            fault: None,
        })
    }

    /// Append one event as a single write of a complete line.
    pub fn append(&mut self, event: &JournalEvent) -> Result<(), RecordingError> {
        let mut line = serde_json::to_vec(event).map_err(RecordingError::encoder)?;
        line.push(b'\n');
        #[cfg(test)]
        if let Some(fault @ (JournalFault::AppendNothing | JournalFault::AppendTorn)) = self.fault {
            self.fault = None;
            self.torn = true;
            if fault == JournalFault::AppendTorn {
                let _ = self.file.write_all(&line[..line.len() / 2]);
            }
            return Err(RecordingError::io(
                "append to a track journal",
                &std::io::Error::other("injected"),
            ));
        }
        // After a failed append the file may end in a fragment; a newline
        // first turns it into one ignorable line instead of the front of
        // this event. It goes out in the same write.
        if self.torn {
            line.insert(0, b'\n');
        }
        match self.file.write_all(&line) {
            Ok(()) => {
                self.torn = false;
                Ok(())
            }
            Err(error) => {
                self.torn = true;
                Err(RecordingError::io("append to a track journal", &error))
            }
        }
    }

    /// Flush the journal to stable storage.
    pub fn sync(&mut self) -> Result<(), RecordingError> {
        #[cfg(test)]
        if self.fault == Some(JournalFault::SyncFails) {
            self.fault = None;
            return Err(RecordingError::io(
                "sync a track journal",
                &std::io::Error::other("injected"),
            ));
        }
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
    /// `(frame, clipped)` for every anchor and gap: how many samples had been
    /// clamped before `frame` when the event was written.
    pub clip_marks: Vec<(u64, u64)>,
    /// Lines that were torn, too long or not understood.
    pub ignored_lines: usize,
}

impl JournalSummary {
    /// A lower bound for the samples clamped in the first `frames` frames:
    /// the largest count recorded by an event that lies inside them. An event
    /// past `frames` counted audio that is not there (it was lost in a
    /// crash), so it says nothing about the audio that is.
    pub fn clipped_within(&self, frames: u64) -> u64 {
        self.clip_marks
            .iter()
            .filter(|(frame, _)| *frame <= frames)
            .map(|(_, clipped)| *clipped)
            .max()
            .unwrap_or(0)
    }
}

/// Read a journal. A journal that does not exist is an empty summary (a crash
/// before the first line); other read failures are errors. The file is read
/// in a stream with bounded memory per line, to its end, whatever its size;
/// more events than [`MAX_JOURNAL_EVENTS`] is an error, never a short summary.
pub fn read_journal(path: &Path) -> Result<JournalSummary, RecordingError> {
    let file = match open_member(path, Open::Read, "open a track journal") {
        Ok(file) => file,
        Err(RecordingError::Io {
            kind: ErrorKind::NotFound,
            ..
        }) => return Ok(JournalSummary::default()),
        Err(error) => return Err(error),
    };
    read_events(BufReader::with_capacity(64 * 1024, file))
}

/// [`read_journal`] over bytes already in memory.
pub fn summarize(bytes: &[u8]) -> Result<JournalSummary, RecordingError> {
    read_events(bytes)
}

/// Fold the lines of a journal into a summary.
fn read_events<R: BufRead>(mut reader: R) -> Result<JournalSummary, RecordingError> {
    let mut folder = Folder::default();
    let mut line: Vec<u8> = Vec::new();
    // The current line has outgrown MAX_LINE_BYTES: the rest of it is skipped.
    let mut overlong = false;
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(RecordingError::io("read a track journal", &error)),
        };
        if chunk.is_empty() {
            break;
        }
        let (part, ends_line) = match chunk.iter().position(|b| *b == b'\n') {
            Some(newline) => (&chunk[..newline], true),
            None => (chunk, false),
        };
        if !overlong {
            if line.len() + part.len() > MAX_LINE_BYTES {
                overlong = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(part);
            }
        }
        let used = part.len() + usize::from(ends_line);
        reader.consume(used);
        if ends_line {
            if overlong {
                folder.summary.ignored_lines += 1;
            } else {
                folder.line(&line)?;
            }
            line.clear();
            overlong = false;
        }
    }
    // No newline at the end: the last write was torn.
    if overlong || !line.is_empty() {
        folder.summary.ignored_lines += 1;
    }
    Ok(folder.summary)
}

#[derive(Default)]
struct Folder {
    summary: JournalSummary,
    /// Anchors and gaps seen.
    events: usize,
}

impl Folder {
    fn count_event(&mut self) -> Result<(), RecordingError> {
        self.events += 1;
        if self.events > MAX_JOURNAL_EVENTS {
            return Err(RecordingError::JournalTooLarge);
        }
        Ok(())
    }

    /// One complete line, without its newline.
    fn line(&mut self, line: &[u8]) -> Result<(), RecordingError> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let summary = &mut self.summary;
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
                self.count_event()?;
                let summary = &mut self.summary;
                summary.anchors.push((host_ns, frame));
                summary.clip_marks.push((frame, clipped));
            }
            Ok(JournalEvent::Gap {
                at_frame,
                length,
                reason,
                clipped,
            }) => {
                self.count_event()?;
                let summary = &mut self.summary;
                summary.gaps.push(Gap::new(at_frame, length, reason));
                summary.clip_marks.push((at_frame, clipped));
            }
            Err(_) => summary.ignored_lines += 1,
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;
