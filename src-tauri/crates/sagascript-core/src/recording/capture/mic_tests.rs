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

// ---- Codex review of 1b, round 3: the identity belongs to the device that is opened ----

use std::cell::Cell;

use super::{
    check_stream_target, describe, id_from_debug_text, same_device, verify_default, InputDevice,
    OpenedInput,
};

/// A device as a value: its fields do not change when the system default does.
#[derive(Clone)]
struct FakeDevice {
    id: Option<u64>,
    name: &'static str,
    format: DeviceFormat,
}

impl InputDevice for FakeDevice {
    type Config = ();

    fn id(&self) -> Option<u64> {
        self.id
    }
    fn name(&self) -> Option<String> {
        Some(self.name.into())
    }
    fn default_format(&self) -> Option<(DeviceFormat, cpal::SampleFormat, ())> {
        Some((self.format, cpal::SampleFormat::F32, ()))
    }
}

fn usb(id: u64) -> FakeDevice {
    FakeDevice {
        id: Some(id),
        name: "USB Microphone",
        format: DeviceFormat {
            sample_rate: 48_000,
            channels: 1,
        },
    }
}

/// A system whose default input is switched by the caller, between the moment
/// the default device is handed out and anything else the caller reads.
struct SwitchingHost {
    devices: [FakeDevice; 2],
    default: Cell<usize>,
}

impl SwitchingHost {
    /// The default device now, and then the default is switched to the other.
    fn default_input_device(&self) -> Option<FakeDevice> {
        let device = self.devices[self.default.get()].clone();
        self.default.set(1 - self.default.get());
        Some(device)
    }

    /// What asking the system for "the current default" would say now: the
    /// separate read the first version of the identity relied on.
    fn current_default_id(&self) -> Option<u64> {
        self.devices[self.default.get()].id
    }
}

#[test]
fn the_identity_is_read_from_the_device_value_not_from_the_current_default() {
    // The default is A when the device is handed out and B right after.
    let host = SwitchingHost {
        devices: [usb(77), usb(41)],
        default: Cell::new(0),
    };
    let device = host.default_input_device().unwrap();
    // A separate "what is the default now" read would now say B (41)...
    assert_eq!(host.current_default_id(), Some(41));
    // ...but the snapshot of the device that was handed out says A.
    let opened = describe(&device).unwrap();
    assert_eq!(opened.snapshot.id, Some(77));
    assert_eq!(opened.snapshot.name.as_deref(), Some("USB Microphone"));
}

#[test]
fn a_start_on_a_device_that_replaced_the_expected_one_for_an_instant_is_refused() {
    // The recording was set up on B (41). While it starts, the default is A
    // (77: the same name, the same format) when the device is looked up, and
    // B again right after. Capture would open A. It is refused.
    let host = SwitchingHost {
        devices: [usb(77), usb(41)],
        default: Cell::new(0),
    };
    let expected = describe(&usb(41)).unwrap();
    let result = verify_default(
        host.default_input_device(),
        &expected.snapshot,
        expected.sample_format,
    );
    assert!(matches!(
        result,
        Err(RecordingError::SourceUnavailable { reason }) if reason.contains("changed")
    ));
    assert_eq!(
        host.current_default_id(),
        Some(41),
        "and the default is B again"
    );
}

#[test]
fn a_start_on_the_expected_device_is_accepted_and_returns_that_device() {
    let host = SwitchingHost {
        devices: [usb(41), usb(77)],
        default: Cell::new(0),
    };
    let expected = describe(&usb(41)).unwrap();
    let device = verify_default(
        host.default_input_device(),
        &expected.snapshot,
        expected.sample_format,
    )
    .unwrap();
    assert_eq!(device.0.id, Some(41));
}

#[test]
fn a_start_without_a_default_device_or_on_another_format_is_refused() {
    let expected = describe(&usb(41)).unwrap();
    assert!(
        verify_default::<FakeDevice>(None, &expected.snapshot, expected.sample_format).is_err()
    );
    let mut other = usb(41);
    other.format.sample_rate = 44_100;
    assert!(verify_default(Some(other), &expected.snapshot, expected.sample_format).is_err());
}

#[test]
fn the_stream_target_check_follows_the_same_identity_rules_as_the_watch() {
    let opened = |device: FakeDevice| -> OpenedInput<()> { describe(&device).unwrap() };
    let expected = opened(usb(41));
    let check = |device: FakeDevice| {
        check_stream_target(&expected.snapshot, expected.sample_format, &opened(device)).is_ok()
    };
    assert!(check(usb(41)));
    assert!(!check(usb(77)), "another id, the same name and format");
    // The same id under another name is the same device.
    let mut renamed = usb(41);
    renamed.name = "Desk Mic";
    assert!(check(renamed));
    // With no id on one side, the names decide.
    let mut unknown = usb(41);
    unknown.id = None;
    assert!(check(unknown.clone()));
    unknown.name = "Another Mic";
    assert!(!check(unknown));
    assert!(same_device(&expected.snapshot, &expected.snapshot));
}

#[test]
fn the_core_audio_id_is_found_in_cpals_debug_text_and_nothing_else_passes_for_one() {
    // The text `cpal` 0.15.3 prints for a CoreAudio device (pinned: a newer
    // `cpal` that prints something else yields no id, and names decide).
    let text = r#"Device { audio_device_id: 73, name: Ok("MacBook Pro Microphone") }"#;
    assert_eq!(id_from_debug_text(text), Some(73));
    // A name that looks like the field does not confuse it.
    let tricky = r#"Device { audio_device_id: 12, name: Ok("audio_device_id: 99") }"#;
    assert_eq!(id_from_debug_text(tricky), Some(12));
    for bad in [
        "",
        "Device { name: Ok(\"x\") }",
        "Device { audio_device_id: 0, name: Ok(\"x\") }",
        "Device { audio_device_id: -4, name: Ok(\"x\") }",
        "Device { audio_device_id: 99999999999, name: Ok(\"x\") }",
        "Device { audio_device_id: abc }",
        "Device { audio_device_id: }",
    ] {
        assert_eq!(id_from_debug_text(bad), None, "{bad:?}");
    }
}

/// Reads the default device's description; opens no stream and captures no
/// audio. Ignored so that no test run touches the audio hardware at all.
#[test]
#[ignore = "queries the audio hardware (no capture); run by hand on a Mac"]
fn the_default_input_device_of_a_mac_has_an_id_that_cpals_debug_text_gives() {
    use cpal::traits::HostTrait;
    let Some(device) = cpal::default_host().default_input_device() else {
        eprintln!("no default input device");
        return;
    };
    let Some(opened) = describe(&device) else {
        eprintln!("the default input device reports no format");
        return;
    };
    eprintln!("default input: {:?}", opened.snapshot);
    #[cfg(target_os = "macos")]
    assert!(opened.snapshot.id.is_some(), "cpal's Debug text changed?");
}

/// A device whose default configuration changes every time it is read: 2
/// channels the first time, 4 after that (the read number is the "config").
struct ChangingDevice {
    reads: Cell<u32>,
}

impl InputDevice for ChangingDevice {
    type Config = u32;

    fn id(&self) -> Option<u64> {
        Some(41)
    }
    fn name(&self) -> Option<String> {
        Some("Interface".into())
    }
    fn default_format(&self) -> Option<(DeviceFormat, cpal::SampleFormat, u32)> {
        let read = self.reads.get() + 1;
        self.reads.set(read);
        let channels = if read == 1 { 2 } else { 4 };
        Some((
            DeviceFormat {
                sample_rate: 48_000,
                channels,
            },
            cpal::SampleFormat::F32,
            read,
        ))
    }
}

fn interface_snapshot(channels: u16) -> DeviceSnapshot {
    DeviceSnapshot {
        id: Some(41),
        name: Some("Interface".into()),
        format: DeviceFormat {
            sample_rate: 48_000,
            channels,
        },
    }
}

#[test]
fn the_configuration_a_stream_is_built_with_is_the_one_that_was_checked() {
    // The device is 2 channels when it is checked and 4 on any later read.
    let device = ChangingDevice {
        reads: Cell::new(0),
    };
    let (device, config) = verify_default(
        Some(device),
        &interface_snapshot(2),
        cpal::SampleFormat::F32,
    )
    .unwrap();
    assert_eq!(config, 1, "the configuration of the read that was checked");
    assert_eq!(device.reads.get(), 1, "and it is the only read there is");
}

#[test]
fn a_device_that_already_has_the_new_configuration_when_checked_is_refused() {
    // Expecting 2 channels, the first read already says 4.
    let device = ChangingDevice {
        reads: Cell::new(1),
    };
    assert!(verify_default(
        Some(device),
        &interface_snapshot(2),
        cpal::SampleFormat::F32
    )
    .is_err());
}
