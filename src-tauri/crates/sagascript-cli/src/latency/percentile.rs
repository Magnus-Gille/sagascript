pub(crate) fn percentile_ms(samples: &[f64], percentile: u8) -> Result<Option<f64>, &'static str> {
    if !(1..=100).contains(&percentile) {
        return Err("percentile must be between 1 and 100");
    }

    if samples
        .iter()
        .any(|sample| !sample.is_finite() || *sample < 0.0)
    {
        return Err("samples must be finite and non-negative");
    }

    if samples.is_empty() {
        return Ok(None);
    }

    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);

    // Split the multiplication before doing it so the rank calculation stays
    // within usize even for a very large input slice.
    let count = sorted.len();
    let percentile = usize::from(percentile);
    let rank = (count / 100) * percentile + ((count % 100) * percentile).div_ceil(100);
    sorted
        .get(rank.checked_sub(1).ok_or("calculated rank was zero")?)
        .copied()
        .map(Some)
        .ok_or("calculated rank was outside the sample set")
}

#[cfg(test)]
mod tests {
    use super::percentile_ms;

    #[test]
    fn uses_nearest_rank_without_mutating_samples() {
        let samples = [9.0, 1.0, 7.0, 5.0];

        assert_eq!(percentile_ms(&samples, 50), Ok(Some(5.0)));
        assert_eq!(percentile_ms(&samples, 95), Ok(Some(9.0)));
        assert_eq!(samples, [9.0, 1.0, 7.0, 5.0]);
    }

    #[test]
    fn validates_percentile_and_all_samples() {
        for percentile in [0, 101, 255] {
            assert!(percentile_ms(&[1.0], percentile).is_err());
        }
        for sample in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1] {
            assert!(percentile_ms(&[1.0, sample, 2.0], 50).is_err());
        }
    }

    #[test]
    fn handles_empty_singleton_duplicates_and_fractions() {
        assert_eq!(percentile_ms(&[], 50), Ok(None));
        assert_eq!(percentile_ms(&[f64::MAX], 95), Ok(Some(f64::MAX)));
        assert_eq!(percentile_ms(&[2.5, 2.5, 2.5], 95), Ok(Some(2.5)));
    }
}
