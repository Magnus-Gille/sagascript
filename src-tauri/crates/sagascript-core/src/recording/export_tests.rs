use std::io::Cursor;
use std::time::Duration;

use sagascript_testsupport::TempDir;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use super::*;
use crate::recording::capture::{FakeSignal, NativeFormat};
use crate::recording::test_recordings::*;
use crate::recording::test_signals::{decode_flac, recording, track, STARTED_AT};
use crate::recording::{Quality, RecordingStore};

const MONO44: NativeFormat = NativeFormat {
    sample_rate: 44_100,
    channels: 1,
};

struct Rig {
    temp: TempDir,
    store: RecordingStore,
}

fn rig() -> Rig {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    Rig { temp, store }
}

impl Rig {
    fn out(&self, name: &str) -> PathBuf {
        self.temp.path().join("exports").join(name)
    }
}

fn options(format: ExportFormat, layout: ExportLayout, out: PathBuf) -> ExportOptions {
    ExportOptions {
        format,
        layout,
        out,
    }
}

/// Mic and a system track that started `later` after it, 1 s of audio each.
fn two_tracks(store: &RecordingStore, mic: NativeFormat, system: NativeFormat, later: u64) -> Made {
    record(
        store,
        "Two",
        STARTED_AT,
        Quality::Lossless,
        &[
            Take::mic(mic),
            Take::system(system, Duration::from_millis(later)),
        ],
        |session, handles| {
            deliver_all(
                session,
                handles,
                Duration::from_secs(1),
                Duration::from_millis(100),
            );
        },
    )
}

/// What an independent decoder (`symphonia`) makes of a WAV: rate, channels,
/// interleaved 24-bit samples.
fn decode_wav(bytes: &[u8]) -> (u32, usize, Vec<i32>) {
    // The header first, by hand: sizes must be patched.
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..16], b"WAVEfmt ");
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
        bytes.len() - 8,
        "the RIFF size"
    );
    let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
    assert!(
        bytes.len() - 44 - data <= 1,
        "the data size (and one pad byte)"
    );

    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("wav");
    let mut reader = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .expect("symphonia probes the WAV")
        .format;
    let track = reader.default_track().unwrap().clone();
    let rate = track.codec_params.sample_rate.unwrap();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .unwrap();
    let mut channels = 0;
    let mut samples = Vec::new();
    loop {
        let packet = match reader.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) => break,
            Err(error) => panic!("{error}"),
        };
        match decoder.decode(&packet).unwrap() {
            AudioBufferRef::S24(buffer) => {
                channels = buffer.spec().channels.count();
                for i in 0..buffer.frames() {
                    for c in 0..channels {
                        samples.push(buffer.chan(c)[i].inner());
                    }
                }
            }
            other => panic!("expected 24-bit samples, got {:?}", other.spec()),
        }
    }
    (rate, channels, samples)
}

fn channel(samples: &[i32], channels: usize, index: usize) -> Vec<i32> {
    samples
        .iter()
        .skip(index)
        .step_by(channels)
        .copied()
        .collect()
}

#[test]
fn flac_tracks_are_byte_copies_of_the_stored_files() {
    let rig = rig();
    let made = two_tracks(&rig.store, MONO48, MONO48, 0);
    let out = rig.out("tracks");
    let report = export_recording(
        &made.folder,
        &options(ExportFormat::Flac, ExportLayout::Tracks, out.clone()),
    )
    .unwrap();

    assert_eq!(
        report.files,
        vec![out.join("mic.flac"), out.join("system.flac")]
    );
    assert!(!report.derived && report.resampled_to.is_none() && report.note.is_none());
    for name in ["mic.flac", "system.flac"] {
        assert_eq!(
            std::fs::read(out.join(name)).unwrap(),
            std::fs::read(made.folder.join(name)).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn wav_tracks_decode_to_the_stored_samples() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Wav",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        1.0,
    );
    let out = rig.out("wav");
    let report = export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Tracks, out.clone()),
    )
    .unwrap();
    assert_eq!(report.files, vec![out.join("mic.wav")]);
    assert!(!report.derived);

    let stored = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap());
    let (rate, channels, samples) = decode_wav(&std::fs::read(out.join("mic.wav")).unwrap());
    assert_eq!((rate, channels), (48_000, 1));
    assert_eq!(samples, stored.samples, "the same 24-bit samples");
}

#[test]
fn a_stereo_track_and_a_compact_track_keep_their_format_in_wav() {
    let rig = rig();
    let stereo = NativeFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let made = record(
        &rig.store,
        "Stereo",
        STARTED_AT,
        Quality::Lossless,
        &[Take::mic(stereo)],
        |session, handles| {
            handles[0].set_signal(FakeSignal::Noise {
                amplitude: 0.5,
                seed: 3,
            });
            deliver_all(
                session,
                handles,
                Duration::from_millis(500),
                Duration::from_millis(100),
            );
        },
    );
    let out = rig.out("stereo");
    export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Tracks, out.clone()),
    )
    .unwrap();
    let stored = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap());
    let (rate, channels, samples) = decode_wav(&std::fs::read(out.join("mic.wav")).unwrap());
    assert_eq!((rate, channels), (48_000, 2));
    assert_eq!(samples, stored.samples);

    // Compact: 16 kHz, 16-bit samples widened to 24 bits.
    let compact = record_mic(
        &rig.store,
        "Compact",
        STARTED_AT,
        Quality::Compact,
        MONO48,
        0.5,
    );
    let out = rig.out("compact");
    export_recording(
        &compact.folder,
        &options(ExportFormat::Wav, ExportLayout::Tracks, out.clone()),
    )
    .unwrap();
    let stored = decode_flac(&std::fs::read(compact.folder.join("mic.flac")).unwrap());
    let (rate, _, samples) = decode_wav(&std::fs::read(out.join("mic.wav")).unwrap());
    assert_eq!(rate, 16_000);
    let widened: Vec<i32> = stored.samples.iter().map(|s| s << 8).collect();
    assert_eq!(samples, widened);
}

#[test]
fn stereo_puts_track_one_left_track_two_right_aligned_by_their_offsets() {
    let rig = rig();
    let made = two_tracks(&rig.store, MONO48, MONO48, 500);
    let out = rig.out("stereo.flac");
    let report = export_recording(
        &made.folder,
        &options(ExportFormat::Flac, ExportLayout::Stereo, out.clone()),
    )
    .unwrap();
    assert_eq!(report.files, vec![out.clone()]);
    assert!(report.derived);
    assert_eq!(report.resampled_to, None);
    assert_eq!(
        report.note.as_deref(),
        Some("derived from the stored tracks")
    );

    let mic = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap()).samples;
    let system = decode_flac(&std::fs::read(made.folder.join("system.flac")).unwrap()).samples;
    let exported = decode_flac(&std::fs::read(&out).unwrap());
    assert_eq!((exported.sample_rate, exported.channels), (48_000, 2));
    let left = channel(&exported.samples, 2, 0);
    let right = channel(&exported.samples, 2, 1);
    let frames = (24_000 + system.len()).max(mic.len());
    assert_eq!(left.len(), frames);
    assert_eq!(&left[..mic.len()], &mic[..]);
    assert!(left[mic.len()..].iter().all(|s| *s == 0));
    assert!(
        right[..24_000].iter().all(|s| *s == 0),
        "500 ms of silence first"
    );
    assert_eq!(&right[24_000..24_000 + system.len()], &system[..]);
}

#[test]
fn mixdown_is_the_mean_of_the_tracks() {
    let rig = rig();
    let made = two_tracks(&rig.store, MONO48, MONO48, 500);
    let out = rig.out("mix.wav");
    let report = export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Mixdown, out.clone()),
    )
    .unwrap();
    assert!(report.derived);
    assert_eq!(
        report.note.as_deref(),
        Some("derived from the stored tracks; tracks were summed")
    );

    let mic = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap()).samples;
    let system = decode_flac(&std::fs::read(made.folder.join("system.flac")).unwrap()).samples;
    let (rate, channels, mixed) = decode_wav(&std::fs::read(&out).unwrap());
    assert_eq!((rate, channels), (48_000, 1));
    assert_eq!(mixed.len(), (24_000 + system.len()).max(mic.len()));
    for (i, value) in mixed.iter().enumerate() {
        let a = mic.get(i).copied().unwrap_or(0);
        let b = i
            .checked_sub(24_000)
            .and_then(|j| system.get(j))
            .copied()
            .unwrap_or(0);
        let expected = (f64::from(a) + f64::from(b)) / 2.0;
        assert!((f64::from(*value) - expected).abs() <= 1.0, "frame {i}");
    }
}

#[test]
fn one_track_is_the_same_on_both_sides_or_keeps_its_own_channels() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Mono",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        0.5,
    );
    let out = rig.out("mono-as-stereo.wav");
    export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Stereo, out.clone()),
    )
    .unwrap();
    let stored = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap()).samples;
    let (_, channels, samples) = decode_wav(&std::fs::read(&out).unwrap());
    assert_eq!(channels, 2);
    assert_eq!(channel(&samples, 2, 0), stored);
    assert_eq!(channel(&samples, 2, 1), stored);

    let stereo = NativeFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let made = record(
        &rig.store,
        "Pair",
        STARTED_AT,
        Quality::Lossless,
        &[Take::mic(stereo)],
        |session, handles| {
            handles[0].set_signal(FakeSignal::Noise {
                amplitude: 0.5,
                seed: 9,
            });
            deliver_all(
                session,
                handles,
                Duration::from_millis(500),
                Duration::from_millis(100),
            );
        },
    );
    let out = rig.out("pair.wav");
    export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Stereo, out.clone()),
    )
    .unwrap();
    let stored = decode_flac(&std::fs::read(made.folder.join("mic.flac")).unwrap()).samples;
    let (_, _, samples) = decode_wav(&std::fs::read(&out).unwrap());
    assert_eq!(samples, stored, "its own left and right");
}

#[test]
fn tracks_at_different_rates_are_resampled_to_the_highest() {
    let rig = rig();
    let made = two_tracks(&rig.store, MONO44, MONO48, 0);
    let out = rig.out("resampled.wav");
    let report = export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Stereo, out.clone()),
    )
    .unwrap();
    assert!(report.derived);
    assert_eq!(report.resampled_to, Some(48_000));
    let note = report.note.unwrap();
    assert!(
        note.contains("derived and resampled from 44100 Hz"),
        "{note}"
    );

    let system_frames = made.outcome.manifest.tracks[1].frames as i64;
    let (rate, channels, samples) = decode_wav(&std::fs::read(&out).unwrap());
    assert_eq!((rate, channels), (48_000, 2));
    assert!((samples.len() as i64 / 2 - system_frames).abs() <= 1);
    // The 44.1 kHz sine, now at 48 kHz, in phase .
    let left = channel(&samples, 2, 0);
    let worst = (4_000..40_000)
        .map(|n| {
            let ideal = 0.5 * (std::f64::consts::TAU * 440.0 * n as f64 / 48_000.0).sin();
            (f64::from(left[n]) / f64::from(1 << 23) - ideal).abs()
        })
        .fold(0.0, f64::max);
    assert!(worst < 0.01, "worst error {worst}");
}

#[test]
fn an_existing_output_is_never_replaced() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Keep",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        0.3,
    );
    let out = rig.out("once.wav");
    let opts = options(ExportFormat::Wav, ExportLayout::Mixdown, out.clone());
    export_recording(&made.folder, &opts).unwrap();
    let first = std::fs::read(&out).unwrap();

    let error = export_recording(&made.folder, &opts).unwrap_err();
    assert_eq!(error.code(), "recording_io");
    assert!(matches!(
        error,
        RecordingError::Io {
            kind: io::ErrorKind::AlreadyExists,
            ..
        }
    ));
    assert_eq!(std::fs::read(&out).unwrap(), first);

    // Tracks: one name taken, so nothing is written at all.
    let dir = rig.out("tracks");
    let two = two_tracks(&rig.store, MONO48, MONO48, 0);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("system.flac"), b"mine").unwrap();
    let error = export_recording(
        &two.folder,
        &options(ExportFormat::Flac, ExportLayout::Tracks, dir.clone()),
    )
    .unwrap_err();
    assert_eq!(error.code(), "recording_io");
    assert!(!dir.join("mic.flac").exists());
    assert_eq!(std::fs::read(dir.join("system.flac")).unwrap(), b"mine");
}

#[test]
fn an_output_inside_the_recording_folder_is_refused() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Inside",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        0.3,
    );
    let name = made.folder.file_name().unwrap().to_owned();
    let before = std::fs::read_dir(&made.folder).unwrap().count();
    for (layout, out) in [
        (ExportLayout::Mixdown, made.folder.join("mix.wav")),
        (ExportLayout::Tracks, made.folder.join("exports")),
        (
            ExportLayout::Stereo,
            made.folder.join("deeper/er/stereo.wav"),
        ),
        // Through the parent and back down.
        (
            ExportLayout::Mixdown,
            made.folder.join("..").join(&name).join("sneaky.wav"),
        ),
    ] {
        let error =
            export_recording(&made.folder, &options(ExportFormat::Wav, layout, out)).unwrap_err();
        assert_eq!(error.code(), "recording_invalid_input", "{layout:?}");
    }
    assert_eq!(std::fs::read_dir(&made.folder).unwrap().count(), before);
    assert!(!made.folder.join("deeper").exists());
}

#[cfg(unix)]
#[test]
fn a_link_into_the_recording_folder_does_not_get_around_the_rule() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Link",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        0.3,
    );
    let link = rig.temp.path().join("link");
    std::os::unix::fs::symlink(&made.folder, &link).unwrap();
    let error = export_recording(
        &made.folder,
        &options(ExportFormat::Wav, ExportLayout::Mixdown, link.join("x.wav")),
    )
    .unwrap_err();
    assert_eq!(error.code(), "recording_invalid_input");
}

#[test]
fn a_recording_in_progress_is_not_exported() {
    let rig = rig();
    let writer = rig
        .store
        .create(recording("Open", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let error = export_recording(
        writer.folder(),
        &options(ExportFormat::Flac, ExportLayout::Tracks, rig.out("t")),
    )
    .unwrap_err();
    assert_eq!(error.code(), "recording_recovery_needed");
}

#[test]
fn a_damaged_track_fails_the_export_and_leaves_no_file() {
    let rig = rig();
    let made = record_mic(
        &rig.store,
        "Cut",
        STARTED_AT,
        Quality::Lossless,
        MONO48,
        1.0,
    );
    let path = made.folder.join("mic.flac");
    let length = std::fs::metadata(&path).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(length / 2)
        .unwrap();

    for (format, layout, out) in [
        (ExportFormat::Wav, ExportLayout::Tracks, rig.out("a")),
        (ExportFormat::Flac, ExportLayout::Mixdown, rig.out("b.flac")),
    ] {
        let error =
            export_recording(&made.folder, &options(format, layout, out.clone())).unwrap_err();
        assert_eq!(error.code(), "recording_corrupt_track");
        let leftovers = std::fs::read_dir(&out)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "no half-written file");
        assert!(!out.is_file());
    }
}
