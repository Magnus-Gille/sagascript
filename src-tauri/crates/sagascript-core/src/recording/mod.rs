//! The recorder's portable storage layer (SAGA-562, ADR 0010, design §4.4-§4.5).
//!
//! A recording is a **folder**: one FLAC file per track, a `recording.json`
//! manifest, an append-only journal per track and, while a process owns it, a
//! `recording.lock`. This module writes and recovers that folder. It has no
//! audio-device dependency and is compiled in every build.
//!
//! | Piece | What it is |
//! |---|---|
//! | [`manifest`] | `recording.json`: types, versioning, the migration rule |
//! | [`journal`] | `<track>.events.jsonl`: start, clock anchors, gaps |
//! | [`flac`] | the streaming [`FlacEncoder`] and the frame walker recovery uses |
//! | [`encoder`] | [`TrackEncoder`], the seam the writer thread drives |
//! | [`naming`] | folder names: sanitised title plus short id |
//! | [`store`] | creating a recording, the lock, the write and fsync order |
//! | [`recover`] | finishing recordings a process left behind |
//! | [`error`] | [`RecordingError`], with stable codes |
//!
//! # Crash model
//!
//! Every format and write order is chosen so that a kill at any point leaves a
//! playable file and enough metadata to finish. A **killed process** loses at
//! most the bytes still in the `BufWriter` of each track; a **power loss or OS
//! crash** loses at most what was written since the last fsync (ten seconds,
//! [`SYNC_INTERVAL`]). Recovery keeps exactly the fully valid frames and moves
//! anything after them to `<track>.tail`; it never deletes audio, and it
//! never touches a recording whose lock another process holds.
//!
//! # Concurrency
//!
//! The writer types are plain owned values driven by one thread (`&mut self`);
//! they hold no mutex, so there is nothing to add to the lock order of
//! `docs/ARCHITECTURE.md` §3. The one lock is the advisory file lock
//! `recording.lock`, taken non-blockingly, which keeps a second process (the
//! CLI while the app starts, a second app instance) from recovering a
//! recording that is still being written.

pub mod capture;
pub mod encoder;
pub mod error;
pub mod flac;
mod io;
pub mod journal;
pub mod manifest;
pub mod naming;
pub mod recover;
pub mod session;
pub mod store;
#[cfg(test)]
mod test_signals;

use std::time::Duration;

pub use encoder::{TrackEncoder, TrackSummary};
pub use error::RecordingError;
pub use flac::{FlacEncoder, TrackSink};
pub use manifest::{
    parse_manifest, serialize_manifest, Client, Clock, EndedReason, Gap, GapReason, Manifest,
    Quality, RecordingState, Track, MANIFEST_FILE, MIN_READER_VERSION, SCHEMA_VERSION,
};
pub use recover::{
    recover_folder, RecoveredRecording, RecoveryOutcome, RecoveryReport, TrackRecovery,
};
pub use session::{
    RecordingSession, SessionConfig, SessionOutcome, SessionState, SessionStatus, SessionWarning,
    SourceSpec, TrackStatus,
};
pub use store::{NewRecording, NewTrack, RecordingStore, RecordingWriter, LOCK_FILE};

/// The size of the header every track file starts with: STREAMINFO and the
/// reserved region that holds the SEEKTABLE and PADDING. Audio frames follow.
pub const TRACK_HEADER_LEN: usize = flac::HEADER_LEN;

/// How often tracks and journals are fsynced while recording, and at every gap.
pub const SYNC_INTERVAL: Duration = Duration::from_secs(10);
