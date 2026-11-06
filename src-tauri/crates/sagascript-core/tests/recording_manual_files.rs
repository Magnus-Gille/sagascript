//! Writes recordings for the manual checks of SAGA-562 with `flac -t`, ffmpeg
//! and CoreAudio (`afconvert`). Ignored by default: CI never depends on those
//! tools, and the files are for a person to look at.
//!
//! ```text
//! SAGASCRIPT_RECORDING_MANUAL_DIR=/some/empty/folder \
//!   cargo test -p sagascript-core --test recording_manual_files -- --ignored
//! ```
//!
//! Then, for example, `flac -t *.flac`, `ffmpeg -v error -i x.flac -f null -`,
//! and `afconvert -f WAVE -d LEI24 x.flac x.wav`. `torn.flac` is the file a
//! killed process left (cut mid-frame): `flac -t` must reject it and
//! `recovered.flac`, made from it by recovery, must pass.

use std::path::Path;

use sagascript_core::recording::{
    recover_folder, Client, FlacEncoder, NewRecording, NewTrack, Quality, RecordingStore,
    TrackEncoder,
};

/// A deterministic float signal: tones and a low noise floor, in -1.0..1.0.
fn tones(len: usize, rate: f64, seed: u64) -> Vec<f32> {
    let mut state = seed | 1;
    (0..len)
        .map(|i| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let noise =
                (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f64 / 16_777_216.0 - 0.5;
            let t = i as f64 / rate;
            (0.30 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                + 0.18 * (2.0 * std::f64::consts::PI * 1371.0 * t).sin()
                + 0.002 * noise) as f32
        })
        .collect()
}

fn write(dir: &Path, name: &str, rate: u32, channels: u8, bits: u8, samples: &[f32]) {
    let mut encoder = FlacEncoder::create(&dir.join(name), rate, channels, bits).unwrap();
    encoder.push(samples).unwrap();
    Box::new(encoder).finish().unwrap();
}

fn edge(kind: &str, len: usize) -> Vec<f32> {
    let mut state = 547u64;
    (0..len)
        .map(|i| match kind {
            "silence" => 0.0,
            "dc-max" => 8_388_607.0 / 8_388_608.0,
            "dc-min" => -1.0,
            "square" => {
                if (i / 24) % 2 == 0 {
                    8_388_607.0 / 8_388_608.0
                } else {
                    -1.0
                }
            }
            "white" => {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                let n = (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as i64 - (1 << 23);
                n as f32 / 8_388_608.0
            }
            "alternating" => {
                if i % 2 == 0 {
                    8_388_607.0 / 8_388_608.0
                } else {
                    -1.0
                }
            }
            "impulse" => {
                if i == len / 2 {
                    8_388_607.0 / 8_388_608.0
                } else {
                    0.0
                }
            }
            _ => unreachable!(),
        })
        .collect()
}

#[test]
#[ignore = "writes files for the manual flac/ffmpeg/afconvert checks"]
fn write_files_for_the_manual_checks() {
    let dir = std::env::var("SAGASCRIPT_RECORDING_MANUAL_DIR")
        .expect("set SAGASCRIPT_RECORDING_MANUAL_DIR to an empty folder");
    let dir = Path::new(&dir);
    assert!(
        std::fs::read_dir(dir)
            .expect("the folder exists")
            .next()
            .is_none(),
        "the folder must be empty"
    );

    // Typical recordings; the lengths are not multiples of 4096.
    write(
        dir,
        "speech-mono-48k-24.flac",
        48_000,
        1,
        24,
        &tones(20 * 48_000 + 251, 48_000.0, 1),
    );
    write(
        dir,
        "short-251.flac",
        48_000,
        1,
        24,
        &tones(251, 48_000.0, 2),
    );
    let left = tones(10 * 44_100 + 17, 44_100.0, 3);
    let stereo: Vec<f32> = left.iter().flat_map(|s| [*s, *s * 0.5]).collect();
    write(dir, "stereo-44k1-16.flac", 44_100, 2, 16, &stereo);
    write(
        dir,
        "compact-16k-16.flac",
        16_000,
        1,
        16,
        &tones(30 * 16_000 + 5, 16_000.0, 4),
    );
    write(
        dir,
        "seek-8k-16-25s.flac",
        8_000,
        1,
        16,
        &tones(25 * 8_000, 8_000.0, 5),
    );
    write(
        dir,
        "hires-96k-24.flac",
        96_000,
        1,
        24,
        &tones(5 * 96_000 + 99, 96_000.0, 6),
    );

    // The seven edge signals of the study, 480 000 samples each.
    for kind in [
        "silence",
        "dc-max",
        "dc-min",
        "square",
        "white",
        "alternating",
        "impulse",
    ] {
        write(
            dir,
            &format!("edge-{kind}.flac"),
            48_000,
            1,
            24,
            &edge(kind, 480_000),
        );
    }

    // A killed recording: the track file cut mid-frame, then recovered.
    let store_dir = dir.join("store");
    std::fs::create_dir(&store_dir).unwrap();
    let store = RecordingStore::new(&store_dir);
    let mut writer = store
        .create(NewRecording {
            title: "Manual check".into(),
            started_at: "2026-11-06T09:30:00.000+01:00".into(),
            client: Client {
                kind: "desktop".into(),
                app_version: "test".into(),
                build: "test".into(),
                extra: Default::default(),
            },
            quality: Quality::Lossless,
            tracks: vec![NewTrack {
                id: "mic".into(),
                role: "mic".into(),
                sample_rate: 48_000,
                channels: 1,
                bits: 24,
                clock_domain: "mach_continuous".into(),
                start_ns: 0,
                resampled_from: None,
                device: None,
            }],
        })
        .unwrap();
    writer
        .push("mic", &tones(12 * 4096 + 300, 48_000.0, 7))
        .unwrap();
    let folder = writer.folder().to_path_buf();
    drop(writer);
    let track = folder.join("mic.flac");
    let torn = std::fs::metadata(&track).unwrap().len() - 1234;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&track)
        .unwrap()
        .set_len(torn)
        .unwrap();
    std::fs::copy(&track, dir.join("torn.flac")).unwrap();
    recover_folder(&folder).unwrap();
    std::fs::copy(&track, dir.join("recovered.flac")).unwrap();
    std::fs::copy(folder.join("mic.tail"), dir.join("recovered.tail")).unwrap();
    println!("wrote the files to {}", dir.display());
}
