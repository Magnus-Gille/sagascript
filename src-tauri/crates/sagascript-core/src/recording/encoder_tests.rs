use super::*;

const MAX_24: i32 = (1 << 23) - 1;
const MIN_24: i32 = -(1 << 23);

#[test]
fn every_24_bit_integer_that_f32_can_carry_converts_exactly() {
    // f32 has a 24-bit mantissa, so n / 2^23 is exact for every 24-bit n.
    let samples = [
        0,
        1,
        -1,
        2,
        255,
        256,
        65_535,
        1 << 20,
        MAX_24,
        MAX_24 - 1,
        MIN_24,
        MIN_24 + 1,
    ];
    for n in samples {
        let x = n as f32 / (1u32 << 23) as f32;
        assert_eq!(quantise(x, 24), (n, false), "n = {n}");
    }
    // A dense sweep around zero and both ends.
    for n in (-300..300)
        .chain(MAX_24 - 300..=MAX_24)
        .chain(MIN_24..MIN_24 + 300)
    {
        let x = n as f32 / (1u32 << 23) as f32;
        assert_eq!(quantise(x, 24), (n, false), "n = {n}");
    }
}

#[test]
fn sixteen_bit_values_convert_exactly_at_sixteen_bit() {
    for n in [-32768, -32767, -1, 0, 1, 32766, 32767] {
        let x = n as f32 / 32768.0;
        assert_eq!(quantise(x, 16), (n, false), "n = {n}");
    }
}

#[test]
fn plus_one_is_clipped_minus_one_is_exact() {
    assert_eq!(quantise(1.0, 24), (MAX_24, true));
    assert_eq!(quantise(-1.0, 24), (MIN_24, false));
    assert_eq!(quantise(1.0, 16), (32767, true));
    assert_eq!(quantise(-1.0, 16), (-32768, false));
}

#[test]
fn out_of_range_and_non_finite_samples_clamp_and_count() {
    assert_eq!(quantise(2.5, 24), (MAX_24, true));
    assert_eq!(quantise(-2.5, 24), (MIN_24, true));
    assert_eq!(quantise(f32::INFINITY, 24), (MAX_24, true));
    assert_eq!(quantise(f32::NEG_INFINITY, 24), (MIN_24, true));
    assert_eq!(quantise(f32::MAX, 24), (MAX_24, true));
    assert_eq!(quantise(f32::NAN, 24), (0, true));
    assert_eq!(quantise(f32::NAN, 16), (0, true));
}

#[test]
fn values_between_steps_round_to_the_nearest() {
    let step = 1.0 / (1u32 << 23) as f32;
    assert_eq!(quantise(0.4 * step, 24), (0, false));
    assert_eq!(quantise(0.6 * step, 24), (1, false));
    assert_eq!(quantise(-0.6 * step, 24), (-1, false));
    assert_eq!(quantise(-0.4 * step, 24), (0, false));
}

#[test]
fn negative_zero_is_zero() {
    assert_eq!(quantise(-0.0, 24), (0, false));
}
