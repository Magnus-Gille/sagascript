use super::*;

fn mono(rate: u32) -> NativeFormat {
    NativeFormat {
        sample_rate: rate,
        channels: 1,
    }
}

fn stereo(rate: u32) -> NativeFormat {
    NativeFormat {
        sample_rate: rate,
        channels: 2,
    }
}

fn pool(blocks: usize, block_frames: usize) -> ChannelConfig {
    ChannelConfig {
        block_frames,
        blocks,
    }
}

#[test]
fn a_callback_becomes_a_block_with_its_host_stamp() {
    let (mut sink, mut rx) = track_channel(mono(48_000), pool(4, 512));
    sink.push(&[0.1, 0.2, 0.3], 1_000);
    let block = rx.pop().expect("a block");
    assert_eq!(block.samples(), &[0.1, 0.2, 0.3]);
    assert_eq!(block.frames(), 3);
    assert_eq!(block.host_ns(), 1_000);
    assert_eq!(block.dropped_before(), 0);
    rx.recycle(block);
    assert!(rx.pop().is_none());
}

#[test]
fn a_callback_larger_than_a_block_is_split_and_each_part_is_stamped() {
    let (mut sink, mut rx) = track_channel(mono(48_000), pool(8, 4));
    let samples: Vec<f32> = (0..10).map(|i| i as f32 / 100.0).collect();
    sink.push(&samples, 5_000_000);
    let mut frames = Vec::new();
    let mut stamps = Vec::new();
    while let Some(block) = rx.pop() {
        stamps.push(block.host_ns());
        frames.extend_from_slice(block.samples());
        rx.recycle(block);
    }
    assert_eq!(frames, samples);
    // 4 frames at 48 kHz are 83_333 ns (rounded down).
    assert_eq!(stamps, vec![5_000_000, 5_083_333, 5_166_666]);
}

#[test]
fn an_empty_pool_drops_frames_and_the_next_block_says_how_many() {
    let (mut sink, mut rx) = track_channel(mono(48_000), pool(2, 4));
    // The pool is two blocks of four frames: eight frames fit, the rest do not.
    sink.push(&[0.5; 12], 0);
    assert_eq!(rx.dropped_total(), 4);
    let first = rx.pop().unwrap();
    let second = rx.pop().unwrap();
    assert!(rx.pop().is_none());
    assert_eq!(first.dropped_before() + second.dropped_before(), 0);
    rx.recycle(first);
    rx.recycle(second);

    // Space again: the block that gets through carries the count.
    sink.push(&[0.25; 3], 777);
    let after = rx.pop().unwrap();
    assert_eq!(after.dropped_before(), 4);
    assert_eq!(after.samples(), &[0.25; 3]);
    rx.recycle(after);
    sink.push(&[0.25; 3], 778);
    assert_eq!(rx.pop().unwrap().dropped_before(), 0, "reported once");
}

#[test]
fn injected_drops_are_counted_like_natural_ones() {
    let (mut sink, mut rx) = track_channel(mono(48_000), pool(4, 64));
    sink.drop_frames(480);
    sink.push(&[0.0; 10], 0);
    let block = rx.pop().unwrap();
    assert_eq!(block.dropped_before(), 480);
    assert_eq!(rx.dropped_total(), 480);
}

#[test]
fn frames_dropped_with_nothing_after_them_are_reported_when_the_sink_goes() {
    let (mut sink, rx) = track_channel(mono(48_000), pool(1, 4));
    assert_eq!(rx.closed(), None);
    sink.push(&[0.0; 4], 0);
    sink.push(&[0.0; 6], 1);
    drop(sink);
    assert_eq!(rx.closed(), Some(6));
}

#[test]
fn peaks_are_the_largest_absolute_sample_per_channel_and_reset_when_taken() {
    let (mut sink, rx) = track_channel(stereo(48_000), pool(4, 8));
    sink.push(&[0.1, -0.7, -0.4, 0.2, 0.3, f32::NAN], 0);
    assert_eq!(rx.take_peaks(), [0.4, 0.7]);
    assert_eq!(rx.take_peaks(), [0.0, 0.0]);
}

#[test]
fn a_channel_pair_is_picked_out_of_a_wider_device() {
    let (mut sink, mut rx) = track_channel(stereo(44_100), pool(2, 8));
    // Two frames of a four-channel device; keep channels 2 and 3.
    sink.push_pair(&[1., 2., 3., 4., 5., 6., 7., 8.], 4, (2, 3), 0);
    assert_eq!(rx.pop().unwrap().samples(), &[3., 4., 7., 8.]);
    let (mut mono_sink, mut mono_rx) = track_channel(mono(44_100), pool(2, 8));
    mono_sink.push_pair(&[1., 2., 3., 4., 5., 6., 7., 8.], 4, (1, 0), 0);
    assert_eq!(mono_rx.pop().unwrap().samples(), &[2., 6.]);
}

#[test]
fn signals_are_taken_once_and_a_format_change_carries_the_new_format() {
    let (sink, rx) = track_channel(mono(48_000), pool(2, 8));
    let handle = sink.handle();
    assert!(!rx.take_signals().any());
    handle.signal(SourceSignal::DeviceLost);
    handle.signal(SourceSignal::FormatChanged(stereo(44_100)));
    let signals = rx.take_signals();
    assert!(signals.device_lost);
    assert!(!signals.device_changed);
    assert_eq!(signals.format_changed, Some(stereo(44_100)));
    assert!(!rx.take_signals().any());
    handle.signal(SourceSignal::SystemSleep);
    handle.signal(SourceSignal::DeviceChanged);
    let signals = rx.take_signals();
    assert!(signals.system_sleep && signals.device_changed);
}

#[test]
fn the_ends_are_send_and_the_handle_is_send_and_sync() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<BlockSink>();
    send::<TrackReceiver>();
    send_sync::<SignalHandle>();
}
