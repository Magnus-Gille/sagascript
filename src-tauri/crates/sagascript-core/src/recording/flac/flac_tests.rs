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

// ---- the order of writes and syncs at finish -------------------------------------

/// What a [`ScriptedSink`] saw, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    Write { at: u64, len: usize },
    Sync,
}

/// An in-memory track that logs every write and sync and can fail the n-th
/// sync, the way a disk does. The bytes and the log are shared so a test can
/// look at them after the encoder (and with it the sink) is gone.
#[derive(Clone)]
struct ScriptedSink {
    data: std::sync::Arc<std::sync::Mutex<Cursor<Vec<u8>>>>,
    ops: std::sync::Arc<std::sync::Mutex<Vec<Op>>>,
    syncs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Fail the sync with this number (counted from 0 after `arm`).
    fail_sync: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
    writes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Fail the write with this number (counted from 0 after `arm_write`);
    /// nothing of a failed write reaches the data.
    fail_write: std::sync::Arc<std::sync::Mutex<Option<usize>>>,
}

impl ScriptedSink {
    fn new() -> Self {
        Self {
            data: Default::default(),
            ops: Default::default(),
            syncs: Default::default(),
            fail_sync: Default::default(),
            writes: Default::default(),
            fail_write: Default::default(),
        }
    }

    /// Forget what happened so far and fail the `n`-th write from now on.
    fn arm_write(&self, fail_write: Option<usize>) {
        self.ops.lock().unwrap().clear();
        self.writes.store(0, std::sync::atomic::Ordering::SeqCst);
        *self.fail_write.lock().unwrap() = fail_write;
    }

    /// Forget what happened so far and fail the `n`-th sync from now on.
    fn arm(&self, fail_sync: Option<usize>) {
        self.ops.lock().unwrap().clear();
        self.syncs.store(0, std::sync::atomic::Ordering::SeqCst);
        *self.fail_sync.lock().unwrap() = fail_sync;
    }

    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }

    fn bytes(&self) -> Vec<u8> {
        self.data.lock().unwrap().get_ref().clone()
    }
}

impl io::Write for ScriptedSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let number = self
            .writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if *self.fail_write.lock().unwrap() == Some(number) {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        let mut data = self.data.lock().unwrap();
        let at = data.position();
        let written = data.write(buf)?;
        self.ops
            .lock()
            .unwrap()
            .push(Op::Write { at, len: written });
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Seek for ScriptedSink {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        io::Seek::seek(&mut *self.data.lock().unwrap(), position)
    }
}

impl TrackSink for ScriptedSink {
    fn sync_data(&mut self) -> io::Result<()> {
        let number = self.syncs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.ops.lock().unwrap().push(Op::Sync);
        if *self.fail_sync.lock().unwrap() == Some(number) {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        Ok(())
    }
}

/// An encoder on a [`ScriptedSink`] that has taken three blocks and a bit
/// (so the last frame is still waiting when `finish` starts).
fn encoder_on_scripted_sink() -> (FlacEncoder<ScriptedSink>, ScriptedSink, Vec<i32>) {
    let sink = ScriptedSink::new();
    let mut encoder = FlacEncoder::new(sink.clone(), 48_000, 1, 24).unwrap();
    let pcm = at_bits(&speechlike(3 * BLOCK_SIZE + 700, 41), 24);
    encoder.push_pcm(&pcm).unwrap();
    (encoder, sink, pcm)
}

#[test]
fn finish_syncs_the_frames_before_the_header_is_rewritten_and_the_header_after() {
    let (encoder, sink, _) = encoder_on_scripted_sink();
    sink.arm(None);
    encoder.finish_into_parts().unwrap();

    let ops = sink.ops();
    let rewrite = ops
        .iter()
        .position(|op| matches!(op, Op::Write { at: 0, .. }))
        .expect("the header is rewritten at offset 0");
    let last_frame_write = ops[..rewrite]
        .iter()
        .rposition(|op| matches!(op, Op::Write { at, .. } if *at >= HEADER_LEN as u64))
        .expect("the last frame is written before the header");
    assert!(
        ops[last_frame_write..rewrite].contains(&Op::Sync),
        "the frames are durable before the metadata changes: {ops:?}"
    );
    assert!(
        ops[rewrite..].contains(&Op::Sync),
        "the new metadata is synced too: {ops:?}"
    );
    assert_eq!(
        ops.iter().filter(|op| **op == Op::Sync).count(),
        2,
        "one sync on each side of the rewrite: {ops:?}"
    );
}

#[test]
fn a_failed_sync_of_the_frames_leaves_the_placeholder_header_untouched() {
    let (encoder, sink, pcm) = encoder_on_scripted_sink();
    let placeholder = sink.bytes()[..HEADER_LEN].to_vec();
    sink.arm(Some(0));
    let error = encoder.finish_into_parts().err().expect("finish fails");
    assert!(error.is_disk_full(), "{error:?}");

    let bytes = sink.bytes();
    assert!(
        bytes[..HEADER_LEN] == placeholder,
        "the metadata was not rewritten over frames that may not be durable"
    );
    assert!(
        !sink
            .ops()
            .iter()
            .any(|op| matches!(op, Op::Write { at: 0, .. })),
        "no write at offset 0"
    );
    // The frames themselves reached the sink, whole: recovery can use them.
    let mut kept = 0;
    scan_frames(
        &mut Cursor::new(&bytes[HEADER_LEN..]),
        HEADER_LEN as u64,
        params(48_000, 1, 24),
        &mut |_| {
            kept += 1;
            true
        },
    )
    .unwrap();
    assert_eq!(kept, 4, "three blocks and the last short one");
    let _ = pcm;
}

#[test]
fn a_failed_sync_after_the_rewrite_is_reported_and_the_header_is_the_new_one() {
    let (encoder, sink, _) = encoder_on_scripted_sink();
    sink.arm(Some(1));
    let error = encoder.finish_into_parts().err().expect("finish fails");
    assert!(error.is_disk_full(), "{error:?}");
    let header = parse_header(&sink.bytes()).expect("a header");
    assert_eq!(header.total_samples, 3 * BLOCK_SIZE as u64 + 700);
}

// ---- gaps and pushes that FLAC cannot count ------------------------------------

use super::format::MAX_TOTAL_SAMPLES;

/// Everything a refused push must leave alone.
fn state_of(encoder: &FlacEncoder<Cursor<Vec<u8>>>) -> (u64, u64, usize, u64) {
    (
        encoder.frames_pushed(),
        encoder.clipped_samples(),
        encoder.pending.len(),
        encoder.next_frame,
    )
}

#[test]
fn a_gap_whose_sample_count_overflows_is_refused_without_a_panic_or_a_change() {
    // 2^63 frames of stereo is 2^64 samples: the product wraps to zero in a
    // release build and panics in a debug build.
    for (channels, frames) in [
        (2u8, 1u64 << 63),
        (2, u64::MAX),
        (1, u64::MAX),
        (2, (1 << 63) + 7),
    ] {
        let mut encoder = encoder(48_000, channels, 24);
        encoder
            .push_pcm(&vec![1; 5 * usize::from(channels)])
            .unwrap();
        let before = state_of(&encoder);
        let result = encoder.push_silence(frames);
        assert!(
            matches!(result, Err(RecordingError::UnsupportedFormat { .. })),
            "{channels} ch, {frames} frames: {result:?}"
        );
        assert_eq!(state_of(&encoder), before, "{channels} ch, {frames} frames");
        // The encoder still works.
        encoder.push_silence(10).unwrap();
        assert_eq!(encoder.frames_pushed(), 15);
    }
}

#[test]
fn nothing_past_the_36_bit_sample_count_of_streaminfo_is_accepted() {
    let mut encoder = encoder(48_000, 2, 24);
    encoder.frames_pushed = MAX_TOTAL_SAMPLES - 5;
    // Exactly up to the limit is fine ...
    encoder.push_pcm(&[3; 10]).unwrap();
    assert_eq!(encoder.frames_pushed(), MAX_TOTAL_SAMPLES);
    // ... and one frame more is refused on every path, leaving no trace.
    let before = state_of(&encoder);
    for result in [
        encoder.push_pcm(&[1, 2]),
        encoder.push(&[2.0, 0.5]),
        encoder.push_silence(1),
    ] {
        assert!(
            matches!(result, Err(RecordingError::UnsupportedFormat { .. })),
            "{result:?}"
        );
        assert_eq!(state_of(&encoder), before);
    }
    // A request that is too long is refused whole, not cut short.
    let mut other = self::encoder(48_000, 1, 24);
    other.frames_pushed = MAX_TOTAL_SAMPLES - 5;
    let before = state_of(&other);
    assert!(other.push_silence(6).is_err());
    assert_eq!(state_of(&other), before);
    other.push_silence(5).unwrap();
    assert_eq!(other.frames_pushed(), MAX_TOTAL_SAMPLES);
    // The track can still be ended.
    assert!(other.finish_into_parts().is_ok());
}

#[test]
fn the_room_check_agrees_with_the_pushes() {
    let mut encoder = encoder(48_000, 1, 24);
    encoder.frames_pushed = MAX_TOTAL_SAMPLES - 1;
    assert!(encoder.ensure_room(1).is_ok());
    assert!(encoder.ensure_room(2).is_err());
    assert!(encoder.ensure_room(u64::MAX).is_err());
}

// ---- a track that ends early keeps the audio it had accepted ------------------------

/// What the frames of a track file hold: how many, how many samples, and
/// whether the file ends exactly at the end of the last whole frame.
fn frames_in(bytes: &[u8]) -> (u64, u64, bool) {
    let summary = scan_frames(
        &mut Cursor::new(&bytes[HEADER_LEN..]),
        HEADER_LEN as u64,
        params(48_000, 1, 24),
        &mut |_| true,
    )
    .unwrap();
    (
        summary.frames,
        summary.samples,
        summary.end_offset == bytes.len() as u64,
    )
}

/// The file a track of `pcm` and then `silence` frames of silence has when it
/// is finished, with every block stored verbatim (so a block is big enough to
/// fill the write buffer).
fn verbatim_reference(pcm: &[i32], silence: u64) -> Encoded {
    let mut encoder = encoder(48_000, 1, 24);
    encoder.force_verbatim = true;
    encoder.push_pcm(pcm).unwrap();
    if silence > 0 {
        encoder.push_silence(silence).unwrap();
    }
    finish(encoder)
}

fn verbatim_encoder_on_scripted_sink() -> (FlacEncoder<ScriptedSink>, ScriptedSink) {
    let sink = ScriptedSink::new();
    let mut encoder = FlacEncoder::new(sink.clone(), 48_000, 1, 24).unwrap();
    encoder.force_verbatim = true;
    (encoder, sink)
}

#[test]
fn salvage_writes_the_partial_block_syncs_it_and_leaves_the_header_for_recovery() {
    let (mut encoder, sink, pcm) = encoder_on_scripted_sink();
    let placeholder = sink.bytes()[..HEADER_LEN].to_vec();
    sink.arm(None);
    encoder.salvage(pcm.len() as u64).unwrap();

    let bytes = sink.bytes();
    assert!(
        bytes[..HEADER_LEN] == placeholder,
        "the header is not rewritten"
    );
    // Three blocks and the 700 frames that were waiting, whole.
    assert_eq!(
        frames_in(&bytes),
        (4, 3 * BLOCK_SIZE as u64 + 700, true),
        "the partial block is in the file"
    );
    // They are the frames a normal finish writes.
    let reference = encode(48_000, 1, 24, &pcm);
    assert!(bytes[HEADER_LEN..] == reference.bytes[HEADER_LEN..]);
    // And durable: the last write is followed by a sync.
    let ops = sink.ops();
    let last_write = ops.iter().rposition(|op| matches!(op, Op::Write { .. }));
    assert!(
        last_write.is_some_and(|at| ops[at..].contains(&Op::Sync)),
        "{ops:?}"
    );
    // The track takes nothing more.
    assert!(matches!(
        encoder.push_pcm(&[1; 4]),
        Err(RecordingError::TrackFinished)
    ));
    assert!(matches!(
        encoder.push_silence(4),
        Err(RecordingError::TrackFinished)
    ));
}

#[test]
fn salvage_keeps_nothing_that_was_accepted_after_the_frame_it_is_given() {
    let mut encoder = encoder(48_000, 1, 24);
    let pcm = at_bits(&speechlike(3 * BLOCK_SIZE + 700, 42), 24);
    encoder.push_pcm(&pcm).unwrap();
    // A gap was under way: 10 000 frames of silence, two blocks of which
    // (with the 700 frames that came first in the first one) were encoded.
    encoder.push_silence(10_000).unwrap();
    encoder.salvage(pcm.len() as u64).unwrap();

    let parts = {
        // Look at the sink the encoder wrote to.
        let bytes = encoder.out.get_ref().get_ref().clone();
        (bytes, encoder.frame_log().unwrap().to_vec())
    };
    let reference = {
        let mut other = self::encoder(48_000, 1, 24);
        other.push_pcm(&pcm).unwrap();
        other.push_silence(10_000).unwrap();
        finish(other)
    };
    let (kept, log) = parts;
    assert_eq!(
        log.len(),
        5,
        "three blocks, the straddling block, one of silence"
    );
    let size: usize = log.iter().map(|len| *len as usize).sum();
    assert_eq!(kept.len(), HEADER_LEN + size);
    assert!(
        kept[HEADER_LEN..] == reference.bytes[HEADER_LEN..HEADER_LEN + size],
        "the blocks that were encoded are untouched, and the 2508 frames of silence still waiting are not added"
    );
    assert_eq!(frames_in(&kept), (5, 5 * BLOCK_SIZE as u64, true));
}

#[test]
fn salvage_of_a_track_with_nothing_waiting_adds_no_frame() {
    let (mut encoder, sink) = verbatim_encoder_on_scripted_sink();
    let pcm = at_bits(&speechlike(2 * BLOCK_SIZE, 43), 24);
    encoder.push_pcm(&pcm).unwrap();
    sink.arm(None);
    encoder.salvage(pcm.len() as u64).unwrap();
    let bytes = sink.bytes();
    assert_eq!(frames_in(&bytes), (2, 2 * BLOCK_SIZE as u64, true));
    assert!(sink.ops().contains(&Op::Sync));
}

#[test]
fn salvage_refuses_to_keep_more_than_the_track_accepted_and_changes_nothing() {
    let mut encoder = encoder(48_000, 1, 24);
    encoder.push_pcm(&[5; 100]).unwrap();
    for keep in [101, 1 << 40, u64::MAX] {
        let result = encoder.salvage(keep);
        assert!(
            matches!(result, Err(RecordingError::InvalidInput { .. })),
            "{keep}: {result:?}"
        );
    }
    // Still a working track.
    encoder.push_pcm(&[6; 28]).unwrap();
    assert_eq!(encoder.frames_pushed(), 128);
    let file = finish(encoder);
    assert_eq!(header_of(&file).total_samples, 128);
}

#[test]
fn a_silence_block_that_cannot_be_written_does_not_cost_the_audio_before_the_gap() {
    // Two verbatim blocks wait in the write buffer, and 1000 frames of speech
    // wait for the rest of their block.
    let (mut encoder, sink) = verbatim_encoder_on_scripted_sink();
    let pcm = at_bits(&speechlike(2 * BLOCK_SIZE + 1000, 44), 24);
    encoder.push_pcm(&pcm).unwrap();
    // The gap's silence completes that block, which needs the buffer to be
    // written, and the disk refuses.
    sink.arm_write(Some(0));
    let error = encoder.push_silence(4000).unwrap_err();
    assert!(error.is_disk_full(), "{error:?}");
    assert!(matches!(
        encoder.push_pcm(&[1; 4]),
        Err(RecordingError::TrackFinished)
    ));

    // The block that failed held the 1000 frames and the start of the
    // silence. The audio is written out on its own; the silence is not.
    encoder.salvage(pcm.len() as u64).unwrap();
    let bytes = sink.bytes();
    assert_eq!(frames_in(&bytes), (3, pcm.len() as u64, true));
    let reference = verbatim_reference(&pcm, 0);
    assert!(
        bytes[HEADER_LEN..] == reference.bytes[HEADER_LEN..],
        "the audio before the gap, to the last frame of it"
    );
}

#[test]
fn a_later_silence_block_that_cannot_be_written_leaves_the_audio_and_the_silence_that_made_it() {
    // 1000 frames of speech wait; the silence completes their block (which is
    // then in the write buffer) and a second block; the third needs the
    // buffer to be written, and the disk refuses.
    let (mut encoder, sink) = verbatim_encoder_on_scripted_sink();
    let pcm = at_bits(&speechlike(1000, 45), 24);
    encoder.push_pcm(&pcm).unwrap();
    sink.arm_write(Some(0));
    let error = encoder.push_silence(3 * BLOCK_SIZE as u64).unwrap_err();
    assert!(error.is_disk_full(), "{error:?}");

    // The audio is already inside a block that was encoded: salvage writes
    // the buffer (the disk works again) and adds nothing.
    encoder.salvage(pcm.len() as u64).unwrap();
    let bytes = sink.bytes();
    assert_eq!(frames_in(&bytes), (2, 2 * BLOCK_SIZE as u64, true));
    let reference = verbatim_reference(&pcm, 3 * BLOCK_SIZE as u64);
    let two_frames: usize = reference.frame_lengths[..2]
        .iter()
        .map(|n| *n as usize)
        .sum();
    assert!(
        bytes[HEADER_LEN..] == reference.bytes[HEADER_LEN..HEADER_LEN + two_frames],
        "the straddling block and the silence block, as a finish would have written them"
    );
}

#[test]
fn a_write_that_keeps_failing_is_reported_by_salvage_and_leaves_a_file_recovery_can_use() {
    let (mut encoder, sink) = verbatim_encoder_on_scripted_sink();
    let pcm = at_bits(&speechlike(2 * BLOCK_SIZE + 300, 46), 24);
    encoder.push_pcm(&pcm).unwrap();
    // The first write of the salvage fails: the buffer cannot be written.
    sink.arm_write(Some(0));
    let error = encoder.salvage(pcm.len() as u64).unwrap_err();
    assert!(error.is_disk_full(), "{error:?}");
    // What reached the sink is the header: no torn frame, nothing to scan.
    let bytes = sink.bytes();
    assert_eq!(bytes.len(), HEADER_LEN);
    // The track took nothing more even so.
    assert!(matches!(
        encoder.push_pcm(&[1; 4]),
        Err(RecordingError::TrackFinished)
    ));
}
