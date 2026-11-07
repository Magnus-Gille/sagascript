//! Format negotiation only. No test here opens an input stream or touches the
//! microphone (SAGA-563): `MicSource::open` and `start` are for the owner's
//! hardware checklist.

use super::*;

fn device(sample_rate: u32, channels: u16) -> DeviceFormat {
    DeviceFormat {
        sample_rate,
        channels,
    }
}

#[test]
fn a_mono_device_is_stored_as_mono_whatever_the_pair() {
    for pair in [(1, 2), (1, 1), (3, 4)] {
        let n = negotiate(device(48_000, 1), pair).unwrap();
        assert_eq!(
            n.format,
            NativeFormat {
                sample_rate: 48_000,
                channels: 1
            }
        );
        assert_eq!(n.pair, (0, 0));
        assert_eq!(n.device_channels, 1);
    }
}

#[test]
fn a_stereo_device_is_stored_as_stereo_by_default() {
    let n = negotiate(device(44_100, 2), MicConfig::default().channel_pair).unwrap();
    assert_eq!(
        n.format,
        NativeFormat {
            sample_rate: 44_100,
            channels: 2
        }
    );
    assert_eq!(n.pair, (0, 1));
}

#[test]
fn a_wide_interface_keeps_the_pair_asked_for_counted_from_one() {
    let default = negotiate(device(96_000, 8), (1, 2)).unwrap();
    assert_eq!((default.pair, default.format.channels), ((0, 1), 2));
    assert_eq!(default.device_channels, 8);
    let third_and_fourth = negotiate(device(96_000, 8), (3, 4)).unwrap();
    assert_eq!(third_and_fourth.pair, (2, 3));
    let reversed = negotiate(device(96_000, 8), (8, 1)).unwrap();
    assert_eq!(reversed.pair, (7, 0));
}

#[test]
fn the_same_channel_twice_is_mono_of_that_channel() {
    let n = negotiate(device(48_000, 4), (3, 3)).unwrap();
    assert_eq!(n.format.channels, 1);
    assert_eq!(n.pair, (2, 2));
}

#[test]
fn a_pair_outside_the_device_or_a_device_without_a_format_is_refused() {
    assert!(matches!(
        negotiate(device(48_000, 2), (1, 3)),
        Err(RecordingError::InvalidInput { .. })
    ));
    assert!(matches!(
        negotiate(device(48_000, 4), (0, 1)),
        Err(RecordingError::InvalidInput { .. })
    ));
    assert!(matches!(
        negotiate(device(0, 2), (1, 2)),
        Err(RecordingError::SourceUnavailable { .. })
    ));
    assert!(matches!(
        negotiate(device(48_000, 0), (1, 2)),
        Err(RecordingError::SourceUnavailable { .. })
    ));
}

#[test]
fn the_source_can_move_to_the_thread_that_owns_it() {
    fn is_send<T: Send>() {}
    is_send::<MicSource>();
}

// ---- Codex review of 1b, finding 6: changes cpal does not report --------------

struct Scripted(
    std::collections::VecDeque<Option<DeviceSnapshot>>,
    Option<DeviceSnapshot>,
);

impl DeviceProbe for Scripted {
    fn default_input(&mut self) -> Option<DeviceSnapshot> {
        match self.0.pop_front() {
            Some(next) => {
                self.1 = next.clone();
                next
            }
            None => self.1.clone(),
        }
    }
}

fn snapshot(name: &str, sample_rate: u32, channels: u16) -> DeviceSnapshot {
    DeviceSnapshot {
        id: None,
        name: Some(name.into()),
        format: DeviceFormat {
            sample_rate,
            channels,
        },
    }
}

fn watch(script: Vec<Option<DeviceSnapshot>>) -> DeviceWatch {
    DeviceWatch::new(
        Box::new(Scripted(script.into(), None)),
        snapshot("Built-in Microphone", 48_000, 1),
        (1, 2),
    )
}

#[test]
fn the_same_device_and_format_is_not_a_change() {
    let mut w = watch(vec![Some(snapshot("Built-in Microphone", 48_000, 1)); 5]);
    for _ in 0..5 {
        assert_eq!(w.check(), None);
    }
}

#[test]
fn another_default_device_ends_the_track_after_two_looks() {
    let other = snapshot("AirPods", 24_000, 1);
    let mut w = watch(vec![Some(other.clone()), Some(other)]);
    assert_eq!(w.check(), None, "one look is not enough");
    assert_eq!(w.check(), Some(SourceSignal::DeviceChanged));
    assert_eq!(w.check(), None, "reported once");
}

#[test]
fn a_new_rate_or_channel_count_on_the_same_device_is_a_format_change() {
    let mut w = watch(vec![Some(snapshot("Built-in Microphone", 44_100, 1)); 2]);
    assert_eq!(w.check(), None);
    assert_eq!(
        w.check(),
        Some(SourceSignal::FormatChanged(NativeFormat {
            sample_rate: 44_100,
            channels: 1
        }))
    );
    let mut w = watch(vec![Some(snapshot("Built-in Microphone", 48_000, 2)); 2]);
    w.check();
    assert_eq!(
        w.check(),
        Some(SourceSignal::FormatChanged(NativeFormat {
            sample_rate: 48_000,
            channels: 2
        }))
    );
}

#[test]
fn a_device_that_vanishes_is_a_device_change() {
    let mut w = watch(vec![None, None]);
    assert_eq!(w.check(), None);
    assert_eq!(w.check(), Some(SourceSignal::DeviceChanged));
}

#[test]
fn a_blip_that_does_not_repeat_is_ignored() {
    let back = snapshot("Built-in Microphone", 48_000, 1);
    let mut w = watch(vec![
        Some(snapshot("AirPods", 24_000, 1)),
        Some(back.clone()),
        None,
        Some(back),
    ]);
    for _ in 0..4 {
        assert_eq!(w.check(), None);
    }
}

#[test]
fn an_unknown_name_still_catches_a_format_change_but_not_a_swap() {
    let mut w = DeviceWatch::new(
        Box::new(Scripted(
            vec![Some(snapshot("Whatever", 48_000, 1)); 2].into(),
            None,
        )),
        DeviceSnapshot {
            id: None,
            name: None,
            format: DeviceFormat {
                sample_rate: 48_000,
                channels: 1,
            },
        },
        (1, 2),
    );
    assert_eq!(w.check(), None);
    assert_eq!(
        w.check(),
        None,
        "no name to compare, same format: nothing to see"
    );
}

#[test]
fn a_change_reaches_the_writer_as_the_signal_that_ends_the_track() {
    use crate::recording::capture::{track_channel, ChannelConfig};
    let (sink, receiver) = track_channel(
        NativeFormat {
            sample_rate: 48_000,
            channels: 1,
        },
        ChannelConfig {
            block_frames: 64,
            blocks: 4,
        },
    );
    let other = snapshot("AirPods", 24_000, 1);
    let mut w = watch(vec![Some(other.clone()), Some(other)]);
    // What the capture thread does with each look.
    let handle = sink.handle();
    for _ in 0..2 {
        if let Some(signal) = w.check() {
            handle.signal(signal);
        }
    }
    assert!(receiver.take_signals().device_changed);
}

fn with_id(id: u64, mut snapshot: DeviceSnapshot) -> DeviceSnapshot {
    snapshot.id = Some(id);
    snapshot
}

#[test]
fn two_devices_with_the_same_name_and_format_are_told_apart_by_their_identity() {
    let opened = with_id(41, snapshot("USB Microphone", 48_000, 1));
    let other = with_id(77, snapshot("USB Microphone", 48_000, 1));
    let mut w = DeviceWatch::new(
        Box::new(Scripted(
            vec![Some(other.clone()), Some(other)].into(),
            None,
        )),
        opened,
        (1, 2),
    );
    assert_eq!(w.check(), None);
    assert_eq!(w.check(), Some(SourceSignal::DeviceChanged));
}

#[test]
fn the_same_identity_is_the_same_device_even_if_its_name_changed() {
    let opened = with_id(41, snapshot("USB Microphone", 48_000, 1));
    let renamed = with_id(41, snapshot("Desk Mic", 48_000, 1));
    let mut w = DeviceWatch::new(
        Box::new(Scripted(vec![Some(renamed); 3].into(), None)),
        opened,
        (1, 2),
    );
    for _ in 0..3 {
        assert_eq!(w.check(), None);
    }
}

#[test]
fn without_an_identity_on_both_sides_the_name_is_all_there_is() {
    // The documented limit: two devices that are not told apart by anything but
    // a name that is the same are not told apart.
    let opened = with_id(41, snapshot("USB Microphone", 48_000, 1));
    let same_name_no_id = snapshot("USB Microphone", 48_000, 1);
    let mut w = DeviceWatch::new(
        Box::new(Scripted(vec![Some(same_name_no_id); 3].into(), None)),
        opened,
        (1, 2),
    );
    for _ in 0..3 {
        assert_eq!(w.check(), None);
    }
}

#[test]
fn a_device_that_goes_from_two_to_four_channels_is_a_format_change_although_the_pair_stays_stereo()
{
    let mut w = DeviceWatch::new(
        Box::new(Scripted(
            vec![Some(snapshot("Interface", 48_000, 4)); 2].into(),
            None,
        )),
        snapshot("Interface", 48_000, 2),
        (1, 2),
    );
    assert_eq!(w.check(), None);
    // The track format would still be 48 kHz stereo: the signal is raised
    // all the same, and the session believes it.
    assert_eq!(
        w.check(),
        Some(SourceSignal::FormatChanged(NativeFormat {
            sample_rate: 48_000,
            channels: 2
        }))
    );
}
