use std::io::{self, Cursor, Read};

use super::*;
use crate::recording::flac::format::HEADER_LEN;
use crate::recording::flac::FlacEncoder;
use crate::recording::test_signals::*;

fn mono() -> StreamParams {
    StreamParams {
        sample_rate: 48_000,
        channels: 1,
        bits: 24,
    }
}

fn stereo() -> StreamParams {
    StreamParams {
        sample_rate: 48_000,
        channels: 2,
        bits: 24,
    }
}

/// The frame bytes of an encoded file and the length of each frame.
fn frames_of(params: StreamParams, pcm: &[i32]) -> (Vec<u8>, Vec<u32>) {
    let mut encoder = FlacEncoder::new(
        Cursor::new(Vec::new()),
        params.sample_rate,
        params.channels,
        params.bits,
    )
    .unwrap()
    .with_frame_log();
    encoder.push_pcm(pcm).unwrap();
    let parts = encoder.finish_into_parts().unwrap();
    (
        parts.sink.into_inner()[HEADER_LEN..].to_vec(),
        parts.frame_lengths.unwrap(),
    )
}

fn scan(bytes: &[u8], params: StreamParams) -> (ScanSummary, Vec<usize>) {
    let mut seen = Vec::new();
    let summary = scan_frames(&mut Cursor::new(bytes), 100, params, &mut |frame| {
        seen.push(frame.bytes.len());
        true
    })
    .unwrap();
    (summary, seen)
}

#[test]
fn no_strict_prefix_of_a_frame_is_taken_for_a_frame() {
    let pcm = at_bits(&speechlike(300, 5), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    assert_eq!(lengths.len(), 1);
    assert_eq!(
        walk_frame(&frames, mono(), 0),
        Walk::Frame(FrameEnd {
            len: frames.len(),
            block_size: 300
        })
    );
    for cut in 0..frames.len() {
        assert!(
            !matches!(walk_frame(&frames[..cut], mono(), 0), Walk::Frame(_)),
            "a prefix of {cut} of {} bytes was accepted",
            frames.len()
        );
    }
}

#[test]
fn no_single_bit_error_in_a_frame_goes_unnoticed() {
    for (params, pcm) in [
        (mono(), at_bits(&speechlike(250, 6), 24)),
        (stereo(), {
            let first = at_bits(&speechlike(250, 7), 24);
            let second = second_channel(&first);
            interleave(&[&first, &second])
        }),
    ] {
        let (frames, _) = frames_of(params, &pcm);
        for byte in 0..frames.len() {
            for bit in 0..8 {
                let mut changed = frames.clone();
                changed[byte] ^= 1 << bit;
                assert!(
                    !matches!(walk_frame(&changed, params, 0), Walk::Frame(_)),
                    "bit {bit} of byte {byte} flipped without notice"
                );
            }
        }
    }
}

#[test]
fn a_frame_must_carry_the_number_that_is_expected() {
    let pcm = at_bits(&speechlike(4096 * 2, 8), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    let second = &frames[lengths[0] as usize..];
    assert!(matches!(walk_frame(second, mono(), 1), Walk::Frame(_)));
    assert_eq!(walk_frame(second, mono(), 0), Walk::Invalid);
    assert_eq!(walk_frame(second, mono(), 2), Walk::Invalid);
}

#[test]
fn a_scan_ends_at_the_first_frame_that_is_not_valid() {
    let pcm = at_bits(&speechlike(4096 * 5, 9), 24);
    let (mut frames, lengths) = frames_of(mono(), &pcm);
    let start_of_third: u32 = lengths[..2].iter().sum();
    frames[start_of_third as usize + 20] ^= 0x10;
    let (summary, seen) = scan(&frames, mono());
    assert_eq!(seen, vec![lengths[0] as usize, lengths[1] as usize]);
    assert_eq!(summary.frames, 2);
    assert_eq!(summary.samples, 2 * 4096);
    assert_eq!(
        summary.end_offset,
        100 + u64::from(start_of_third),
        "offsets count from the start offset"
    );
}

#[test]
fn a_scan_of_a_clean_stream_sees_every_frame_and_ends_at_its_end() {
    let pcm = at_bits(&speechlike(4096 * 3 + 17, 10), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    let (summary, seen) = scan(&frames, mono());
    assert_eq!(
        seen,
        lengths.iter().map(|l| *l as usize).collect::<Vec<_>>()
    );
    assert_eq!(summary.samples, 4096 * 3 + 17);
    assert_eq!(summary.end_offset, 100 + frames.len() as u64);
}

#[test]
fn nothing_may_follow_a_short_frame() {
    let pcm = at_bits(&speechlike(4096 + 300, 11), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    // A valid-looking frame number 2 after the short frame 1.
    let longer = at_bits(&speechlike(4096 * 3, 11), 24);
    let (other, other_lengths) = frames_of(mono(), &longer);
    let third_start: u32 = other_lengths[..2].iter().sum();
    let mut joined = frames.clone();
    joined.extend_from_slice(&other[third_start as usize..]);
    let (summary, seen) = scan(&joined, mono());
    assert_eq!(seen.len(), 2, "the two frames of the short stream");
    assert_eq!(
        summary.end_offset,
        100 + lengths.iter().map(|l| u64::from(*l)).sum::<u64>()
    );
}

#[test]
fn a_scan_stops_where_the_callback_refuses() {
    let pcm = at_bits(&speechlike(4096 * 4, 12), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    let mut count = 0;
    let summary = scan_frames(&mut Cursor::new(&frames), 0, mono(), &mut |_| {
        count += 1;
        count <= 2
    })
    .unwrap();
    assert_eq!(summary.frames, 2);
    assert_eq!(summary.end_offset, u64::from(lengths[0] + lengths[1]));
}

/// A reader that gives a few bytes per call, as a slow device does.
struct Trickle<R>(R, usize);

impl<R: Read> Read for Trickle<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let limit = buf.len().min(self.1);
        self.0.read(&mut buf[..limit])
    }
}

#[test]
fn the_read_pattern_does_not_change_what_a_scan_finds() {
    let pcm = at_bits(&edge_signal(Edge::WhiteFullScale, 4096 * 40), 24);
    let (frames, lengths) = frames_of(mono(), &pcm);
    assert!(
        frames.len() > 256 * 1024,
        "the stream is longer than one read"
    );
    let expected: Vec<usize> = lengths.iter().map(|l| *l as usize).collect();
    for step in [1usize, 7, 1000, 65_536, usize::MAX] {
        let mut seen = Vec::new();
        let summary = scan_frames(
            &mut Trickle(Cursor::new(&frames), step),
            0,
            mono(),
            &mut |frame| {
                seen.push(frame.bytes.len());
                true
            },
        )
        .unwrap();
        assert_eq!(seen, expected, "read size {step}");
        assert_eq!(summary.end_offset, frames.len() as u64);
    }
}

#[test]
fn arbitrary_bytes_never_panic_and_never_overrun() {
    let pcm = at_bits(&speechlike(4096 * 2 + 900, 13), 24);
    let (frames, _) = frames_of(mono(), &pcm);
    let mut rng = Rng::new(0xC0FFEE);
    // Random bytes.
    for _ in 0..300 {
        let length = rng.range(0, 6000) as usize;
        let noise: Vec<u8> = (0..length).map(|_| rng.next_u64() as u8).collect();
        let (summary, _) = scan(&noise, mono());
        assert_eq!(
            (summary.frames, summary.end_offset),
            (0, 100),
            "noise is not a frame"
        );
    }
    // A valid stream with a few random bytes changed or cut off.
    for _ in 0..600 {
        let mut changed = frames.clone();
        for _ in 0..rng.range(0, 4) {
            let at = rng.range(0, changed.len() as i64 - 1) as usize;
            changed[at] = rng.next_u64() as u8;
        }
        if rng.range(0, 1) == 1 {
            changed.truncate(rng.range(0, changed.len() as i64) as usize);
        }
        let (summary, seen) = scan(&changed, mono());
        assert!(summary.end_offset - 100 <= changed.len() as u64);
        assert_eq!(
            summary.end_offset - 100,
            seen.iter().map(|l| *l as u64).sum::<u64>()
        );
        assert_eq!(summary.frames as usize, seen.len());
    }
}
