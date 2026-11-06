//! Recordings for the tests of reading, listing and exporting: made by the
//! real [`RecordingSession`] over [`FakeSource`]s, so the folders are what
//! the recorder writes (gaps, offsets and all). No hardware, no real time.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::capture::{FakeHandle, FakeSource, NativeFormat};
use super::manifest::{Client, Quality};
use super::session::disk::FreeSpace;
use super::session::{RecordingSession, SessionConfig, SessionOutcome, SourceSpec};
use super::store::RecordingStore;

pub(crate) const MONO48: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};

struct PlentyOfRoom;

impl FreeSpace for PlentyOfRoom {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        Ok(100 << 30)
    }
}

/// One track of a made recording.
#[derive(Clone)]
pub(crate) struct Take {
    pub id: &'static str,
    pub format: NativeFormat,
    /// How much later than the first take this one's first block is stamped.
    pub starts_after: Duration,
}

impl Take {
    pub(crate) fn mic(format: NativeFormat) -> Self {
        Self {
            id: "mic",
            format,
            starts_after: Duration::ZERO,
        }
    }

    pub(crate) fn system(format: NativeFormat, starts_after: Duration) -> Self {
        Self {
            id: "system",
            format,
            starts_after,
        }
    }
}

pub(crate) struct Made {
    pub folder: PathBuf,
    pub outcome: SessionOutcome,
}

fn client() -> Client {
    Client {
        kind: "desktop".into(),
        app_version: "test".into(),
        build: "test".into(),
        extra: Default::default(),
    }
}

/// Wait until the writer has handled everything the sources delivered.
pub(crate) fn catch_up(session: &RecordingSession, handles: &[FakeHandle]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = session.status();
        let done = status.tracks.iter().zip(handles).all(|(track, handle)| {
            track.frames_received + track.dropped_frames
                == handle.frames_delivered() + handle.frames_dropped()
        });
        if done {
            return;
        }
        assert!(Instant::now() < deadline, "the writer did not catch up");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// `total` of audio on every handle, in interleaved steps so that no track's
/// clock runs far ahead of another's.
pub(crate) fn deliver_all(
    session: &RecordingSession,
    handles: &[FakeHandle],
    total: Duration,
    step: Duration,
) {
    let mut left = total;
    while !left.is_zero() {
        let now = left.min(step);
        for handle in handles {
            handle.deliver(now);
        }
        catch_up(session, handles);
        left -= now;
    }
}

/// Record `takes` into `store`; `script` plays the sources, then the session
/// is stopped.
pub(crate) fn record(
    store: &RecordingStore,
    title: &str,
    started_at: &str,
    quality: Quality,
    takes: &[Take],
    script: impl FnOnce(&RecordingSession, &[FakeHandle]),
) -> Made {
    let mut specs = Vec::new();
    let mut handles: Vec<FakeHandle> = Vec::new();
    for take in takes {
        let (source, handle) = FakeSource::new(take.format);
        handle.clock().advance(take.starts_after);
        let source = source.with_prime(Duration::from_millis(20));
        specs.push(SourceSpec {
            id: take.id.into(),
            role: take.id.into(),
            source: Box::new(source),
        });
        handles.push(handle);
    }
    let mut config = SessionConfig::new(title, started_at, client(), quality);
    config.clock = handles[0].clock();
    config.free_space = Arc::new(PlentyOfRoom);
    config.folder_warnings = Some(Vec::new());
    config.detect_sleep = false;
    config.start_timeout = Duration::from_millis(500);
    let session = RecordingSession::new(store.clone(), config);
    session.start(specs).expect("the session starts");
    catch_up(&session, &handles);
    script(&session, &handles);
    catch_up(&session, &handles);
    let outcome = session.stop().expect("the session stops");
    Made {
        folder: outcome.folder.clone(),
        outcome,
    }
}

/// A one-track (`mic`) recording of `seconds` of the fake sine, delivered in
/// 100 ms callbacks.
pub(crate) fn record_mic(
    store: &RecordingStore,
    title: &str,
    started_at: &str,
    quality: Quality,
    format: NativeFormat,
    seconds: f64,
) -> Made {
    record(
        store,
        title,
        started_at,
        quality,
        &[Take::mic(format)],
        |session, handles| {
            deliver_all(
                session,
                handles,
                Duration::from_secs_f64(seconds),
                Duration::from_millis(100),
            )
        },
    )
}
