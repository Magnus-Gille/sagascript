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
    let mut detector = SleepDetector::new(Duration::from_secs(2));
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

#[test]
fn the_real_offset_is_stable_while_the_machine_is_awake() {
    let mut detector = SleepDetector::new(Duration::from_secs(1));
    assert!(!detector.poll());
    std::thread::sleep(Duration::from_millis(20));
    assert!(!detector.poll());
    assert_eq!(sleep_offset_ns().is_some(), cfg!(target_vendor = "apple"));
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
