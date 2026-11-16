use super::*;

#[test]
fn the_worst_case_is_the_uncompressed_rate() {
    // 48 kHz mono 24-bit: 144 kB/s, about 518 MB/h (FLAC speech is 295 MB/h).
    assert_eq!(uncompressed_bytes_per_second(48_000, 1, 24), 144_000);
    assert_eq!(uncompressed_bytes_per_second(48_000, 2, 24), 288_000);
    assert_eq!(uncompressed_bytes_per_second(16_000, 1, 16), 32_000);
    assert_eq!(uncompressed_bytes_per_second(44_100, 2, 16), 176_400);
}

#[test]
fn thresholds_are_minutes_of_audio_with_floors() {
    let mono = DiskPolicy::for_rate(144_000);
    assert_eq!(mono.low_bytes, 512 * 1024 * 1024, "the floor");
    assert_eq!(mono.critical_bytes, 64 * 1024 * 1024, "the floor");
    // A 96 kHz stereo 24-bit interface: 576 kB/s, so ten minutes are 345 MB
    // (below the floor) and a multi-track session is above it.
    let big = DiskPolicy::for_rate(2_000_000);
    assert_eq!(big.low_bytes, 1_200_000_000);
    assert_eq!(big.critical_bytes, 120_000_000);
}

#[test]
fn levels_are_ordered_and_cut_at_the_thresholds() {
    let policy = DiskPolicy {
        low_bytes: 1000,
        critical_bytes: 100,
        poll_interval: Duration::from_secs(1),
    };
    assert_eq!(policy.level(5000), DiskLevel::Ok);
    assert_eq!(policy.level(1000), DiskLevel::Ok);
    assert_eq!(policy.level(999), DiskLevel::Low);
    assert_eq!(policy.level(100), DiskLevel::Low);
    assert_eq!(policy.level(99), DiskLevel::Critical);
    assert_eq!(policy.level(0), DiskLevel::Critical);
    assert!(DiskLevel::Critical > DiskLevel::Low && DiskLevel::Low > DiskLevel::Ok);
}

#[test]
fn the_real_volume_answers_even_for_a_folder_that_does_not_exist_yet() {
    let temp = sagascript_testsupport::TempDir::new();
    let missing = temp.path().join("not").join("yet");
    let free = VolumeFreeSpace.available_bytes(&missing).unwrap();
    assert!(free > 0);
}
