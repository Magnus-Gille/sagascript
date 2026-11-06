//! The session policy, one deterministic `step` at a time: no thread, no real
//! clock, no device. Every case of the design (§4.3) is driven through a
//! [`FakeSource`] and asserted in the manifest, the journal and the decoded
//! audio.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sagascript_testsupport::TempDir;

use super::core::{CoreSettings, PipelineSpec, SessionCore};
use super::disk::{DiskPolicy, FreeSpace};
use super::status::{StatusShared, TrackCounters};
use crate::recording::capture::{
    track_channel, CaptureSource, ChannelConfig, FakeHandle, FakeSignal, FakeSource, ManualClock,
    NativeFormat, SessionClock, SleepDetector,
};
use crate::recording::encoder::{quantise, TrackEncoder};
use crate::recording::flac::{FlacEncoder, TrackSink};
use crate::recording::io::{create_new_private, sync_file};
use crate::recording::journal::{journal_file_name, read_journal};
use crate::recording::manifest::{
    parse_manifest, EndedReason, GapReason, Manifest, Quality, RecordingState,
};
use crate::recording::session::status::SessionOutcome;
use crate::recording::store::{NewRecording, NewTrack};
use crate::recording::test_signals::{decode_flac, recording, track};
use crate::recording::{RecordingError, RecordingStore, TRACK_HEADER_LEN};

const MONO48: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};
const STEREO44: NativeFormat = NativeFormat {
    sample_rate: 44_100,
    channels: 2,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---- the rig ---------------------------------------------------------------

struct FreeBytes(AtomicU64);

impl FreeSpace for FreeBytes {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn settings() -> CoreSettings {
    CoreSettings {
        stall_timeout_ns: 500_000_000,
        min_gap_ns: 100_000_000,
        max_gap_fill_ns: 60_000_000_000,
        anchor_interval_ns: 10_000_000_000,
        device_loss_grace_ns: 10_000_000_000,
        disk: DiskPolicy {
            low_bytes: 1 << 30,
            critical_bytes: 1 << 20,
            poll_interval: Duration::from_secs(5),
        },
        file_limit_bytes: None,
    }
}

struct Opts {
    format: NativeFormat,
    quality: Quality,
    block_frames: usize,
    blocks: usize,
    prime: Duration,
    settings: CoreSettings,
    /// Fail like a full disk after this many bytes of the track file.
    disk_full_after: Option<u64>,
    /// What the source plays, from its first (priming) block on.
    signal: FakeSignal,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            format: MONO48,
            quality: Quality::Lossless,
            block_frames: 1024,
            blocks: 64,
            prime: ms(20),
            settings: settings(),
            disk_full_after: None,
            signal: FakeSignal::Noise {
                amplitude: 0.5,
                seed: 7,
            },
        }
    }
}

/// A track file that stops accepting bytes like a full disk.
struct FailingFile {
    file: File,
    budget: u64,
}

impl Write for FailingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.budget == 0 {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        let take = buf.len().min(self.budget as usize);
        let written = self.file.write(&buf[..take])?;
        self.budget -= written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for FailingFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

impl TrackSink for FailingFile {
    fn sync_data(&mut self) -> io::Result<()> {
        sync_file(&self.file)
    }
}

struct Rig {
    _temp: TempDir,
    root: PathBuf,
    handle: FakeHandle,
    source: FakeSource,
    core: SessionCore,
    status: Arc<StatusShared>,
    free: Arc<FreeBytes>,
    quality: Quality,
    format: NativeFormat,
}

impl Rig {
    fn new(opts: Opts) -> Self {
        let temp = TempDir::new();
        let root = temp.path().join("Recordings");
        let clock = Arc::new(ManualClock::new());
        let (mut source, handle) = FakeSource::with_clock(opts.format, Arc::clone(&clock));
        source = source.with_prime(opts.prime);
        handle.set_signal(opts.signal.clone());
        let (sink, mut receiver) = track_channel(
            opts.format,
            ChannelConfig {
                block_frames: opts.block_frames,
                blocks: opts.blocks,
            },
        );
        source.start(sink).unwrap();
        let first = receiver.pop().expect("the prime is the first block");

        let compact = opts.quality == Quality::Compact;
        let (rate, channels, bits) = if compact {
            (16_000, 1, 16)
        } else {
            (opts.format.sample_rate, opts.format.channels, 24)
        };
        let mut new_track = track("mic", rate, channels, bits);
        new_track.start_ns = first.host_ns();
        new_track.resampled_from =
            (compact && opts.format.sample_rate != 16_000).then_some(opts.format.sample_rate);
        let mut request: NewRecording = recording("Session test", vec![new_track]);
        request.quality = opts.quality;

        let store = RecordingStore::new(&root);
        let writer = match opts.disk_full_after {
            None => store.create(request).unwrap(),
            Some(budget) => store
                .create_with(
                    request,
                    &mut || uuid::Uuid::new_v4().to_string(),
                    &mut move |path: &Path, new: &NewTrack| {
                        let file = create_new_private(path)
                            .map_err(|e| RecordingError::io("create a track file", &e))?;
                        let sink = FailingFile { file, budget };
                        Ok(Box::new(FlacEncoder::new(
                            sink,
                            new.sample_rate,
                            new.channels,
                            new.bits,
                        )?) as Box<dyn TrackEncoder>)
                    },
                )
                .unwrap(),
        };

        let status = Arc::new(StatusShared::new());
        let _ = status.tracks.set(vec![TrackCounters::new("mic", "mic")]);
        let free = Arc::new(FreeBytes(AtomicU64::new(100 << 30)));
        let core = SessionCore::new(
            writer,
            vec![PipelineSpec {
                id: "mic".into(),
                receiver,
                native: opts.format,
                compact,
                first_block: first,
            }],
            opts.settings,
            clock,
            Arc::clone(&status),
            Arc::clone(&free) as Arc<dyn FreeSpace>,
            None::<SleepDetector>,
        )
        .unwrap();
        Self {
            _temp: temp,
            root,
            handle,
            source,
            core,
            status,
            free,
            quality: opts.quality,
            format: opts.format,
        }
    }

    fn mono() -> Self {
        Self::new(Opts::default())
    }

    /// Step until the queues are empty.
    fn pump(&mut self) {
        for _ in 0..10_000 {
            if !self.core.step(false).unwrap() {
                return;
            }
        }
        panic!("the core never went idle");
    }

    fn track_status(&self) -> super::status::TrackStatus {
        self.status.snapshot(0).tracks.remove(0)
    }

    /// Stop the source and finish.
    fn finish(mut self) -> Finished {
        self.pump();
        self.source.stop();
        for _ in 0..10_000 {
            self.core.step(true).unwrap();
            if self.core.is_finished() {
                break;
            }
        }
        assert!(self.core.is_finished(), "the core did not finish");
        let outcome = self.core.take_outcome().unwrap();
        Finished {
            _temp: self._temp,
            handle: self.handle,
            outcome,
            quality: self.quality,
            format: self.format,
        }
    }

    /// The session ended without a stop: the last track ended on its own.
    fn finished_by_itself(mut self) -> Finished {
        self.pump();
        assert!(self.core.is_finished(), "the core did not finish by itself");
        let outcome = self.core.take_outcome().unwrap();
        Finished {
            _temp: self._temp,
            handle: self.handle,
            outcome,
            quality: self.quality,
            format: self.format,
        }
    }
}

struct Finished {
    _temp: TempDir,
    handle: FakeHandle,
    outcome: SessionOutcome,
    quality: Quality,
    format: NativeFormat,
}

impl Finished {
    fn manifest(&self) -> &Manifest {
        &self.outcome.manifest
    }

    fn track(&self) -> &crate::recording::Track {
        &self.outcome.manifest.tracks[0]
    }

    fn gaps(&self) -> Vec<(u64, u64, GapReason)> {
        self.track()
            .gaps
            .iter()
            .map(|g| (g.at_frame, g.length, g.reason.clone()))
            .collect()
    }

    fn file(&self) -> Vec<u8> {
        std::fs::read(self.outcome.folder.join("mic.flac")).unwrap()
    }

    fn on_disk(&self) -> Manifest {
        parse_manifest(&std::fs::read(self.outcome.folder.join("recording.json")).unwrap()).unwrap()
    }

    /// The audio the track should hold for stored frames `0..frames`: the
    /// source's signal, with silence where `gaps` say the writer inserted it.
    /// Lossless only.
    fn expected_samples(&self, frames: u64, gaps: &[(u64, u64)]) -> Vec<i32> {
        let channels = usize::from(self.format.channels);
        let mut want: Vec<i32> = self
            .handle
            .expected(0, frames)
            .iter()
            .map(|s| quantise(*s, 24).0)
            .collect();
        for (at, length) in gaps {
            let from = *at as usize * channels;
            let to = ((at + length) as usize * channels).min(want.len());
            want[from..to].fill(0);
        }
        want
    }

    /// Decode the track and require it to be the source's signal with
    /// `gaps` as silence, over exactly the frames the manifest claims.
    fn assert_audio(&self, gaps: &[(u64, u64)]) {
        assert_eq!(self.quality, Quality::Lossless);
        let decoded = decode_flac(&self.file());
        assert_eq!(decoded.errors, 0);
        assert_eq!(decoded.md5_verified, Some(true));
        let frames = self.track().frames;
        assert_eq!(
            decoded.samples.len() as u64,
            frames * u64::from(self.format.channels)
        );
        assert!(
            decoded.samples == self.expected_samples(frames, gaps),
            "the stored audio is not the source's signal with the gaps as silence"
        );
    }

    fn journal(&self) -> crate::recording::journal::JournalSummary {
        read_journal(&self.outcome.folder.join(journal_file_name("mic"))).unwrap()
    }
}

// ---- the plain recording -----------------------------------------------------

#[test]
fn a_recording_is_the_signal_bit_exact_with_no_gaps() {
    let mut rig = Rig::mono();
    for _ in 0..200 {
        rig.handle.deliver(ms(10));
        rig.pump();
    }
    let done = rig.finish();
    assert_eq!(done.manifest().state, RecordingState::Complete);
    assert_eq!(done.on_disk(), *done.manifest());
    assert_eq!(done.track().frames, done.handle.position());
    assert_eq!(done.track().frames, 960 + 200 * 480);
    assert_eq!(done.track().ended_reason, Some(EndedReason::Stopped));
    assert!(done.track().gaps.is_empty());
    assert_eq!(done.outcome.ended_early, None);
    done.assert_audio(&[]);
    assert!(!done.outcome.folder.join("recording.lock").exists());
}

#[test]
fn a_stereo_44100_recording_keeps_both_channels() {
    let mut rig = Rig::new(Opts {
        format: STEREO44,
        ..Opts::default()
    });
    for _ in 0..50 {
        rig.handle.deliver(ms(20));
        rig.pump();
    }
    let done = rig.finish();
    assert_eq!(done.track().channels, 2);
    assert_eq!(done.track().sample_rate, 44_100);
    assert_eq!(done.track().bits, 24);
    done.assert_audio(&[]);
}

#[test]
fn a_callback_larger_than_a_block_is_stored_whole() {
    let mut rig = Rig::new(Opts {
        block_frames: 100,
        blocks: 64,
        ..Opts::default()
    });
    rig.handle.deliver(ms(100)); // 4800 frames in 100-frame blocks
    rig.pump();
    let done = rig.finish();
    assert!(done.track().gaps.is_empty());
    done.assert_audio(&[]);
}

// ---- gaps -------------------------------------------------------------------

#[test]
fn an_injected_overrun_becomes_a_gap_of_the_dropped_length() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    rig.handle.overrun(480);
    rig.handle.deliver(ms(100));
    rig.pump();
    let before = 960 + 4800;
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(before, 480, GapReason::Overrun)]);
    assert_eq!(done.track().frames, done.handle.position());
    done.assert_audio(&[(before, 480)]);
    // The journal has it too, written when it happened.
    assert_eq!(done.journal().gaps.len(), 1);
    assert_eq!(done.journal().gaps[0].length, 480);
}

#[test]
fn a_writer_that_falls_behind_drops_frames_and_the_gap_is_exactly_what_was_dropped() {
    // A pool of four 64-frame blocks and a prime that fits in one.
    let mut rig = Rig::new(Opts {
        block_frames: 64,
        blocks: 4,
        prime: Duration::from_micros(500), // 24 frames
        ..Opts::default()
    });
    for _ in 0..6 {
        rig.handle.deliver_frames(64); // the last two find the pool empty
    }
    assert_eq!(rig.handle.position(), 24 + 6 * 64);
    rig.pump();
    rig.handle.deliver_frames(64);
    rig.pump();
    let status = rig.track_status();
    assert_eq!(status.dropped_frames, 128);
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(24 + 4 * 64, 128, GapReason::Overrun)]);
    assert_eq!(done.track().frames, done.handle.position());
    done.assert_audio(&[(24 + 4 * 64, 128)]);
}

#[test]
fn a_stall_trips_the_watchdog_and_the_gap_length_comes_from_the_stamps() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    assert!(!rig.track_status().stalled);

    rig.handle.stall(ms(400));
    rig.pump();
    assert!(
        !rig.track_status().stalled,
        "400 ms is under the 500 ms watchdog"
    );
    rig.handle.stall(ms(300)); // 700 ms in all
    rig.pump();
    assert!(rig.track_status().stalled, "the watchdog trips at 500 ms");

    rig.handle.deliver(ms(100));
    rig.pump();
    assert!(!rig.track_status().stalled, "audio is flowing again");
    let at = 960 + 4800;
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(at, 33_600, GapReason::Stall)]);
    assert_eq!(done.track().frames, done.handle.position());
    done.assert_audio(&[(at, 33_600)]);
}

#[test]
fn a_late_block_under_the_minimum_is_not_a_gap() {
    let rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.handle.idle(ms(60));
    rig.handle.deliver(ms(100));
    let done = rig.finish();
    assert!(done.track().gaps.is_empty());
}

#[test]
fn audio_that_resumes_after_a_lost_device_is_a_device_loss_gap() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    rig.handle.lose_device();
    rig.handle.stall(ms(2_000));
    rig.pump();
    assert!(rig.track_status().device_lost);
    rig.handle.deliver(ms(100));
    rig.pump();
    assert!(!rig.track_status().device_lost);
    let at = 960 + 4800;
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(at, 96_000, GapReason::DeviceLoss)]);
    done.assert_audio(&[(at, 96_000)]);
}

#[test]
fn a_gap_is_anchored_to_the_clock_right_after_it() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    rig.handle.stall(ms(1_000));
    rig.handle.deliver(ms(100));
    rig.pump();
    let done = rig.finish();
    let resumed_at = 960 + 4800 + 48_000;
    let anchors = &done.track().clock.anchors;
    assert_eq!(anchors.len(), 2, "{anchors:?}");
    assert_eq!(anchors[0].1, 0);
    // The block after the gap: stored frame `resumed_at`, captured 1.12 s
    // (20 + 100 ms of audio and 1 s of silence) after the start.
    assert_eq!(anchors[1].1, resumed_at);
    assert_eq!(anchors[1].0 - anchors[0].0, 1_120_000_000);
    assert_eq!(done.journal().anchors, *anchors);
}

#[test]
fn anchors_are_written_every_ten_seconds_of_host_time() {
    let mut rig = Rig::mono();
    for _ in 0..(25 * 10) {
        rig.handle.deliver(ms(100));
        rig.pump();
    }
    let done = rig.finish();
    let anchors = &done.track().clock.anchors;
    // Frame 0, then the first block at or past each ten seconds.
    assert!(anchors.len() >= 3, "{anchors:?}");
    for pair in anchors.windows(2) {
        let seconds = (pair[1].0 - pair[0].0) as f64 / 1e9;
        assert!((10.0..10.2).contains(&seconds), "{seconds}");
        // The frame index maps to the host clock: 48 000 frames a second.
        let frames = (pair[1].1 - pair[0].1) as f64;
        assert!((frames / 48_000.0 - seconds).abs() < 0.002);
    }
    assert_eq!(done.journal().anchors.len(), anchors.len());
}

#[test]
fn the_ten_second_fsync_hook_runs_on_the_session_clock() {
    let mut rig = Rig::mono();
    for _ in 0..(35 * 10) {
        rig.handle.deliver(ms(100));
        rig.pump();
    }
    // 35 s of virtual time: the hook has fired three times, not once a block.
    assert_eq!(rig.core.syncs(), 3);
}

// ---- endings ----------------------------------------------------------------

fn ended_by(reason: EndedReason, act: impl FnOnce(&mut Rig)) -> Finished {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(200));
    rig.pump();
    act(&mut rig);
    let done = rig.finished_by_itself();
    assert_eq!(done.track().ended_reason, Some(reason.clone()));
    assert_eq!(done.on_disk().tracks[0].ended_reason, Some(reason.clone()));
    assert_eq!(done.on_disk().state, RecordingState::Complete);
    assert_eq!(done.outcome.ended_early, Some(reason));
    done
}

#[test]
fn a_device_change_ends_the_track_and_keeps_every_block_before_it() {
    let done = ended_by(EndedReason::DeviceChanged, |rig| {
        rig.handle.deliver(ms(100)); // queued before the event
        rig.handle.change_device();
    });
    assert_eq!(done.track().frames, 960 + 9_600 + 4_800);
    done.assert_audio(&[]);
}

#[test]
fn a_format_change_ends_the_track() {
    let done = ended_by(EndedReason::FormatChanged, |rig| {
        rig.handle.change_format(NativeFormat {
            sample_rate: 44_100,
            channels: 1,
        });
    });
    assert_eq!(
        done.track().sample_rate,
        48_000,
        "the file keeps its format"
    );
    done.assert_audio(&[]);
}

#[test]
fn a_channel_count_change_ends_the_track_too() {
    ended_by(EndedReason::FormatChanged, |rig| {
        rig.handle.change_format(NativeFormat {
            sample_rate: 48_000,
            channels: 2,
        });
    });
}

#[test]
fn the_same_format_reported_again_is_not_a_change() {
    let rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.handle.change_format(MONO48);
    rig.handle.deliver(ms(100));
    let done = rig.finish();
    assert_eq!(done.track().ended_reason, Some(EndedReason::Stopped));
}

#[test]
fn a_system_sleep_ends_the_track_and_does_not_write_hours_of_silence() {
    let done = ended_by(EndedReason::SystemSleep, |rig| {
        rig.handle.sleep(Duration::from_secs(3 * 3600));
        rig.handle.deliver(ms(100)); // after the wake
    });
    // Only what was recorded before the sleep.
    assert_eq!(done.track().frames, 960 + 9_600);
    assert!(done.track().gaps.is_empty());
    assert!(done.file().len() < 100_000, "{} bytes", done.file().len());
    done.assert_audio(&[]);
}

#[test]
fn a_clock_jump_nobody_explained_ends_the_track_and_is_not_filled() {
    let done = ended_by(EndedReason::Other("clock_discontinuity".into()), |rig| {
        rig.handle.stall(Duration::from_secs(120));
        rig.handle.deliver(ms(100));
    });
    assert_eq!(done.track().frames, 960 + 9_600);
}

#[test]
fn a_device_that_reports_a_problem_and_never_returns_is_a_device_change() {
    ended_by(EndedReason::DeviceChanged, |rig| {
        rig.handle.lose_device();
        rig.pump(); // the writer sees the report
        rig.handle.idle(Duration::from_secs(11));
        rig.core.step(false).unwrap();
    });
}

#[test]
fn the_other_endings_do_not_depend_on_the_order_of_events() {
    // A device change and a sleep together: the more specific cause wins.
    ended_by(EndedReason::SystemSleep, |rig| {
        rig.handle.change_device();
        rig.handle.sleep(Duration::from_secs(60));
    });
}

// ---- stopping ---------------------------------------------------------------

#[test]
fn frames_dropped_after_the_last_block_are_a_gap_at_the_end() {
    let rig = Rig::new(Opts {
        block_frames: 64,
        blocks: 4,
        prime: Duration::from_micros(500),
        ..Opts::default()
    });
    for _ in 0..6 {
        rig.handle.deliver_frames(64);
    }
    // No block gets through after the pool filled: the last 128 frames are
    // reported when the stream goes.
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(24 + 4 * 64, 128, GapReason::Overrun)]);
    assert_eq!(done.track().frames, done.handle.position());
}

#[test]
fn a_stall_that_is_still_going_when_the_user_stops_is_a_gap_too() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    rig.handle.stall(Duration::from_secs(3));
    let done = rig.finish();
    assert_eq!(done.gaps(), vec![(960 + 4800, 144_000, GapReason::Stall)]);
    assert_eq!(done.track().frames, done.handle.position());
}

#[test]
fn stopping_straight_after_the_last_block_adds_nothing() {
    let rig = Rig::mono();
    rig.handle.deliver(ms(100));
    let done = rig.finish();
    assert!(done.track().gaps.is_empty());
    assert_eq!(done.track().frames, 960 + 4800);
}

// ---- disk -------------------------------------------------------------------

#[test]
fn a_write_error_from_a_full_disk_cuts_the_track_to_its_last_complete_frame() {
    let mut rig = Rig::new(Opts {
        disk_full_after: Some(TRACK_HEADER_LEN as u64 + 20_000),
        ..Opts::default()
    });
    rig.handle.deliver(ms(20));
    rig.handle.overrun(480); // a gap early on survives the cut
    for _ in 0..200 {
        rig.handle.deliver(ms(10));
        rig.pump();
    }
    let status = rig.track_status();
    assert_eq!(status.ended, Some(EndedReason::DiskFull));
    let done = rig.finished_by_itself();

    assert_eq!(done.track().ended_reason, Some(EndedReason::DiskFull));
    assert_eq!(done.outcome.ended_early, Some(EndedReason::DiskFull));
    assert_eq!(done.manifest().state, RecordingState::Complete);
    assert_eq!(done.on_disk().state, RecordingState::Complete);
    let frames = done.track().frames;
    assert!(
        frames > 0 && frames.is_multiple_of(4096),
        "{frames}: whole blocks only"
    );
    assert!(frames < done.handle.position());
    // The file is valid up to the cut: STREAMINFO matches, MD5 verifies, and
    // the audio is the signal (with the gap as silence) up to there.
    let at = 960 + 960;
    done.assert_audio(&[(at, 480)]);
    assert_eq!(done.gaps(), vec![(at, 480, GapReason::Overrun)]);
    // Nothing was kept as a tail: there was no space for one.
    assert!(!done.outcome.folder.join("mic.tail").exists());
    assert!(!done.outcome.folder.join("recording.lock").exists());
}

#[test]
fn a_full_disk_while_the_last_block_is_written_at_stop_is_cut_back_too() {
    // Room for the header and the first frames only; the stop flushes the rest.
    let rig = Rig::new(Opts {
        disk_full_after: Some(TRACK_HEADER_LEN as u64 + 14_000),
        ..Opts::default()
    });
    rig.handle.deliver(ms(250));
    let done = rig.finish();
    assert_eq!(done.track().ended_reason, Some(EndedReason::DiskFull));
    assert_eq!(done.manifest().state, RecordingState::Complete);
    assert!(done.track().frames.is_multiple_of(4096));
    done.assert_audio(&[]);
}

#[test]
fn critical_free_space_stops_and_finalizes_cleanly() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    assert_eq!(rig.status.snapshot(0).free_disk_bytes, Some(100 << 30));
    rig.free.0.store(512 * 1024, Ordering::SeqCst); // under the 1 MiB critical level
    rig.handle.deliver(ms(100));
    rig.handle.idle(Duration::from_secs(6)); // past the poll interval
    let done = rig.finished_by_itself();
    assert_eq!(done.track().ended_reason, Some(EndedReason::DiskFull));
    assert_eq!(done.manifest().state, RecordingState::Complete);
    // A clean stop keeps every frame that was delivered.
    assert_eq!(done.track().frames, 960 + 4800 + 4800);
    done.assert_audio(&[]);
}

#[test]
fn low_free_space_warns_and_keeps_recording() {
    let mut rig = Rig::mono();
    rig.free.0.store(512 << 20, Ordering::SeqCst); // under 1 GiB, over 1 MiB
    rig.handle.deliver(ms(100));
    rig.handle.idle(Duration::from_secs(6));
    rig.pump();
    assert!(rig
        .status
        .snapshot(0)
        .warnings
        .contains(&super::SessionWarning::LowDisk));
    rig.handle.deliver(ms(100));
    let done = rig.finish();
    assert_eq!(done.track().ended_reason, Some(EndedReason::Stopped));
}

#[test]
fn a_fat32_track_ends_before_the_file_size_limit() {
    let mut s = settings();
    s.file_limit_bytes = Some(TRACK_HEADER_LEN as u64 + 12_000);
    let mut rig = Rig::new(Opts {
        settings: s,
        ..Opts::default()
    });
    for _ in 0..100 {
        rig.handle.deliver(ms(20));
        rig.pump();
    }
    let done = rig.finished_by_itself();
    assert_eq!(
        done.track().ended_reason,
        Some(EndedReason::Other("file_size_limit".into()))
    );
    assert_eq!(done.manifest().state, RecordingState::Complete);
    let size = done.file().len() as u64;
    assert!(size < TRACK_HEADER_LEN as u64 + 12_000 + 20_000, "{size}");
    done.assert_audio(&[]);
}

// ---- the compact tier -------------------------------------------------------

fn compact(format: NativeFormat) -> Rig {
    compact_with(
        format,
        FakeSignal::Sine {
            hz: 440.0,
            amplitude: 0.3,
        },
    )
}

fn compact_with(format: NativeFormat, signal: FakeSignal) -> Rig {
    Rig::new(Opts {
        format,
        quality: Quality::Compact,
        signal,
        ..Opts::default()
    })
}

fn decode16(done: &Finished) -> Vec<i32> {
    let decoded = decode_flac(&done.file());
    assert_eq!(decoded.errors, 0);
    assert_eq!(decoded.md5_verified, Some(true));
    assert_eq!(decoded.sample_rate, 16_000);
    assert_eq!(decoded.channels, 1);
    decoded.samples
}

#[test]
fn the_compact_tier_stores_16_kilohertz_16_bit_mono_and_says_it_was_resampled() {
    let mut rig = compact(MONO48);
    for _ in 0..100 {
        rig.handle.deliver(ms(20));
        rig.pump();
    }
    let done = rig.finish();
    let track = done.track();
    assert_eq!(done.manifest().quality, Quality::Compact);
    assert_eq!(
        (track.sample_rate, track.bits, track.channels),
        (16_000, 16, 1)
    );
    assert_eq!(track.resampled_from, Some(48_000));
    assert!(!track.lossless);
    // Exactly the frames the time implies: (20 ms + 2 s) at 16 kHz.
    assert_eq!(
        track.frames,
        (done.handle.position() as f64 / 3.0).round() as u64
    );
    let samples = decode16(&done);
    assert_eq!(samples.len() as u64, track.frames);
}

#[test]
fn the_resampler_delay_is_compensated_so_the_track_keeps_the_source_time() {
    // One impulse, 1.0 s after the first block: input frame 960 + 48 000.
    let mut fixture = vec![0.0f32; 960 + 48_000 + 4_800 + 24_000];
    fixture[960 + 48_000] = 0.9;
    let mut rig = compact_with(MONO48, FakeSignal::Fixture(Arc::new(fixture)));
    rig.handle.deliver(ms(1_000));
    rig.pump();
    rig.handle.deliver_frames(4_800);
    rig.pump();
    rig.handle.deliver(ms(500));
    let done = rig.finish();
    let samples = decode16(&done);
    let (peak_at, peak) = samples
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| s.abs())
        .unwrap();
    // Stored frame (960 + 48 000) / 3 = 16 320.
    assert!(
        (peak_at as i64 - 16_320).abs() <= 1,
        "the impulse landed at stored frame {peak_at}"
    );
    assert!(*peak > 3_000, "{peak}");
}

#[test]
fn compact_gaps_and_anchors_are_in_stored_track_frames() {
    let mut rig = compact(MONO48);
    rig.handle.deliver(ms(200)); // 9 600 native frames + the 960 of the prime
    rig.pump();
    rig.handle.overrun(4_800); // 100 ms
    rig.handle.deliver(ms(200));
    rig.pump();
    let done = rig.finish();
    // 10 560 native frames before the gap are 3 520 stored frames.
    assert_eq!(done.gaps(), vec![(3_520, 1_600, GapReason::Overrun)]);
    let anchors = &done.track().clock.anchors;
    assert_eq!(anchors.len(), 2, "{anchors:?}");
    assert_eq!(
        anchors[1].1,
        3_520 + 1_600,
        "the first stored frame after the gap"
    );
    assert_eq!(done.track().frames, 3_520 + 1_600 + 3_200);
    let samples = decode16(&done);
    assert!(samples[3_520..3_520 + 1_600].iter().all(|s| *s == 0));
}

#[test]
fn compact_stereo_44100_is_downmixed_to_mono_16k() {
    let mut rig = compact(STEREO44);
    for _ in 0..50 {
        rig.handle.deliver(ms(20));
        rig.pump();
    }
    let done = rig.finish();
    assert_eq!(done.track().resampled_from, Some(44_100));
    let samples = decode16(&done);
    let peak = samples.iter().map(|s| s.abs()).max().unwrap();
    // Both channels carry the same tone, so the mean has its amplitude.
    assert!((8_000..11_000).contains(&peak), "{peak}");
}

#[test]
fn a_compact_track_that_ends_on_a_device_change_flushes_the_resampler_first() {
    let rig = compact(MONO48);
    rig.handle.deliver(ms(500));
    rig.handle.change_device();
    let done = rig.finished_by_itself();
    assert_eq!(done.track().ended_reason, Some(EndedReason::DeviceChanged));
    assert_eq!(
        done.track().frames,
        (done.handle.position() as f64 / 3.0).round() as u64
    );
}

// ---- misc -------------------------------------------------------------------

#[test]
fn the_status_reports_peaks_counts_and_bytes() {
    let mut rig = Rig::mono();
    for _ in 0..30 {
        rig.handle.deliver(ms(10));
        rig.pump();
    }
    let status = rig.status.snapshot(rig.core_now());
    let track = &status.tracks[0];
    assert!((0.45..=0.51).contains(&track.peak), "{}", track.peak);
    assert_eq!(track.channel_peaks[1], 0.0);
    assert_eq!(track.frames_received, 960 + 30 * 480);
    assert!(status.bytes_written >= TRACK_HEADER_LEN as u64);
    assert!(status.elapsed >= ms(300), "{:?}", status.elapsed);
}

impl Rig {
    fn core_now(&self) -> u64 {
        self.handle.clock().now_ns()
    }
}

#[test]
fn the_folder_is_final_named_and_owned_until_the_manifest_says_complete() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(100));
    rig.pump();
    let folder = std::fs::read_dir(&rig.root)
        .unwrap()
        .flatten()
        .next()
        .unwrap()
        .path();
    let live = parse_manifest(&std::fs::read(folder.join("recording.json")).unwrap()).unwrap();
    assert_eq!(live.state, RecordingState::Recording);
    assert!(folder.join("recording.lock").exists());
    let done = rig.finish();
    assert_eq!(done.outcome.folder, folder);
}

#[test]
fn a_full_disk_at_the_journal_sync_when_the_track_ends_still_ends_with_disk_full() {
    let mut rig = Rig::mono();
    rig.handle.deliver(ms(300));
    rig.pump();
    rig.core
        .writer_for_tests()
        .inject_journal_sync_failure("mic");
    let done = rig.finish();
    assert_eq!(done.track().ended_reason, Some(EndedReason::DiskFull));
    assert_eq!(done.on_disk().state, RecordingState::Complete);
    assert_eq!(done.track().frames, done.handle.position());
    done.assert_audio(&[]);
}
