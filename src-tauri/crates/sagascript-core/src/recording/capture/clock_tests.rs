use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::*;

#[test]
fn the_host_clock_moves_forward_and_is_never_zero() {
    let a = host_now_ns();
    std::thread::sleep(Duration::from_millis(2));
    let b = host_now_ns();
    assert!(a > 0);
    assert!(b > a);
    assert!(
        b - a < 5_000_000_000,
        "nanoseconds, not ticks of another unit"
    );
}

#[test]
fn the_domain_is_named_for_the_platform() {
    if cfg!(target_vendor = "apple") {
        assert_eq!(HOST_CLOCK_DOMAIN, "mach_continuous");
    } else if cfg!(windows) {
        assert_eq!(HOST_CLOCK_DOMAIN, "qpc");
    } else {
        assert_eq!(HOST_CLOCK_DOMAIN, "monotonic");
    }
}

#[test]
fn a_sleep_is_the_offset_between_the_clocks_growing() {
    let mut detector = SleepDetector::unarmed(Duration::from_secs(2));
    assert!(!detector.observe(5_000_000_000), "the baseline");
    assert!(!detector.observe(5_000_001_000), "ordinary drift");
    assert!(
        !detector.observe(7_000_000_000),
        "exactly the threshold is not enough"
    );
    assert!(detector.observe(7_000_000_000 + 2_000_000_001));
    assert!(
        !detector.observe(9_000_000_002),
        "the new baseline is the offset after the sleep"
    );
}

fn probe_of(offset: &Arc<AtomicU64>) -> SleepProbe {
    let offset = Arc::clone(offset);
    Arc::new(move || Some(offset.load(Ordering::SeqCst)))
}

#[test]
fn the_baseline_is_read_when_the_detector_is_armed_so_an_early_sleep_counts() {
    let offset = Arc::new(AtomicU64::new(1_000));
    let mut detector = SleepDetector::armed(Duration::from_millis(100), probe_of(&offset));
    // The machine sleeps before the detector has been polled even once.
    offset.fetch_add(5_000_000_000, Ordering::SeqCst);
    assert!(detector.poll(), "a sleep before the first poll was missed");
    assert!(!detector.poll(), "and it is reported once");
}

#[test]
fn a_short_sleep_of_a_fraction_of_a_second_is_noticed() {
    let offset = Arc::new(AtomicU64::new(0));
    let mut detector = SleepDetector::armed(Duration::from_millis(100), probe_of(&offset));
    offset.fetch_add(60_000_000, Ordering::SeqCst); // clock noise, not a sleep
    assert!(!detector.poll());
    offset.fetch_add(150_000_000, Ordering::SeqCst); // 150 ms asleep
    assert!(detector.poll());
}

#[test]
fn a_probe_that_reads_nothing_never_reports_a_sleep() {
    let mut detector = SleepDetector::armed(Duration::from_millis(100), Arc::new(|| None));
    assert!(!detector.poll());
    assert!(!SleepDetector::unarmed(Duration::from_millis(1)).poll());
}

#[test]
fn the_real_offset_is_stable_while_the_machine_is_awake() {
    let mut detector = SleepDetector::armed(Duration::from_millis(100), system_sleep_probe());
    assert!(!detector.poll());
    std::thread::sleep(Duration::from_millis(20));
    assert!(!detector.poll());
    assert_eq!(
        sleep_offset_ns().is_some(),
        cfg!(any(target_vendor = "apple", windows))
    );
}

#[test]
fn the_manual_clock_moves_only_when_told_and_the_host_clock_follows_it() {
    let clock = ManualClock::new();
    assert_eq!(clock.now_ns(), 0);
    assert_eq!(clock.host_ns(), MANUAL_HOST_BASE_NS);
    clock.advance(Duration::from_millis(250));
    clock.advance_ns(5);
    assert_eq!(clock.now_ns(), 250_000_005);
    assert_eq!(clock.host_ns(), MANUAL_HOST_BASE_NS + 250_000_005);
}

#[test]
fn the_system_clock_is_the_host_clock_and_a_monotonic_counter() {
    let clock = SystemClock::new();
    let (a, host_a) = (clock.now_ns(), clock.host_ns());
    std::thread::sleep(Duration::from_millis(2));
    assert!(clock.now_ns() > a);
    assert!(clock.host_ns() > host_a);
}
