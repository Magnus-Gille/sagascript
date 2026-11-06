//! Small text helpers shared by `record` (recorder mode) and `recordings`
//! (SAGA-564): durations, sizes, levels and the wording of the recorder's
//! warnings. Pure, so they are unit-tested without a recording.

#[cfg(feature = "record")]
use sagascript_core::recording::{session::fsclass::FolderWarning, SessionWarning};
use sagascript_core::recording::{EndedReason, Quality};

/// `h:mm:ss`, for example `1:02:03`; rounds down to whole seconds.
pub(crate) fn format_hms(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    format!("{}:{:02}:{:02}", total / 3600, total / 60 % 60, total % 60)
}

/// A size in decimal units: `512 B`, `12.3 kB`, `4.0 MB`, `1.5 GB`.
pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(feature = "record")]
/// The level of a peak in dB relative to full scale, `-inf dB` for silence.
pub(crate) fn format_level(peak: f32) -> String {
    if peak.is_finite() && peak > 1e-6 {
        format!("{:.0} dB", 20.0 * f64::from(peak).log10())
    } else {
        "-inf dB".to_string()
    }
}

pub(crate) fn quality_id(quality: Quality) -> &'static str {
    match quality {
        Quality::Lossless => "lossless",
        Quality::Compact => "compact",
    }
}

/// The wire name of an ending reason (`stopped`, `device_changed`, ...).
pub(crate) fn ended_reason_id(reason: &EndedReason) -> String {
    String::from(reason.clone())
}

/// `2026-11-05 14:02` from an RFC 3339 timestamp; the text itself when it is
/// not one.
pub(crate) fn short_when(started_at: &str) -> String {
    match (started_at.get(0..10), started_at.get(11..16)) {
        (Some(date), Some(time)) if started_at.as_bytes().get(10) == Some(&b'T') => {
            format!("{date} {time}")
        }
        _ => started_at.to_string(),
    }
}

#[cfg(feature = "record")]
/// What the user is told about a folder or disk condition.
pub(crate) fn warning_text(warning: &SessionWarning) -> &'static str {
    match warning {
        SessionWarning::LowDisk => "Free disk space is low; the recording stops cleanly if it runs out.",
        SessionWarning::Folder(FolderWarning::Fat32) => {
            "The folder is on a FAT32 volume: a track file cannot exceed 4 GiB."
        }
        SessionWarning::Folder(FolderWarning::ExFat) => {
            "The folder is on an exFAT volume, which has no journal; unplugging the disk can lose audio."
        }
        SessionWarning::Folder(FolderWarning::NetworkVolume) => {
            "The folder is on a network volume; a lost connection leaves gaps or ends the recording."
        }
        SessionWarning::Folder(FolderWarning::CloudSynced) => {
            "The folder is synced by a cloud client, which can slow or interrupt a long recording."
        }
    }
}

#[cfg(feature = "record")]
/// How an early ending is described.
pub(crate) fn ended_early_text(reason: &EndedReason) -> String {
    match reason {
        EndedReason::DeviceChanged => "the input device changed".to_string(),
        EndedReason::FormatChanged => "the input device changed its format".to_string(),
        EndedReason::SystemSleep => "the computer went to sleep".to_string(),
        EndedReason::DiskFull => "the disk is full".to_string(),
        EndedReason::Interrupted => "the recording was interrupted".to_string(),
        EndedReason::Stopped => "stopped".to_string(),
        EndedReason::Other(other) => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_h_mm_ss() {
        assert_eq!(format_hms(0.0), "0:00:00");
        assert_eq!(format_hms(9.99), "0:00:09");
        assert_eq!(format_hms(65.0), "0:01:05");
        assert_eq!(format_hms(3723.4), "1:02:03");
        assert_eq!(format_hms(-3.0), "0:00:00");
        assert_eq!(format_hms(f64::NAN), "0:00:00");
        assert_eq!(format_hms(100.0 * 3600.0), "100:00:00");
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1000), "1.0 kB");
        assert_eq!(format_bytes(12_345), "12.3 kB");
        assert_eq!(format_bytes(295_000_000), "295.0 MB");
        assert_eq!(format_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(format_bytes(u64::MAX), "18446744.1 TB");
    }

    #[cfg(feature = "record")]
    #[test]
    fn levels_are_dbfs_and_silence_is_minus_infinity() {
        assert_eq!(format_level(1.0), "0 dB");
        assert_eq!(format_level(0.5), "-6 dB");
        assert_eq!(format_level(0.1), "-20 dB");
        assert_eq!(format_level(0.0), "-inf dB");
        assert_eq!(format_level(f32::NAN), "-inf dB");
        assert_eq!(format_level(-0.3), "-inf dB");
    }

    #[test]
    fn a_timestamp_is_shortened_to_minutes() {
        assert_eq!(
            short_when("2026-11-05T14:02:11.482+01:00"),
            "2026-11-05 14:02"
        );
        assert_eq!(short_when("yesterday"), "yesterday");
        assert_eq!(short_when("2026-11-05"), "2026-11-05");
    }

    #[cfg(feature = "record")]
    #[test]
    fn every_warning_and_ending_has_words() {
        for warning in [
            SessionWarning::LowDisk,
            SessionWarning::Folder(FolderWarning::Fat32),
            SessionWarning::Folder(FolderWarning::ExFat),
            SessionWarning::Folder(FolderWarning::NetworkVolume),
            SessionWarning::Folder(FolderWarning::CloudSynced),
        ] {
            assert!(warning_text(&warning).len() > 20);
        }
        assert_eq!(
            ended_early_text(&EndedReason::DeviceChanged),
            "the input device changed"
        );
        assert_eq!(ended_early_text(&EndedReason::Other("x".into())), "x");
        assert_eq!(ended_reason_id(&EndedReason::SystemSleep), "system_sleep");
    }
}
