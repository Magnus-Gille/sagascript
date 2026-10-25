use super::*;

fn signal(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| {
            let phase = index as f32 * 0.017;
            (phase.sin() * 0.35) + (phase * 0.13).cos() * 0.1
        })
        .collect()
}

fn collect(rate: u32, input: &[f32], chunks: &[usize]) -> Result<Vec<f32>, ResamplerError> {
    let mut resampler = StreamResampler::new(rate)?;
    let mut output = Vec::new();
    let mut offset = 0;
    for &length in chunks {
        let mut remaining = length;
        while remaining > 0 && offset < input.len() {
            let take = remaining.min(MAX_INPUT_SAMPLES).min(input.len() - offset);
            output.extend(resampler.push(&input[offset..offset + take])?);
            offset += take;
            remaining -= take;
        }
        if offset == input.len() {
            break;
        }
    }
    while offset < input.len() {
        let end = (offset + MAX_INPUT_SAMPLES).min(input.len());
        output.extend(resampler.push(&input[offset..end])?);
        offset = end;
    }
    output.extend(resampler.finish()?);
    Ok(output)
}

#[test]
fn whole_input_and_variable_chunks_are_identical() {
    let input = signal(44_100);
    let baseline_chunks = vec![MAX_INPUT_SAMPLES; input.len().div_ceil(MAX_INPUT_SAMPLES)];
    let whole = collect(44_100, &input, &baseline_chunks).expect("whole input");
    let split =
        collect(44_100, &input, &[17, 1_021, 4_096, 777, 8_192, 2_003]).expect("split input");

    assert_eq!(whole.len(), split.len());
    assert!(whole
        .iter()
        .zip(split.iter())
        .all(|(left, right)| (left - right).abs() <= 1.0e-6));
}

#[test]
fn maximum_callback_is_valid_at_common_input_rates() {
    let input = signal(MAX_INPUT_SAMPLES);
    for rate in [8_000, 22_050, 32_000] {
        let mut resampler = StreamResampler::new(rate).expect("rate");
        let mut output = resampler.push(&input).expect("maximum callback");
        output.extend(resampler.finish().expect("finish"));
        assert_eq!(
            output.len(),
            MAX_INPUT_SAMPLES * 16_000 / rate as usize,
            "rate={rate}"
        );
    }
}

#[test]
fn supported_rates_have_exact_floor_output_counts() {
    for (rate, length) in [(44_100, 4_410usize), (48_000, 4_800), (16_000, 1_600)] {
        let input = signal(length);
        let output = collect(rate, &input, &[length]).expect("supported rate");
        let expected = length as u64 * 16_000 / rate as u64;
        assert_eq!(output.len(), expected as usize, "rate={rate}");
    }
}

#[test]
fn finish_flushes_filter_tail_for_short_input() {
    let input = signal(1_000);
    let output = collect(192_000, &input, &[input.len()]).expect("short input");
    assert_eq!(output.len(), 1_000 * 16_000 / 192_000);
}

#[test]
fn silence_stays_silent() {
    let input = vec![0.0; 4_800];
    let output = collect(48_000, &input, &[333, 1_024, 2_100]).expect("silence");
    assert!(output.iter().all(|sample| sample.abs() <= 1.0e-7));
}

#[test]
fn invalid_input_does_not_corrupt_state() {
    let input = signal(2_048);
    let mut resampler = StreamResampler::new(44_100).expect("rate");
    let before = resampler.push(&input[..1_024]).expect("first chunk");
    let invalid = [f32::NAN, 0.0];
    assert_eq!(
        resampler.push(&invalid),
        Err(ResamplerError::NonFiniteInput)
    );
    let mut actual = before;
    actual.extend(resampler.push(&input[1_024..]).expect("second chunk"));
    actual.extend(resampler.finish().expect("finish"));

    let expected = collect(44_100, &input, &[input.len()]).expect("fresh state");
    assert_eq!(actual.len(), expected.len());
    assert!(actual
        .iter()
        .zip(expected.iter())
        .all(|(left, right)| (left - right).abs() <= 1.0e-6));
}

#[test]
fn oversized_push_is_rejected_without_state_change() {
    let input = signal(2_048);
    let mut resampler = StreamResampler::new(44_100).expect("rate");
    assert_eq!(
        resampler.push(&vec![0.0; MAX_INPUT_SAMPLES + 1]),
        Err(ResamplerError::InputTooLarge)
    );
    let actual = collect_from_instance(&mut resampler, &input);
    let expected = collect(44_100, &input, &[input.len()]).expect("fresh state");
    assert_eq!(actual, expected);
}

fn collect_from_instance(resampler: &mut StreamResampler, input: &[f32]) -> Vec<f32> {
    let mut output = resampler.push(input).expect("input");
    output.extend(resampler.finish().expect("finish"));
    output
}

#[test]
fn thirty_minutes_of_streaming_input_keeps_retained_state_bounded() {
    let mut resampler = StreamResampler::new(16_000).expect("rate");
    let chunk = vec![0.0; 8_192];
    let total_samples = 30 * 60 * 16_000;
    let full_chunks = total_samples / chunk.len();
    let mut output_count = 0usize;
    for _ in 0..full_chunks {
        output_count += resampler.push(&chunk).expect("chunk").len();
        assert!(resampler.buffered_input_samples() < CHUNK_SIZE);
        assert!(resampler.buffered_output_samples() <= MAX_BUFFERED_OUTPUT);
    }
    let remainder = total_samples % chunk.len();
    if remainder != 0 {
        output_count += resampler
            .push(&chunk[..remainder])
            .expect("remainder")
            .len();
    }
    output_count += resampler.finish().expect("finish").len();
    assert_eq!(output_count, total_samples);
}

#[test]
fn shortened_long_run_at_non_native_rates_keeps_state_bounded() {
    // A five-second deterministic surrogate keeps this unit test practical
    // while exercising rubato's streaming path at both common rates.
    for rate in [44_100u32, 48_000] {
        let total_samples = rate as usize * 5;
        let chunk = vec![0.0; MAX_INPUT_SAMPLES];
        let mut resampler = StreamResampler::new(rate).expect("rate");
        let mut offset = 0;
        let mut output_count = 0;
        while offset < total_samples {
            let end = (offset + chunk.len()).min(total_samples);
            output_count += resampler.push(&chunk[..end - offset]).expect("chunk").len();
            assert!(resampler.buffered_input_samples() < CHUNK_SIZE);
            assert!(resampler.buffered_output_samples() <= MAX_BUFFERED_OUTPUT);
            offset = end;
        }
        output_count += resampler.finish().expect("finish").len();
        assert_eq!(output_count, total_samples * 16_000 / rate as usize);
    }
}

#[test]
fn finish_is_terminal_and_push_is_rejected_afterwards() {
    let mut resampler = StreamResampler::new(16_000).expect("rate");
    resampler.push(&[0.0; 16]).expect("input");
    assert!(resampler.finish().is_ok());
    assert_eq!(resampler.push(&[0.0]), Err(ResamplerError::Finished));
}

#[test]
fn invalid_rates_are_rejected() {
    assert!(matches!(
        StreamResampler::new(7_999),
        Err(ResamplerError::InvalidRate)
    ));
    assert!(matches!(
        StreamResampler::new(192_001),
        Err(ResamplerError::InvalidRate)
    ));
}
