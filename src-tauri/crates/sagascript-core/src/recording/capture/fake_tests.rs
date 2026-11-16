use std::time::Duration;

use super::*;
use crate::recording::capture::{track_channel, ChannelConfig};

const MONO: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};

fn started(
    prime: Duration,
) -> (
    FakeHandle,
    crate::recording::capture::TrackReceiver,
    FakeSource,
) {
    let (mut source, handle) = FakeSource::new(MONO);
    source = source.with_prime(prime);
    let (sink, receiver) = track_channel(
        MONO,
        ChannelConfig {
            block_frames: 1024,
            blocks: 64,
        },
    );
    source.start(sink).unwrap();
    (handle, receiver, source)
}

#[test]
fn start_delivers_the_prime_and_deliver_advances_the_clock() {
    let (handle, mut receiver, _source) = started(Duration::from_millis(20));
    let first = receiver.pop().expect("the prime");
    assert_eq!(first.frames(), 960);
    assert_eq!(
        first.host_ns(),
        crate::recording::capture::MANUAL_HOST_BASE_NS
    );
    assert_eq!(handle.clock().now_ns(), 20_000_000);

    assert!(handle.deliver(Duration::from_millis(10)));
    let second = receiver.pop().unwrap();
    assert_eq!(second.frames(), 480);
    assert_eq!(
        second.host_ns(),
        crate::recording::capture::MANUAL_HOST_BASE_NS + 20_000_000
    );
    assert_eq!(handle.frames_delivered(), 960 + 480);
}

#[test]
fn the_audio_is_a_function_of_the_frame_index() {
    let (handle, mut receiver, _source) = started(Duration::from_millis(10));
    handle.deliver_frames(100);
    let mut got = Vec::new();
    while let Some(block) = receiver.pop() {
        got.extend_from_slice(block.samples());
    }
    assert_eq!(got, handle.expected(0, 480 + 100));
}

#[test]
fn stall_and_overrun_make_audio_time_pass_without_delivering() {
    let (handle, mut receiver, _source) = started(Duration::from_millis(10));
    handle.stall(Duration::from_millis(600));
    handle.overrun(480);
    assert_eq!(handle.position(), 480 + 28_800 + 480);
    assert_eq!(handle.frames_dropped(), 480);
    handle.deliver_frames(10);
    let _prime = receiver.pop().unwrap();
    let after = receiver.pop().unwrap();
    assert_eq!(after.dropped_before(), 480);
    // 10 ms + 600 ms + 10 ms of clock before this block.
    assert_eq!(
        after.host_ns(),
        crate::recording::capture::MANUAL_HOST_BASE_NS + 620_000_000
    );
    assert_eq!(
        after.samples(),
        &handle.expected(480 + 28_800 + 480, 10)[..]
    );
}

#[test]
fn events_reach_the_receiver_as_signals() {
    let (handle, receiver, _source) = started(Duration::ZERO);
    handle.lose_device();
    handle.change_format(NativeFormat {
        sample_rate: 44_100,
        channels: 1,
    });
    let signals = receiver.take_signals();
    assert!(signals.device_lost);
    assert_eq!(signals.format_changed.unwrap().sample_rate, 44_100);
    handle.sleep(Duration::from_secs(3600));
    assert!(receiver.take_signals().system_sleep);
    assert_eq!(handle.clock().now_ns(), 3_600_000_000_000);
}

#[test]
fn a_stopped_source_ignores_deliveries() {
    let (mut source, handle) = FakeSource::new(MONO);
    let (sink, _receiver) = track_channel(
        MONO,
        ChannelConfig {
            block_frames: 64,
            blocks: 4,
        },
    );
    assert!(!handle.deliver(Duration::from_millis(1)), "not started");
    source.start(sink).unwrap();
    assert!(handle.running());
    source.stop();
    source.stop();
    assert!(!handle.running());
    assert!(!handle.deliver(Duration::from_millis(1)));
}
