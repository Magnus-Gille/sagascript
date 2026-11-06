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
