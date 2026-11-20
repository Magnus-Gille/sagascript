//! Clock alignment of a recording's tracks (SAGA-303, design §4.3).
//!
//! Every track is stamped against one host clock: an anchor `(host_ns, frame)`
//! at the start, every ten seconds and at every gap. From the anchors alone, a
//! track's real sample rate is a straight-line fit of frame against host time.
//! A device whose crystal runs 50 ppm fast then plays back 50 ppm too short
//! next to another device, and after thirty minutes two tracks are 90 ms
//! apart: enough to smear an echo and to put words in the wrong order.
//!
//! The stored audio is never changed. [`measure`] is the pure fit;
//! [`alignments`] reads the journals of a finished recording and gives each
//! track the sample rate the decoder should resample from, so every derived
//! signal lands on the host timeline.
//!
//! A fit is only trusted when it covers enough time ([`MIN_SPAN_NS`]) and
//! lands within [`MAX_DRIFT_PPM`] of the nominal rate. Anything else (a
//! track a few seconds long, anchors broken by a device change) is left at
//! its nominal rate and reported as not measured.

use std::path::Path;

use super::decode::start_offsets_ns;
use super::journal::{journal_file_name, read_journal};
use super::manifest::{Manifest, Track};

/// The shortest span of anchors a drift is measured over: below it the fit is
/// dominated by the timing jitter of single blocks.
pub const MIN_SPAN_NS: u64 = 30_000_000_000;

/// The largest drift believed. Crystals are good to 100 ppm; ten times that
/// means the anchors, not the clock, are broken.
pub const MAX_DRIFT_PPM: f64 = 1_000.0;

/// The largest residual a fit may have and still be applied: beyond it the
/// anchors are not one straight line (a device change, a cluster of stalls)
/// and a single slope would be wrong for part of the track.
pub const MAX_RESIDUAL_MS: f64 = 25.0;

/// Below this the correction moves a sample by less than it costs: left alone.
pub const MIN_DRIFT_PPM: f64 = 2.0;

/// How one track sits on the recording's host-clock timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackAlignment {
    pub track_id: String,
    /// Where the track starts after the earliest track of its clock domain.
    pub offset_ns: u64,
    /// Frames per second of host time as measured; the nominal rate when
    /// `measured` is false.
    pub effective_rate: f64,
    /// `(effective - nominal) / nominal` in parts per million.
    pub drift_ppm: f64,
    /// The largest distance of any anchor from the fitted line, in
    /// milliseconds; zero when not measured.
    pub residual_ms: f64,
    pub measured: bool,
    /// Whether the decoder resamples at `effective_rate`: only when the fit is
    /// trusted, large enough to matter, and the track shares its clock domain
    /// with another (alone, there is nothing to align to, and the audio stays
    /// bit-identical to the plain resample).
    pub applies: bool,
}

impl TrackAlignment {
    /// The rate the decoder resamples from.
    pub fn resample_rate(&self, nominal_rate: u32) -> f64 {
        if self.applies {
            self.effective_rate
        } else {
            f64::from(nominal_rate)
        }
    }
}

/// The result of fitting a track's anchors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub effective_rate: f64,
    pub residual_ms: f64,
}

/// Least-squares fit of `frame = a + rate * host_seconds` over the anchors.
/// `None` when the anchors do not cover [`MIN_SPAN_NS`], or the rate they give
/// is more than [`MAX_DRIFT_PPM`] from `nominal_rate`.
pub fn measure(anchors: &[(u64, u64)], nominal_rate: u32) -> Option<Fit> {
    let first = anchors.first()?;
    let last = anchors.last()?;
    if anchors.len() < 2 || last.0.saturating_sub(first.0) < MIN_SPAN_NS || nominal_rate == 0 {
        return None;
    }
    let origin = first.0;
    let points: Vec<(f64, f64)> = anchors
        .iter()
        .map(|&(host, frame)| (host.saturating_sub(origin) as f64 / 1e9, frame as f64))
        .collect();
    let count = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / count;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / count;
    let variance: f64 = points.iter().map(|p| (p.0 - mean_x).powi(2)).sum();
    if variance <= 0.0 {
        return None;
    }
    let covariance: f64 = points.iter().map(|p| (p.0 - mean_x) * (p.1 - mean_y)).sum();
    let rate = covariance / variance;
    let nominal = f64::from(nominal_rate);
    if !rate.is_finite() || ((rate - nominal) / nominal * 1e6).abs() > MAX_DRIFT_PPM {
        return None;
    }
    let intercept = mean_y - rate * mean_x;
    let worst = points
        .iter()
        .map(|p| (p.1 - (intercept + rate * p.0)).abs())
        .fold(0.0_f64, f64::max);
    Some(Fit {
        effective_rate: rate,
        residual_ms: worst / rate * 1000.0,
    })
}

/// The alignment of one track from its anchors.
pub fn align_track(track: &Track, anchors: &[(u64, u64)], offset_ns: u64) -> TrackAlignment {
    let nominal = f64::from(track.sample_rate);
    match measure(anchors, track.sample_rate) {
        Some(fit) => TrackAlignment {
            track_id: track.id.clone(),
            offset_ns,
            effective_rate: fit.effective_rate,
            drift_ppm: (fit.effective_rate - nominal) / nominal * 1e6,
            residual_ms: fit.residual_ms,
            measured: true,
            applies: false,
        },
        None => TrackAlignment {
            track_id: track.id.clone(),
            offset_ns,
            effective_rate: nominal,
            drift_ppm: 0.0,
            residual_ms: 0.0,
            measured: false,
            applies: false,
        },
    }
}

/// Whether a measured alignment is applied, given how many tracks (itself
/// included) share its clock domain.
fn is_applied(alignment: &TrackAlignment, tracks_in_domain: usize) -> bool {
    alignment.measured
        && tracks_in_domain >= 2
        && alignment.drift_ppm.abs() >= MIN_DRIFT_PPM
        && alignment.residual_ms <= MAX_RESIDUAL_MS
}

/// The alignment of every track of the finished recording in `folder`, in
/// manifest order. The anchors are the manifest's start anchor and the ones
/// the track's journal kept.
pub fn alignments(manifest: &Manifest, folder: &Path) -> Vec<TrackAlignment> {
    let offsets = start_offsets_ns(manifest);
    let mut all = Vec::with_capacity(manifest.tracks.len());
    for (track, offset) in manifest.tracks.iter().zip(offsets) {
        // The journal only refines the manifest's anchors: if it cannot be
        // read, the recording still decodes, on what the manifest has.
        let kept = read_journal(&folder.join(journal_file_name(&track.id)))
            .map(|journal| journal.anchors)
            .unwrap_or_default();
        let anchors = if kept.len() >= track.clock.anchors.len() {
            kept
        } else {
            track.clock.anchors.clone()
        };
        let mut alignment = align_track(track, &anchors, offset);
        let companions = manifest
            .tracks
            .iter()
            .filter(|other| other.clock.domain == track.clock.domain)
            .count();
        alignment.applies = is_applied(&alignment, companions);
        all.push(alignment);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000_000_000;

    /// Anchors every ten seconds from a clock that runs `ppm` fast.
    fn anchors(rate: u32, seconds: u64, ppm: f64) -> Vec<(u64, u64)> {
        (0..=seconds / 10)
            .map(|step| {
                let host = 5 * SEC + step * 10 * SEC;
                let frames = (f64::from(rate) * (1.0 + ppm / 1e6) * (step * 10) as f64) as u64;
                (host, frames)
            })
            .collect()
    }

    #[test]
    fn a_clock_that_runs_fast_is_measured_to_a_fraction_of_a_ppm() {
        let fit = measure(&anchors(48_000, 600, 50.0), 48_000).expect("measured");
        let drift = (fit.effective_rate - 48_000.0) / 48_000.0 * 1e6;
        assert!((drift - 50.0).abs() < 0.5, "{drift}");
        assert!(fit.residual_ms < 0.1, "{}", fit.residual_ms);
    }

    #[test]
    fn a_perfect_clock_measures_zero_drift() {
        let fit = measure(&anchors(44_100, 120, 0.0), 44_100).expect("measured");
        assert!((fit.effective_rate - 44_100.0).abs() < 0.01);
    }

    #[test]
    fn too_short_a_span_or_too_few_anchors_is_not_measured() {
        assert_eq!(measure(&anchors(48_000, 20, 50.0), 48_000), None);
        assert_eq!(measure(&[(0, 0)], 48_000), None);
        assert_eq!(measure(&[], 48_000), None);
        // Two anchors at the same time have no slope.
        assert_eq!(measure(&[(SEC, 0), (SEC, 100)], 48_000), None);
    }

    #[test]
    fn a_drift_no_crystal_has_is_distrusted() {
        // 5 % fast: a broken anchor stream, not a clock.
        assert_eq!(measure(&anchors(48_000, 120, 50_000.0), 48_000), None);
    }

    #[test]
    fn a_jittery_anchor_shows_in_the_residual_without_moving_the_rate_much() {
        let mut points = anchors(48_000, 600, 0.0);
        points[30].1 += 480; // ten milliseconds late
        let fit = measure(&points, 48_000).expect("measured");
        assert!(fit.residual_ms > 5.0 && fit.residual_ms < 11.0, "{fit:?}");
        assert!((fit.effective_rate - 48_000.0).abs() < 5.0);
    }

    fn measured(drift_ppm: f64, residual_ms: f64) -> TrackAlignment {
        TrackAlignment {
            track_id: "mic".into(),
            offset_ns: 0,
            effective_rate: 48_000.0 * (1.0 + drift_ppm / 1e6),
            drift_ppm,
            residual_ms,
            measured: true,
            applies: false,
        }
    }

    #[test]
    fn a_drift_is_applied_only_when_it_matters_and_can_be_trusted() {
        assert!(is_applied(&measured(50.0, 1.0), 2));
        // Alone in its domain: nothing to align to, the audio stays as it was.
        assert!(!is_applied(&measured(50.0, 1.0), 1));
        // Too small to move a sample.
        assert!(!is_applied(&measured(0.5, 1.0), 2));
        // Anchors that are not one line.
        assert!(!is_applied(&measured(50.0, 80.0), 2));
        let mut unmeasured = measured(50.0, 1.0);
        unmeasured.measured = false;
        assert!(!is_applied(&unmeasured, 2));
    }

    #[test]
    fn an_alignment_that_does_not_apply_resamples_at_the_nominal_rate() {
        let mut alignment = measured(50.0, 1.0);
        assert_eq!(alignment.resample_rate(48_000), 48_000.0);
        alignment.applies = true;
        assert!((alignment.resample_rate(48_000) - 48_002.4).abs() < 1e-6);
    }
}
