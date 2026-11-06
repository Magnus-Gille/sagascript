use std::time::Duration;

use sagascript_testsupport::TempDir;

use super::*;
use crate::recording::capture::NativeFormat;
use crate::recording::test_recordings::*;
use crate::recording::test_signals::{recording, track, STARTED_AT};
use crate::recording::{Quality, RecordingStore};

fn all_chunks(folder: &Path) -> Vec<TrackChunk> {
    decode_tracks(folder)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn of_track<'a>(chunks: &'a [TrackChunk], id: &str) -> Vec<&'a TrackChunk> {
    chunks.iter().filter(|c| c.track_id == id).collect()
}

fn joined(chunks: &[&TrackChunk]) -> Vec<f32> {
    chunks
        .iter()
        .flat_map(|c| c.samples.iter().copied())
        .collect()
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn store() -> (TempDir, RecordingStore) {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    (temp, store)
}

#[test]
fn a_lossless_recording_decodes_to_contiguous_bounded_chunks_at_16_khz() {
    let (_temp, store) = store();
    let made = record_mic(&store, "Decode", STARTED_AT, Quality::Lossless, MONO48, 3.0);

    let chunks = all_chunks(&made.folder);
    assert!(chunks.len() >= 2, "3 s do not fit in one 2 s chunk");
    let mut next = 0;
    for chunk in &chunks {
        assert!(!chunk.samples.is_empty() && chunk.samples.len() <= CHUNK_SAMPLES);
        assert_eq!(
            (chunk.track_id.as_str(), chunk.role.as_str()),
            ("mic", "mic")
        );
        assert_eq!(chunk.start_sample, next, "chunks follow one another");
        next += chunk.samples.len() as u64;
    }
    let total = next as i64;
    assert!((total - 48_000).abs() <= 512, "3 s at 16 kHz, got {total}");
    // A 0.5 sine has an RMS of 0.354; the middle is clear of resampler edges.
    let samples = joined(&chunks.iter().collect::<Vec<_>>());
    let middle = rms(&samples[8_000..40_000]);
    assert!((middle - 0.354).abs() < 0.02, "rms {middle}");
}

#[test]
fn a_compact_recording_decodes_without_resampling() {
    let (_temp, store) = store();
    let made = record_mic(&store, "Compact", STARTED_AT, Quality::Compact, MONO48, 1.0);
    let frames = made.outcome.manifest.tracks[0].frames;

    let samples = joined(&all_chunks(&made.folder).iter().collect::<Vec<_>>());
    assert_eq!(samples.len() as u64, frames, "16 kHz in, 16 kHz out");
    assert!((rms(&samples[2_000..14_000]) - 0.354).abs() < 0.02);
}

#[test]
fn an_overrun_gap_keeps_the_length_and_decodes_as_silence() {
    let (_temp, store) = store();
    let made = record(
        &store,
        "Gap",
        STARTED_AT,
        Quality::Lossless,
        &[Take::mic(MONO48)],
        |session, handles| {
            deliver_all(
                session,
                handles,
                Duration::from_secs(1),
                Duration::from_millis(100),
            );
            handles[0].overrun(24_000); // half a second dropped
            deliver_all(
                session,
                handles,
                Duration::from_secs(1),
                Duration::from_millis(100),
            );
        },
    );
    let track = &made.outcome.manifest.tracks[0];
    assert_eq!(track.gaps.len(), 1);
    let gap = &track.gaps[0];

    let samples = joined(&all_chunks(&made.folder).iter().collect::<Vec<_>>());
    // 1.02 s + 0.5 s + 1 s of timeline (the fake source primes 20 ms).
    let expected = (track.frames / 3) as i64;
    assert!(
        (samples.len() as i64 - expected).abs() <= 512,
        "{} samples for {} frames",
        samples.len(),
        track.frames
    );
    let from = (gap.at_frame / 3) as usize;
    let to = ((gap.at_frame + gap.length) / 3) as usize;
    assert!(
        rms(&samples[from + 400..to - 400]) < 1e-3,
        "the gap decodes as silence"
    );
    assert!(
        rms(&samples[from - 4_000..from - 400]) > 0.3,
        "audio before it"
    );
    assert!(rms(&samples[to + 400..to + 4_000]) > 0.3, "audio after it");
}

#[test]
fn a_later_track_starts_with_silence_for_its_offset() {
    let (_temp, store) = store();
    let made = record(
        &store,
        "Two tracks",
        STARTED_AT,
        Quality::Lossless,
        &[
            Take::mic(MONO48),
            Take::system(MONO48, Duration::from_millis(500)),
        ],
        |session, handles| {
            deliver_all(
                session,
                handles,
                Duration::from_secs(2),
                Duration::from_millis(100),
            );
        },
    );
    let tracks = &made.outcome.manifest.tracks;
    let offset_ns = tracks[1].clock.start_ns - tracks[0].clock.start_ns;
    assert_eq!(
        offset_ns, 500_000_000,
        "the second track started 500 ms later"
    );

    let chunks = all_chunks(&made.folder);
    let mic = of_track(&chunks, "mic");
    let system = of_track(&chunks, "system");
    assert_eq!(mic[0].start_sample, 0);
    assert_eq!(
        system[0].start_sample, 0,
        "its silence starts the timeline too"
    );
    let system_samples = joined(&system);
    assert!(
        rms(&system_samples[..7_900]) < 1e-6,
        "500 ms = 8000 samples of silence"
    );
    assert!(rms(&system_samples[8_400..12_000]) > 0.3, "then audio");
    let mic_samples = joined(&mic);
    assert!((system_samples.len() as i64 - mic_samples.len() as i64 - 8_000).abs() <= 512);
    // Tracks come one after another: all of mic, then all of system.
    let order: Vec<&str> = chunks.iter().map(|c| c.track_id.as_str()).collect();
    let switch = order.iter().position(|id| *id == "system").unwrap();
    assert!(order[..switch].iter().all(|id| *id == "mic"));
    assert!(order[switch..].iter().all(|id| *id == "system"));
}

#[test]
fn a_stereo_track_is_mixed_down_to_mono() {
    let (_temp, store) = store();
    let stereo = NativeFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let made = record_mic(&store, "Stereo", STARTED_AT, Quality::Lossless, stereo, 1.0);
    assert_eq!(made.outcome.manifest.tracks[0].channels, 2);
    let samples = joined(&all_chunks(&made.folder).iter().collect::<Vec<_>>());
    // The same sine on both channels: the mean is the sine.
    assert!((rms(&samples[2_000..14_000]) - 0.354).abs() < 0.02);

    let mut out = Vec::new();
    downmix_into(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
    assert_eq!(out, vec![0.5, 0.5]);
}

#[test]
fn a_recording_that_is_still_recording_is_refused() {
    let (_temp, store) = store();
    let writer = store
        .create(recording("Open", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let error = decode_tracks(writer.folder()).err().expect("refused");
    assert_eq!(error.code(), "recording_recovery_needed");
}

#[test]
fn a_track_shorter_than_the_manifest_says_is_a_typed_error() {
    let (_temp, store) = store();
    let made = record_mic(&store, "Cut", STARTED_AT, Quality::Lossless, MONO48, 2.0);
    let path = made.folder.join("mic.flac");
    let length = std::fs::metadata(&path).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(length / 2)
        .unwrap();

    let mut decoder = decode_tracks(&made.folder).unwrap();
    let error = decoder
        .find_map(|item| item.err())
        .expect("the cut file is reported");
    assert_eq!(error.code(), "recording_corrupt_track");
    assert!(decoder.next().is_none(), "nothing follows an error");
}

#[test]
fn a_missing_track_file_is_reported() {
    let (_temp, store) = store();
    let made = record_mic(
        &store,
        "Missing",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        0.5,
    );
    std::fs::remove_file(made.folder.join("mic.flac")).unwrap();
    let error = decode_tracks(&made.folder)
        .unwrap()
        .find_map(|item| item.err())
        .expect("reported");
    assert_eq!(error.code(), "recording_io");
}

#[test]
fn a_folder_without_a_manifest_is_refused() {
    let temp = TempDir::new();
    assert!(decode_tracks(temp.path()).is_err());
}
