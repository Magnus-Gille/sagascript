use std::io::Cursor;

use super::format::{parse_header, ParsedHeader, SEEK_POINT_LEN};
use super::scan::{scan_frames, walk_frame, Walk};
use super::*;
use crate::recording::test_signals::*;

struct Encoded {
    bytes: Vec<u8>,
    summary: TrackSummary,
    frame_lengths: Vec<u32>,
}

fn encoder(rate: u32, channels: u8, bits: u8) -> FlacEncoder<Cursor<Vec<u8>>> {
    FlacEncoder::new(Cursor::new(Vec::new()), rate, channels, bits)
        .unwrap()
        .with_frame_log()
}

fn finish(encoder: FlacEncoder<Cursor<Vec<u8>>>) -> Encoded {
    let parts = encoder.finish_into_parts().unwrap();
    Encoded {
        bytes: parts.sink.into_inner(),
        summary: parts.summary,
        frame_lengths: parts.frame_lengths.unwrap(),
    }
}

/// Encode interleaved integer samples in one push.
fn encode(rate: u32, channels: u8, bits: u8, pcm: &[i32]) -> Encoded {
    let mut encoder = encoder(rate, channels, bits);
    encoder.push_pcm(pcm).unwrap();
    finish(encoder)
}

fn header_of(file: &Encoded) -> ParsedHeader {
    parse_header(&file.bytes).expect("a header")
}

/// The metadata blocks after "fLaC": (type, last flag, body length).
fn metadata_blocks(bytes: &[u8]) -> Vec<(u8, bool, usize)> {
    let mut blocks = Vec::new();
    let mut position = 4;
    loop {
        let flags = bytes[position];
        let length = (usize::from(bytes[position + 1]) << 16)
            | (usize::from(bytes[position + 2]) << 8)
            | usize::from(bytes[position + 3]);
        blocks.push((flags & 0x7F, flags & 0x80 != 0, length));
        position += 4 + length;
        if flags & 0x80 != 0 {
            return blocks;
        }
    }
}

fn params(rate: u32, channels: u8, bits: u8) -> StreamParams {
    StreamParams {
        sample_rate: rate,
        channels,
        bits,
    }
}

/// Everything the format promises about one encoded file, checked with a
/// decoder that is not the code under test.
fn assert_faithful(file: &Encoded, rate: u32, channels: u8, bits: u8, pcm: &[i32]) {
    // symphonia cannot open a FLAC file that has no frame at all.
    let decoded = (!pcm.is_empty()).then(|| decode_flac(&file.bytes));
    if let Some(decoded) = &decoded {
        assert_eq!(decoded.errors, 0, "the decoder skipped a frame");
        assert_eq!(decoded.sample_rate, rate);
        assert_eq!(decoded.channels, usize::from(channels));
        assert_eq!(decoded.samples.len(), pcm.len(), "sample count");
        assert!(decoded.samples == pcm, "samples differ from the source");
        assert_eq!(
            decoded.md5_verified,
            Some(true),
            "the decoder checked the MD5"
        );
    }

    let header = header_of(file);
    let frames = (pcm.len() / usize::from(channels)) as u64;
    assert_eq!(header.params, params(rate, channels, bits));
    assert_eq!(header.total_samples, frames);
    assert_eq!(file.summary.frames, frames);
    assert_eq!(header.md5, pcm_md5(pcm, bits, channels), "STREAMINFO MD5");
    // Fixed-size blocks: min = max = 4096 even when the last block is short.
    assert_eq!((header.min_block_size, header.max_block_size), (4096, 4096));
    assert_eq!(
        header.max_frame_size,
        file.frame_lengths.iter().copied().max().unwrap_or(0)
    );
    assert_eq!(
        header.min_frame_size,
        file.frame_lengths.iter().copied().min().unwrap_or(0)
    );

    // No frame larger than the same block stored verbatim.
    let p = params(rate, channels, bits);
    for (number, length) in file.frame_lengths.iter().enumerate() {
        let samples = (frames as usize - number * BLOCK_SIZE).min(BLOCK_SIZE);
        assert!(
            *length as usize <= verbatim_frame_len(number as u64, samples, p),
            "frame {number} is {length} bytes, more than its raw size"
        );
    }
    assert_eq!(
        file.frame_lengths.len(),
        (frames as usize).div_ceil(BLOCK_SIZE)
    );
    assert_eq!(
        file.bytes.len() as u64,
        HEADER_LEN as u64
            + file
                .frame_lengths
                .iter()
                .map(|l| u64::from(*l))
                .sum::<u64>()
    );
    assert_eq!(file.summary.file_bytes, file.bytes.len() as u64);

    // The scanner agrees with the writer about every frame boundary.
    let mut offsets = Vec::new();
    let mut cursor = Cursor::new(&file.bytes[HEADER_LEN..]);
    let scan = scan_frames(&mut cursor, HEADER_LEN as u64, p, &mut |frame| {
        offsets.push(frame.bytes.len() as u32);
        true
    })
    .unwrap();
    assert_eq!(offsets, file.frame_lengths);
    assert_eq!(scan.samples, frames);
    assert_eq!(scan.end_offset, file.bytes.len() as u64);

    // audio_sha256 covers exactly the frame bytes.
    assert_eq!(
        file.summary.audio_sha256,
        crate::hash::sha256_hex(&file.bytes[HEADER_LEN..])
    );
}

#[test]
fn round_trip_is_bit_exact_over_the_length_rate_channel_and_depth_corpus() {
    let lengths = [0usize, 1, 251, 4095, 4096, 4097, 8192, 12_345, 3 * 4096 + 1];
    for rate in [44_100u32, 48_000, 96_000] {
        for channels in [1u8, 2] {
            for bits in [16u8, 24] {
                for &length in &lengths {
                    let first = at_bits(&speechlike(length, u64::from(rate) + length as u64), bits);
                    let pcm = if channels == 1 {
                        first
                    } else {
                        let second = second_channel(&first);
                        interleave(&[&first, &second])
                    };
                    let file = encode(rate, channels, bits, &pcm);
                    assert_faithful(&file, rate, channels, bits, &pcm);
                }
            }
        }
    }
}

#[test]
fn round_trip_is_bit_exact_for_the_seven_edge_signals() {
    // 4 blocks and a short one: the length the square wave needs to reach the
    // 100 MB-frame bug of flacenc's default configuration.
    let n = 4 * BLOCK_SIZE + 768;
    for kind in EDGES {
        for bits in [24u8, 16] {
            let mono = at_bits(&edge_signal(kind, n), bits);
            let file = encode(48_000, 1, bits, &mono);
            assert_faithful(&file, 48_000, 1, bits, &mono);

            let other = second_channel(&mono);
            let stereo = interleave(&[&mono, &other]);
            let file = encode(48_000, 2, bits, &stereo);
            assert_faithful(&file, 48_000, 2, bits, &stereo);
        }
    }
}

#[test]
fn the_full_scale_square_wave_stays_small() {
    // flacenc 0.5.1 with its default configuration wrote a 100 MB frame for
    // one block of this signal (study finding 7); the study measured 1.44 MB
    // for the 480 000-sample file.
    let mono = edge_signal(Edge::SquareFullScale, 480_000);
    let file = encode(48_000, 1, 24, &mono);
    assert_faithful(&file, 48_000, 1, 24, &mono);
    assert!(file.bytes.len() < 2_000_000, "{} bytes", file.bytes.len());
    let largest = file.frame_lengths.iter().copied().max().unwrap();
    assert!(
        largest < 3 * BLOCK_SIZE as u32 * 3,
        "largest frame {largest}"
    );
}

#[test]
fn the_final_streaminfo_has_min_equal_to_max_even_for_a_short_last_block() {
    let pcm = at_bits(&speechlike(4096 * 3 + 251, 7), 24);
    let file = encode(48_000, 1, 24, &pcm);
    let header = header_of(&file);
    assert_eq!((header.min_block_size, header.max_block_size), (4096, 4096));
    assert_eq!(header.total_samples, 4096 * 3 + 251);
}

#[test]
fn the_placeholder_header_says_unknown_until_the_track_finishes() {
    let mut encoder = encoder(48_000, 1, 24);
    encoder
        .push_pcm(&at_bits(&speechlike(4096 * 3, 3), 24))
        .unwrap();
    encoder.sync().unwrap();
    let sink = encoder.out.get_ref().get_ref().clone();
    let header = parse_header(&sink).unwrap();
    assert_eq!(header.total_samples, 0, "total samples unknown");
    assert_eq!(header.md5, [0; 16], "md5 unknown");
    assert_eq!((header.min_frame_size, header.max_frame_size), (0, 0));
    assert_eq!(header.first_frame, HEADER_LEN);
    // The frames are already complete and decodable up to here.
    assert!(sink.len() > HEADER_LEN);
}

#[test]
fn the_header_is_streaminfo_then_seektable_then_padding_in_a_fixed_region() {
    let pcm = at_bits(&speechlike(8000 * 25, 11), 16);
    let file = encode(8000, 1, 16, &pcm);
    let blocks = metadata_blocks(&file.bytes);
    // 25 s at 8 kHz = 49 blocks; a point every 20 blocks: frames 0, 20, 40.
    assert_eq!(
        blocks,
        vec![
            (0, false, 34),
            (3, false, 3 * SEEK_POINT_LEN),
            (1, true, RESERVED_LEN - 8 - 3 * SEEK_POINT_LEN),
        ]
    );
    assert_eq!(header_of(&file).first_frame, HEADER_LEN);

    // The placeholder had one PADDING block covering the whole region.
    let fresh = encoder(8000, 1, 16);
    let placeholder = fresh.out.get_ref().get_ref().clone();
    assert_eq!(
        metadata_blocks(&placeholder),
        vec![(0, false, 34), (1, true, RESERVED_LEN - 4)]
    );
    assert_eq!(placeholder.len(), HEADER_LEN);
}

#[test]
fn the_seektable_points_at_real_frames_and_a_decoder_can_seek_in_the_file() {
    let pcm = at_bits(&speechlike(8000 * 25, 12), 16);
    let file = encode(8000, 1, 16, &pcm);
    let header = header_of(&file);
    let p = params(8000, 1, 16);

    // Read the points back out of the SEEKTABLE body (it follows STREAMINFO).
    let body = &file.bytes[format::RESERVED_START + 4..];
    let points: Vec<(u64, u64, u16)> = body[..3 * SEEK_POINT_LEN]
        .chunks(SEEK_POINT_LEN)
        .map(|c| {
            (
                u64::from_be_bytes(c[0..8].try_into().unwrap()),
                u64::from_be_bytes(c[8..16].try_into().unwrap()),
                u16::from_be_bytes(c[16..18].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(
        points.iter().map(|p| p.0).collect::<Vec<_>>(),
        [0, 20 * 4096, 40 * 4096]
    );
    for (sample, offset, samples) in points {
        let start = header.first_frame + offset as usize;
        match walk_frame(&file.bytes[start..], p, sample / BLOCK_SIZE as u64) {
            Walk::Frame(end) => assert_eq!(end.block_size, usize::from(samples)),
            other => panic!("a seek point at {offset} is not a frame: {other:?}"),
        }
    }

    // A decoder seeks to the frame that holds 12 s and reads the right samples
    // there (symphonia refines past the table to the exact frame, so this
    // shows the file seeks, not that the table is used).
    let (landed, samples) = seek_and_read(&file.bytes, 12.0);
    assert_eq!(
        landed,
        23 * 4096,
        "the frame that holds 12 s = sample 96000"
    );
    assert_eq!(samples, pcm[23 * 4096..24 * 4096]);
}

#[test]
fn the_seek_table_thins_itself_instead_of_growing() {
    let mut index = format::SeekIndex::new(48_000);
    // 1 point per 118 frames (10 s); many hours' worth of frames.
    for frame in 0..(118 * 6000) {
        index.observe(frame, frame * BLOCK_SIZE as u64, frame * 5000, BLOCK_SIZE);
        assert!(index.points().len() <= format::SEEK_RESERVE_POINTS);
    }
    let samples: Vec<u64> = index.points().iter().map(|p| p.sample).collect();
    assert!(samples.windows(2).all(|w| w[0] < w[1]), "ascending");
    assert_eq!(samples[0], 0);
    assert!(index.points().len() > format::SEEK_RESERVE_POINTS / 2);
}

#[test]
fn pushing_in_any_chunking_gives_the_same_file() {
    let pcm = at_bits(&speechlike(4096 * 5 + 77, 21), 24);
    let reference = encode(44_100, 1, 24, &pcm);
    for chunk in [1usize, 7, 4095, 4096, 4097, 10_000] {
        let mut encoder = encoder(44_100, 1, 24);
        for piece in pcm.chunks(chunk) {
            encoder.push_pcm(piece).unwrap();
        }
        let file = finish(encoder);
        assert!(file.bytes == reference.bytes, "chunk size {chunk}");
    }
}

#[test]
fn f32_samples_are_stored_exactly_and_counted_when_clamped() {
    let ints = at_bits(&speechlike(5000, 5), 24);
    let mut floats = to_f32(&ints, 24);
    let mut expected = ints.clone();
    // +1.0 is one step out of range, 3.0 and -5.0 are far out, NaN is stored as 0.
    for (index, (value, stored)) in [
        (1.0f32, MAX_24),
        (3.0, MAX_24),
        (-5.0, MIN_24),
        (f32::NAN, 0),
        (-1.0, MIN_24),
    ]
    .into_iter()
    .enumerate()
    {
        floats[100 + index] = value;
        expected[100 + index] = stored;
    }
    let mut encoder = encoder(48_000, 1, 24);
    encoder.push(&floats).unwrap();
    assert_eq!(
        encoder.clipped_samples(),
        4,
        "+1.0, 3.0, -5.0 and NaN; -1.0 is exact"
    );
    let file = finish(encoder);
    assert_eq!(file.summary.clipped_samples, 4);
    assert_faithful(&file, 48_000, 1, 24, &expected);
}

#[test]
fn silence_is_stored_as_zeros_and_counted_in_frames() {
    let ints = at_bits(&speechlike(1000, 9), 24);
    let mut encoder = encoder(48_000, 2, 24);
    let stereo = interleave(&[&ints, &ints]);
    encoder.push_pcm(&stereo).unwrap();
    assert_eq!(encoder.frames_pushed(), 1000);
    encoder.push_silence(10_000).unwrap();
    assert_eq!(encoder.frames_pushed(), 11_000);
    encoder.push_pcm(&stereo).unwrap();
    let file = finish(encoder);

    let mut expected = stereo.clone();
    expected.extend(std::iter::repeat_n(0, 20_000));
    expected.extend_from_slice(&stereo);
    assert_faithful(&file, 48_000, 2, 24, &expected);
}

#[test]
fn a_stereo_track_with_side_channels_scans_and_decodes() {
    // Highly correlated channels make the encoder choose a side-channel mode.
    let left = at_bits(&speechlike(3 * BLOCK_SIZE, 4), 24);
    let right: Vec<i32> = left.iter().map(|s| s + 3).collect();
    let pcm = interleave(&[&left, &right]);
    let file = encode(48_000, 2, 24, &pcm);
    assert_faithful(&file, 48_000, 2, 24, &pcm);
}

#[test]
fn every_block_stored_verbatim_still_decodes_and_scans() {
    for (channels, bits) in [(1u8, 24u8), (1, 16), (2, 24), (2, 16)] {
        let first = at_bits(&speechlike(BLOCK_SIZE * 2 + 300, 33), bits);
        let pcm = if channels == 1 {
            first
        } else {
            let second = second_channel(&first);
            interleave(&[&first, &second])
        };
        let mut encoder = encoder(48_000, channels, bits);
        encoder.force_verbatim = true;
        encoder.push_pcm(&pcm).unwrap();
        let file = finish(encoder);
        assert_faithful(&file, 48_000, channels, bits, &pcm);
        let p = params(48_000, channels, bits);
        let raw: Vec<u32> = [BLOCK_SIZE, BLOCK_SIZE, 300]
            .iter()
            .enumerate()
            .map(|(n, samples)| verbatim_frame_len(n as u64, *samples, p) as u32)
            .collect();
        assert_eq!(
            file.frame_lengths, raw,
            "the verbatim frame has the length that was computed for it"
        );
    }
}

#[test]
fn verbatim_frames_are_valid_for_every_length_of_frame_number_and_block() {
    let p = params(48_000, 2, 24);
    let mut frame = Vec::new();
    for frame_number in [
        0u64, 127, 128, 2047, 2048, 65_535, 65_536, 1_000_000, 2_097_152, 50_000_000,
    ] {
        for samples in [1usize, 255, 256, 257, 4095, 4096] {
            let block: Vec<i32> = (0..samples * 2).map(|i| (i as i32 * 977) - 5000).collect();
            write_verbatim_frame(&mut frame, frame_number, &block, p);
            assert_eq!(frame.len(), verbatim_frame_len(frame_number, samples, p));
            match walk_frame(&frame, p, frame_number) {
                Walk::Frame(end) => {
                    assert_eq!(end.len, frame.len());
                    assert_eq!(end.block_size, samples);
                }
                other => panic!("frame {frame_number}/{samples}: {other:?}"),
            }
        }
    }
}

#[test]
fn unsupported_formats_are_refused_before_a_byte_is_written() {
    for (rate, channels, bits) in [
        (0u32, 1u8, 24u8),
        (700_000, 1, 24),
        (48_000, 0, 24),
        (48_000, 3, 24),
        (48_000, 1, 20),
        (48_000, 1, 32),
    ] {
        let result = FlacEncoder::new(Cursor::new(Vec::new()), rate, channels, bits);
        assert!(
            matches!(result, Err(RecordingError::UnsupportedFormat { .. })),
            "{rate} Hz, {channels} ch, {bits} bit"
        );
    }
}

#[test]
fn samples_that_are_not_whole_frames_are_refused() {
    let mut encoder = encoder(48_000, 2, 24);
    assert!(matches!(
        encoder.push_pcm(&[1, 2, 3]),
        Err(RecordingError::InvalidInput { .. })
    ));
    assert!(matches!(
        encoder.push(&[0.0; 3]),
        Err(RecordingError::InvalidInput { .. })
    ));
    assert_eq!(encoder.frames_pushed(), 0);
}

/// A sink that fails after some bytes, as a full disk does.
struct FailingSink {
    inner: Cursor<Vec<u8>>,
    budget: usize,
    kind: io::ErrorKind,
}

impl io::Write for FailingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.budget {
            return Err(io::Error::from(self.kind));
        }
        self.budget -= buf.len();
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl io::Seek for FailingSink {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        io::Seek::seek(&mut self.inner, position)
    }
}

impl TrackSink for FailingSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_full_disk_is_reported_as_such_and_the_track_stops_accepting_audio() {
    let sink = FailingSink {
        inner: Cursor::new(Vec::new()),
        budget: HEADER_LEN + 20_000,
        kind: io::ErrorKind::StorageFull,
    };
    let mut encoder = FlacEncoder::new(sink, 48_000, 1, 24).unwrap();
    let noise = edge_signal(Edge::WhiteFullScale, BLOCK_SIZE * 40);
    let error = encoder
        .push_pcm(&noise)
        .and_then(|()| encoder.sync())
        .unwrap_err();
    assert!(error.is_disk_full(), "{error:?}");
    // The encoder does not keep writing after a failed write.
    assert!(matches!(
        encoder.push_pcm(&noise[..BLOCK_SIZE]),
        Err(RecordingError::TrackFinished)
    ));
}

#[test]
fn the_encoders_own_buffers_do_not_grow_with_the_recording() {
    let block = at_bits(&speechlike(BLOCK_SIZE, 1), 24);
    let mut encoder = FlacEncoder::new(
        // A sink that keeps nothing: only the encoder's working set is measured.
        DiscardSink::default(),
        48_000,
        1,
        24,
    )
    .unwrap();
    for _ in 0..4 {
        encoder.push_pcm(&block).unwrap();
    }
    let after_four = encoder.working_set_bytes();
    for _ in 0..5000 {
        encoder.push_pcm(&block).unwrap();
    }
    assert_eq!(encoder.working_set_bytes(), after_four);
    assert!(after_four < 256 * 1024, "{after_four} bytes");
    assert_eq!(encoder.frames_pushed(), 5004 * BLOCK_SIZE as u64);
}

#[derive(Default)]
struct DiscardSink {
    position: u64,
}

impl io::Write for DiscardSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.position += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Seek for DiscardSink {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if let SeekFrom::Start(p) = position {
            self.position = p;
        }
        Ok(self.position)
    }
}

impl TrackSink for DiscardSink {
    fn sync_data(&mut self) -> io::Result<()> {
        Ok(())
    }
}
