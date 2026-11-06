use std::io::Cursor;

use super::*;
use crate::recording::flac::FlacEncoder;
use crate::recording::test_signals::*;

fn params() -> StreamParams {
    StreamParams {
        sample_rate: 48_000,
        channels: 1,
        bits: 24,
    }
}

fn totals() -> StreamTotals {
    StreamTotals {
        total_samples: 123_456,
        md5: [7; 16],
        min_frame_size: 1000,
        max_frame_size: 9000,
    }
}

fn some_points() -> Vec<SeekPoint> {
    (0..5u64)
        .map(|i| SeekPoint {
            sample: i * 4096 * 118,
            offset: i * 1_000_000,
            samples: 4096,
        })
        .collect()
}

#[test]
fn the_crcs_match_the_published_check_values() {
    // CRC-8 poly 0x07 (SMBus) and CRC-16/UMTS (poly 0x8005, init 0, no reflection).
    assert_eq!(crc8(b"123456789"), 0xF4);
    assert_eq!(crc16(b"123456789"), 0xFEE8);
    assert_eq!(crc8(b""), 0);
    assert_eq!(crc16(b""), 0);
}

#[test]
fn a_crc16_of_the_data_followed_by_its_crc_is_zero() {
    let data = b"some frame bytes \x00\xff\x10";
    let mut framed = data.to_vec();
    framed.extend_from_slice(&crc16(data).to_be_bytes());
    assert_eq!(crc16(&framed), 0);
}

#[test]
fn the_header_round_trips_through_flacenc_and_the_parser() {
    let points = some_points();
    let bytes = build_header(params(), Some(&totals()), &points, RESERVED_LEN).unwrap();
    assert_eq!(bytes.len(), HEADER_LEN);
    let parsed = parse_header(&bytes).unwrap();
    assert_eq!(parsed.params, params());
    assert_eq!(parsed.total_samples, 123_456);
    assert_eq!(parsed.md5, [7; 16]);
    assert_eq!((parsed.min_block_size, parsed.max_block_size), (4096, 4096));
    assert_eq!((parsed.min_frame_size, parsed.max_frame_size), (1000, 9000));
    assert_eq!(parsed.first_frame, HEADER_LEN);
    assert_eq!(parsed.region_len(), RESERVED_LEN);
    assert_eq!(&parsed.streaminfo[..], &bytes[8..42]);
    // The file starts with the magic and a STREAMINFO block that is not last.
    assert_eq!(&bytes[..4], b"fLaC");
    assert_eq!(bytes[4], 0x00);
}

#[test]
fn the_placeholder_has_no_totals_and_all_of_the_region_is_padding() {
    let bytes = build_header(params(), None, &[], RESERVED_LEN).unwrap();
    let parsed = parse_header(&bytes).unwrap();
    assert_eq!((parsed.total_samples, parsed.md5), (0, [0; 16]));
    assert_eq!((parsed.min_frame_size, parsed.max_frame_size), (0, 0));
    assert_eq!(bytes[42], 0x81, "a last PADDING block");
}

#[test]
fn any_region_size_from_eight_bytes_up_keeps_the_layout() {
    for region in [8usize, 9, 25, 26, 100, 1000] {
        let bytes = build_header(params(), Some(&totals()), &some_points(), region).unwrap();
        let parsed = parse_header(&bytes).unwrap();
        assert_eq!(parsed.region_len(), region);
        assert_eq!(bytes.len(), RESERVED_START + region);
    }
    // Room for two whole points only: the table is cut to fit.
    let bytes = build_header(
        params(),
        Some(&totals()),
        &some_points(),
        8 + 2 * SEEK_POINT_LEN,
    )
    .unwrap();
    assert_eq!(bytes[RESERVED_START + 1..RESERVED_START + 4], [0, 0, 36]);
    assert_eq!(
        parse_header(&bytes).unwrap().region_len(),
        8 + 2 * SEEK_POINT_LEN
    );
}

#[test]
fn a_region_that_cannot_hold_two_block_headers_is_refused_unless_it_is_empty() {
    for region in 1..8 {
        assert!(matches!(
            build_header(params(), Some(&totals()), &[], region),
            Err(RecordingError::CorruptTrack { .. })
        ));
    }
    let bytes = build_header(params(), Some(&totals()), &[], 0).unwrap();
    assert_eq!(bytes.len(), RESERVED_START);
    assert_eq!(parse_header(&bytes).unwrap().first_frame, RESERVED_START);
}

#[test]
fn a_track_longer_than_flac_can_describe_is_refused() {
    let mut too_long = totals();
    too_long.total_samples = 1 << 36;
    assert!(matches!(
        build_header(params(), Some(&too_long), &[], RESERVED_LEN),
        Err(RecordingError::UnsupportedFormat { .. })
    ));
    too_long.total_samples = (1 << 36) - 1;
    let bytes = build_header(params(), Some(&too_long), &[], RESERVED_LEN).unwrap();
    assert_eq!(parse_header(&bytes).unwrap().total_samples, (1 << 36) - 1);
}

#[test]
fn every_prefix_of_a_header_is_truncated_never_accepted() {
    let bytes = build_header(params(), Some(&totals()), &some_points(), RESERVED_LEN).unwrap();
    for cut in (0..bytes.len()).step_by(7).chain([bytes.len() - 1]) {
        assert_eq!(
            parse_header(&bytes[..cut]),
            Err(HeaderError::Truncated),
            "cut at {cut}"
        );
    }
}

#[test]
fn bytes_that_are_not_a_flac_header_are_not_flac() {
    assert_eq!(parse_header(b"RIFF....WAVE"), Err(HeaderError::NotFlac));
    assert_eq!(parse_header(b"ID3\x04"), Err(HeaderError::NotFlac));
    // STREAMINFO of the wrong length.
    let mut bytes = build_header(params(), None, &[], RESERVED_LEN).unwrap();
    bytes[7] = 35;
    assert_eq!(parse_header(&bytes), Err(HeaderError::NotFlac));
    // The first block is not STREAMINFO.
    let mut bytes = build_header(params(), None, &[], RESERVED_LEN).unwrap();
    bytes[4] = 0x01;
    assert_eq!(parse_header(&bytes), Err(HeaderError::NotFlac));
}

fn first_frames() -> Vec<u8> {
    let pcm = at_bits(&speechlike(4096 + 300, 3), 24);
    let mut encoder = FlacEncoder::new(Cursor::new(Vec::new()), 48_000, 1, 24).unwrap();
    encoder.push_pcm(&pcm).unwrap();
    encoder.finish_into_parts().unwrap().sink.into_inner()[HEADER_LEN..].to_vec()
}

#[test]
fn a_frame_header_is_parsed_and_every_prefix_needs_more() {
    let frames = first_frames();
    let Parse::Ok(header) = parse_frame_header(&frames, params()) else {
        panic!("a frame header");
    };
    assert_eq!(header.block_size, 4096);
    assert_eq!(header.frame_number, 0);
    assert_eq!(header.assignment, ChannelAssignment::Independent(1));
    for cut in 0..header.len {
        assert!(
            matches!(
                parse_frame_header(&frames[..cut], params()),
                Parse::NeedMore | Parse::Invalid
            ),
            "a prefix of {cut} bytes is not a header"
        );
    }
}

#[test]
fn a_frame_header_of_another_stream_or_with_a_bad_crc_is_invalid() {
    let frames = first_frames();
    let Parse::Ok(header) = parse_frame_header(&frames, params()) else {
        panic!("a frame header");
    };
    // The same bytes against another stream.
    for other in [
        StreamParams {
            channels: 2,
            ..params()
        },
        StreamParams {
            bits: 16,
            ..params()
        },
        StreamParams {
            sample_rate: 44_100,
            ..params()
        },
    ] {
        assert_eq!(parse_frame_header(&frames, other), Parse::Invalid);
    }
    // Any change inside the header, its CRC included, is detected.
    for byte in 0..header.len {
        for bit in 0..8 {
            let mut changed = frames[..header.len].to_vec();
            changed[byte] ^= 1 << bit;
            assert!(
                !matches!(parse_frame_header(&changed, params()), Parse::Ok(_)),
                "flipping bit {bit} of header byte {byte} was not noticed"
            );
        }
    }
    // A variable-block-size frame is not one of ours.
    let mut variable = frames[..header.len].to_vec();
    variable[1] = 0xF9;
    assert_eq!(parse_frame_header(&variable, params()), Parse::Invalid);
}

#[test]
fn channel_assignments_name_their_channels_and_widths() {
    assert_eq!(ChannelAssignment::Independent(2).channels(), 2);
    assert_eq!(ChannelAssignment::MidSide.channels(), 2);
    // The side channel is one bit wider.
    assert_eq!(ChannelAssignment::LeftSide.subframe_bits(0, 24), 24);
    assert_eq!(ChannelAssignment::LeftSide.subframe_bits(1, 24), 25);
    assert_eq!(ChannelAssignment::SideRight.subframe_bits(0, 24), 25);
    assert_eq!(ChannelAssignment::SideRight.subframe_bits(1, 24), 24);
    assert_eq!(ChannelAssignment::MidSide.subframe_bits(1, 16), 17);
    assert_eq!(ChannelAssignment::Independent(2).subframe_bits(1, 16), 16);
}

#[test]
fn encoding_params_are_checked() {
    assert!(params().validate_for_encoding().is_ok());
    for bad in [
        StreamParams {
            sample_rate: 0,
            ..params()
        },
        StreamParams {
            sample_rate: 655_351,
            ..params()
        },
        StreamParams {
            channels: 0,
            ..params()
        },
        StreamParams {
            channels: 3,
            ..params()
        },
        StreamParams {
            bits: 8,
            ..params()
        },
        StreamParams {
            bits: 32,
            ..params()
        },
    ] {
        assert!(bad.validate_for_encoding().is_err(), "{bad:?}");
    }
}
