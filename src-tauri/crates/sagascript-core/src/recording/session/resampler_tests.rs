use super::*;

fn sine(hz: f64, rate: u32, frames: usize, amplitude: f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (f64::from(amplitude) * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin())
                as f32
        })
        .collect()
}

fn run(source_rate: u32, channels: u8, input: &[f32], chunk: usize) -> Vec<f32> {
    let mut resampler = StreamResampler::new(source_rate, channels).unwrap();
    let mut out = Vec::new();
    for piece in input.chunks(chunk * usize::from(channels)) {
        resampler.process(piece, &mut out).unwrap();
    }
    resampler.finish(&mut out).unwrap();
    out
}

#[test]
fn the_total_is_exactly_the_input_scaled_for_odd_rates_and_odd_chunks() {
    for (rate, frames, chunk) in [
        (48_000, 48_000, 480),
        (48_000, 47_999, 333),
        (44_100, 44_100, 441),
        (44_100, 12_345, 1_000),
        (96_000, 96_000, 4_096),
        (22_050, 7_777, 77),
        (48_000, 1, 1),
        (48_000, 100, 7),
    ] {
        let out = run(rate, 1, &sine(440.0, rate, frames, 0.5), chunk);
        let want = (frames as f64 * 16_000.0 / f64::from(rate)).round() as usize;
        assert_eq!(
            out.len(),
            want,
            "{rate} Hz, {frames} frames, chunks of {chunk}"
        );
    }
}

#[test]
fn the_result_does_not_depend_on_how_the_input_was_chunked() {
    let input = sine(1000.0, 48_000, 30_000, 0.6);
    let a = run(48_000, 1, &input, 480);
    let b = run(48_000, 1, &input, 777);
    let c = run(48_000, 1, &input, 30_000);
    assert_eq!(a.len(), b.len());
    assert_eq!(a, b);
    assert_eq!(a, c);
}

#[test]
fn nothing_is_emitted_for_no_input() {
    let mut resampler = StreamResampler::new(48_000, 1).unwrap();
    let mut out = Vec::new();
    resampler.finish(&mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn an_impulse_lands_where_the_time_says_after_the_delay_is_compensated() {
    // Input frame 9000 at 48 kHz is 0.1875 s: stored frame 3000 at 16 kHz.
    let mut input = vec![0.0f32; 24_000];
    input[9_000] = 1.0;
    let out = run(48_000, 1, &input, 480);
    let (peak_at, peak) = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap();
    assert!(
        (peak_at as i64 - 3_000).abs() <= 1,
        "the impulse is at stored frame {peak_at}, not 3000"
    );
    assert!(*peak > 0.2, "{peak}");
}

/// The lag of a resampled tone behind the ideal one, in stored frames, found
/// by projecting the output onto a sine and a cosine of the tone's frequency.
fn lag_in_stored_frames(rate: u32, hz: f64) -> (f64, f64) {
    let input = sine(hz, rate, rate as usize, 0.5);
    let out = run(rate, 1, &input, 960);
    let (mut s, mut c) = (0.0f64, 0.0f64);
    let body = out.len() - 4_000;
    for (i, v) in out.iter().enumerate().skip(2_000).take(body) {
        let w = std::f64::consts::TAU * hz * i as f64 / 16_000.0;
        s += f64::from(*v) * w.sin();
        c += f64::from(*v) * w.cos();
    }
    let lag = -c.atan2(s) / (std::f64::consts::TAU * hz / 16_000.0);
    let gain = (s * s + c * c).sqrt() / (body as f64 * 0.25);
    (lag, gain)
}

#[test]
fn a_tone_keeps_its_phase_to_within_one_stored_frame_at_every_rate() {
    // `rubato` centres its kernel to within a couple of input frames: at
    // 48 kHz the residual is -0.67 stored frames (42 µs), at every
    // frequency, which is far below the jitter of a host-clock stamp. A shift
    // by the reported delay (42 frames) would fail this by a wide margin.
    for rate in [48_000, 44_100, 96_000, 22_050, 32_000] {
        for hz in [300.0, 1_000.0, 3_000.0] {
            let (lag, gain) = lag_in_stored_frames(rate, hz);
            assert!(
                lag.abs() < 1.0,
                "{rate} Hz, {hz} Hz tone: lag {lag:.3} frames"
            );
            assert!(
                (gain - 1.0).abs() < 0.02,
                "{rate} Hz, {hz} Hz tone: gain {gain:.3}"
            );
        }
    }
}

#[test]
fn stereo_is_downmixed_to_the_mean_before_resampling() {
    let left = sine(500.0, 48_000, 9_600, 0.4);
    let right = sine(500.0, 48_000, 9_600, 0.2);
    let interleaved: Vec<f32> = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    let stereo = run(48_000, 2, &interleaved, 480);
    let mean: Vec<f32> = left
        .iter()
        .zip(&right)
        .map(|(l, r)| (l + r) / 2.0)
        .collect();
    let mono = run(48_000, 1, &mean, 480);
    assert_eq!(stereo.len(), mono.len());
    let worst = stereo
        .iter()
        .zip(&mono)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
}

#[test]
fn sixteen_kilohertz_input_is_only_downmixed() {
    let input = sine(300.0, 16_000, 4_000, 0.3);
    let mut resampler = StreamResampler::new(16_000, 1).unwrap();
    assert!(resampler.is_passthrough());
    assert_eq!(resampler.delay(), 0);
    let mut out = Vec::new();
    resampler.process(&input, &mut out).unwrap();
    resampler.finish(&mut out).unwrap();
    assert_eq!(out, input);
}

#[test]
fn the_filter_latency_is_paid_at_finish_and_adds_nothing_to_the_track() {
    let mut resampler = StreamResampler::new(48_000, 1).unwrap();
    // 256-tap filter at a third of the rate: on the order of 40 output frames.
    assert!(
        resampler.delay() > 10 && resampler.delay() < 200,
        "{}",
        resampler.delay()
    );
    let mut out = Vec::new();
    resampler.process(&vec![0.1; 48_000], &mut out).unwrap();
    assert!(
        out.len() < 16_000 && out.len() > 15_000,
        "the last {} frames wait for more input: {}",
        resampler.delay(),
        out.len()
    );
    resampler.finish(&mut out).unwrap();
    assert_eq!(out.len(), 16_000);
}

#[test]
fn unusable_formats_are_refused() {
    assert!(StreamResampler::new(0, 1).is_err());
    assert!(StreamResampler::new(48_000, 0).is_err());
    assert!(StreamResampler::new(48_000, 3).is_err());
}

#[test]
fn the_impulse_position_does_not_depend_on_where_the_chunks_fall() {
    for (position, first, chunk) in [
        (9_000usize, 480usize, 480usize),
        (48_960, 960, 1024),
        (48_960, 960, 4800),
        (30_001, 1, 333),
        (100_000, 1024, 1024),
    ] {
        let mut input = vec![0.0f32; position + 30_000];
        input[position] = 1.0;
        let mut resampler = StreamResampler::new(48_000, 1).unwrap();
        let mut out = Vec::new();
        resampler.process(&input[..first], &mut out).unwrap();
        for piece in input[first..].chunks(chunk) {
            resampler.process(piece, &mut out).unwrap();
        }
        resampler.finish(&mut out).unwrap();
        let (at, _) = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .unwrap();
        let want = (position as f64 / 3.0).round() as i64;
        assert!(
            (at as i64 - want).abs() <= 1,
            "impulse at {position} (first {first}, chunks {chunk}): stored {at}, want {want}"
        );
    }
}
