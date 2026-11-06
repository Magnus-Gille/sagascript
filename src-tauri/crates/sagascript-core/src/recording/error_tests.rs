use std::collections::BTreeSet;
use std::io;

use super::*;

fn every_variant() -> Vec<RecordingError> {
    vec![
        RecordingError::io(
            "create the recording folder",
            &io::Error::from_raw_os_error(2),
        ),
        RecordingError::io(
            "write a track",
            &io::Error::from(io::ErrorKind::StorageFull),
        ),
        RecordingError::io(
            "write a track",
            &io::Error::from(io::ErrorKind::PermissionDenied),
        ),
        RecordingError::NameUnavailable,
        RecordingError::Locked,
        RecordingError::NotRecording,
        RecordingError::ManifestInvalid { reason: "bad" },
        RecordingError::ManifestTooNew {
            required: 9,
            supported: 1,
        },
        RecordingError::UnsupportedFormat { reason: "rate" },
        RecordingError::InvalidInput { reason: "frames" },
        RecordingError::UnknownTrack,
        RecordingError::TrackFinished,
        RecordingError::CorruptTrack { reason: "magic" },
        RecordingError::SourceUnavailable {
            reason: "no input device",
        },
        RecordingError::SessionFailed {
            reason: "the writer panicked",
        },
        RecordingError::encoder("failed at /Users/someone/Music/secret.flac"),
    ]
}

#[test]
fn every_variant_has_its_own_stable_snake_case_code() {
    let codes: Vec<&str> = every_variant().iter().map(RecordingError::code).collect();
    let unique: BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(unique.len(), codes.len(), "codes repeat: {codes:?}");
    for code in codes {
        assert!(code.starts_with("recording_"), "{code}");
        assert!(
            code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "{code}"
        );
    }
}

#[test]
fn messages_never_carry_a_path() {
    for error in every_variant() {
        let message = error.to_string();
        // The scrubbed form keeps a file name but never the folders before it.
        assert!(!message.contains("/Users"), "{message}");
        assert_eq!(scrub_paths(&message), message, "{message}");
    }
}

#[test]
fn a_full_disk_is_classified_and_retryable() {
    for kind in [io::ErrorKind::StorageFull, io::ErrorKind::QuotaExceeded] {
        let error = RecordingError::io("write a track", &io::Error::from(kind));
        assert!(error.is_disk_full(), "{kind:?}");
        assert_eq!(error.code(), "recording_disk_full");
        assert!(error.retryable());
    }
}

#[test]
fn only_the_documented_failures_are_retryable() {
    let retryable: BTreeSet<&str> = every_variant()
        .iter()
        .filter(|e| e.retryable())
        .map(RecordingError::code)
        .collect();
    assert_eq!(
        retryable,
        BTreeSet::from([
            "recording_disk_full",
            "recording_locked",
            "recording_name_unavailable",
        ])
    );
}

#[test]
fn other_io_errors_keep_the_kind_and_the_os_code_only() {
    let error = RecordingError::io(
        "create the recording folder",
        &io::Error::from_raw_os_error(2),
    );
    assert_eq!(error.code(), "recording_io");
    assert!(error.to_string().contains("os error 2"), "{error}");
}
