use std::fs;
use std::path::Path;

use super::*;

fn destination(dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    dir.path().join(name)
}

fn data_bytes(wav: &[u8]) -> &[u8] {
    &wav[44..]
}

#[test]
fn writes_stereo_header_and_microphone_then_system_samples() {
    let dir = tempfile::tempdir().expect("temp dir");
    let dest = destination(&dir, "meeting.wav");
    let mut recording = Recording::new().expect("recording");
    recording
        .append(&[1.0, -1.0], &[0.5, -0.5])
        .expect("append");
    recording.finalize().expect("finalize");
    recording.export(&dest).expect("export");

    let wav = fs::read(&dest).expect("published wav");
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 1);
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 2);
    assert_eq!(
        u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]),
        16_000
    );
    assert_eq!(
        u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]),
        64_000
    );
    assert_eq!(u16::from_le_bytes([wav[32], wav[33]]), 4);
    assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]), 8);
    assert_eq!(
        data_bytes(&wav),
        &[
            0xff, 0x7f, 0xff, 0x3f, // microphone 1.0, system 0.5
            0x01, 0x80, 0x01, 0xc0, // microphone -1.0, system -0.5
        ]
    );
}

#[test]
fn invalid_input_is_rejected_before_any_pcm_is_written() {
    let mut recording = Recording::new().expect("recording");
    assert!(matches!(
        recording.append(&[0.0], &[]),
        Err(RecordingError::MismatchedChannels)
    ));
    assert_eq!(recording.duration_samples(), 0);
    assert!(matches!(
        recording.append(&[f32::NAN], &[0.0]),
        Err(RecordingError::NonFiniteSample)
    ));
    assert_eq!(recording.duration_samples(), 0);
}

#[test]
fn finalize_is_idempotent_and_rejects_later_append() {
    let mut recording = Recording::new().expect("recording");
    recording.append(&[0.25], &[0.75]).expect("append");
    recording.finalize().expect("finalize");
    recording.finalize().expect("idempotent finalize");
    assert!(matches!(
        recording.append(&[0.0], &[0.0]),
        Err(RecordingError::Finalized)
    ));
}

#[test]
fn failed_export_keeps_spool_and_never_replaces_destination() {
    let dir = tempfile::tempdir().expect("temp dir");
    let existing = destination(&dir, "existing.wav");
    let retry = destination(&dir, "retry.wav");
    fs::write(&existing, b"original").expect("existing file");

    let mut recording = Recording::new().expect("recording");
    recording.append(&[0.0], &[0.0]).expect("append");
    assert!(recording.export(&existing).is_err());
    assert_eq!(fs::read(&existing).expect("existing bytes"), b"original");
    recording.export(&retry).expect("retry export");
    assert!(retry.exists());
}

#[test]
fn small_injected_limit_rejects_without_partial_append() {
    let mut recording = Recording::with_max_samples(2).expect("recording");
    recording.append(&[0.0, 0.0], &[0.0, 0.0]).expect("limit");
    assert_eq!(recording.duration_samples(), 2);
    assert!(matches!(
        recording.append(&[0.0], &[0.0]),
        Err(RecordingError::DurationExceeded)
    ));
    assert_eq!(recording.duration_samples(), 2);
}

#[test]
fn spool_has_no_destination_until_explicit_export_and_hash_is_finalized_wav() {
    let dir = tempfile::tempdir().expect("temp dir");
    let dest = destination(&dir, "explicit.wav");
    let mut recording = Recording::new().expect("recording");
    recording.append(&[0.0], &[0.0]).expect("append");
    assert!(!dest.exists());
    assert!(matches!(
        recording.sha256(),
        Err(RecordingError::NotFinalized)
    ));
    recording.finalize().expect("finalize");
    let digest = recording.sha256().expect("sha256");
    recording.export(&dest).expect("export");
    assert_eq!(
        digest,
        crate::hash::sha256_hex(fs::read(&dest).expect("wav"))
    );
    assert!(Path::new(&dest).is_file());
}
