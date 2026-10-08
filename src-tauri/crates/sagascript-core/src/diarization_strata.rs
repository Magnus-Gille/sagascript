//! Deterministic strata measurements for an already-scored diarization run.
//!
//! This module does not choose a speaker mapping.  [`measure_strata`] accepts the
//! mapping produced by [`crate::diarization_evaluation::evaluate`] and applies it
//! to the explicit UEM.  This keeps short-region and boundary measurements
//! comparable with the global DER run without allowing a local stratum to pick
//! a more favourable mapping.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::diarization_evaluation::{EvaluationReport, ScoringRegion, SpeakerTurn};

pub const SHORT_REGION_SECONDS: f64 = 2.0;
pub const MAX_TIME_SECONDS: f64 = 14_400.0;
pub const MAX_TURNS: usize = 100_000;
pub const MAX_SPEAKERS: usize = 64;
pub const MAX_UEM_REGIONS: usize = 100_000;
const MAX_ID_LENGTH: usize = 128;

#[derive(Debug, Error, PartialEq)]
pub enum StrataError {
    #[error("{kind} contains more than {MAX_TURNS} turns")]
    TooManyTurns { kind: &'static str },
    #[error("{kind} contains more than {MAX_SPEAKERS} distinct speaker IDs")]
    TooManySpeakers { kind: &'static str },
    #[error("UEM contains more than {MAX_UEM_REGIONS} regions")]
    TooManyUemRegions,
    #[error("invalid {kind} interval at index {index}: start={start}, end={end}")]
    InvalidInterval {
        kind: &'static str,
        index: usize,
        start: f64,
        end: f64,
    },
    #[error("{kind} interval at index {index} has an invalid speaker list")]
    InvalidSpeakerList { kind: &'static str, index: usize },
    #[error("UEM regions overlap at normalized index {index}")]
    OverlappingUem { index: usize },
    #[error("hypothesis speaker mapping contains duplicate reference target {reference}")]
    DuplicateMapping { reference: String },
    #[error("speaker mapping names unknown hypothesis speaker {hypothesis}")]
    UnknownHypothesisSpeaker { hypothesis: String },
    #[error("speaker mapping names unknown reference speaker {reference}")]
    UnknownReferenceSpeaker { reference: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BoundaryKind {
    Start,
    End,
}

/// Signed boundary errors are hypothesis time minus reference time.  A positive
/// start error is a late hypothesis start; a positive end error is a late end.
/// Matching is nearest-in-time, one-to-one, and confined to one explicit UEM
/// region.  No boundary is created at a UEM clip edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundaryErrorSummary {
    pub kind: BoundaryKind,
    pub signed_errors_seconds: Vec<f64>,
    pub matched_count: usize,
    pub unmatched_reference: usize,
    pub unmatched_hypothesis: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerBoundaryStratum {
    pub speaker: String,
    pub starts: BoundaryErrorSummary,
    pub ends: BoundaryErrorSummary,
}

/// Source-region measurements are taken after same-speaker union and before UEM
/// clipping. `short_window_reference_seconds` includes this speaker's
/// activity inside the selected short-region windows, including overlap.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpeakerStratum {
    pub speaker: String,
    pub source_region_count: usize,
    pub source_region_seconds: f64,
    pub short_region_count: usize,
    pub short_region_seconds: f64,
    pub short_window_reference_seconds: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StrataScore {
    pub duration_seconds: f64,
    pub reference_speaker_seconds: f64,
    pub miss_seconds: f64,
    pub false_alarm_seconds: f64,
    pub confusion_seconds: f64,
    pub der: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiarizationStrataReport {
    pub uem: Vec<ScoringRegion>,
    /// Disjoint explicit-UEM intersections of all source short-region windows.
    pub short_turn_windows: Vec<ScoringRegion>,
    pub score: StrataScore,
    pub speakers: Vec<SpeakerStratum>,
    pub boundaries: Vec<SpeakerBoundaryStratum>,
}

#[derive(Debug, Clone, Copy)]
struct Interval {
    start: f64,
    end: f64,
}

#[derive(Debug, Clone, Copy)]
struct ClippedInterval {
    start: f64,
    end: f64,
    region: usize,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    time: f64,
    speaker: usize,
    start: bool,
}

#[derive(Debug, Clone, Copy)]
struct BoundaryPoint {
    region: usize,
    time: f64,
}

/// Measure short reference regions, fixed-mapping DER strata, and per-speaker
/// boundary errors over the supplied explicit UEM.
pub fn measure_strata(
    reference: &[SpeakerTurn],
    hypothesis: &[SpeakerTurn],
    uem: &[ScoringRegion],
    evaluation: &EvaluationReport,
) -> Result<DiarizationStrataReport, StrataError> {
    let reference = normalize_turns(reference, "reference")?;
    let hypothesis = normalize_turns(hypothesis, "hypothesis")?;
    let uem = normalize_uem(uem)?;
    let mapping = validate_mapping(evaluation, &reference, &hypothesis)?;
    let short_turn_windows = intersect_short_regions(&reference, &uem);

    let reference_ids: Vec<String> = reference.keys().cloned().collect();
    let hypothesis_ids: Vec<String> = hypothesis.keys().cloned().collect();
    let reference_index: BTreeMap<String, usize> = reference_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect();
    let hypothesis_index: BTreeMap<String, usize> = hypothesis_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect();

    let reference_clipped = clip_all(&reference, &short_turn_windows);
    let hypothesis_clipped = clip_all(&hypothesis, &short_turn_windows);
    let mut score = StrataScore::default();
    let mut scored_reference_seconds = vec![0.0; reference_ids.len()];
    let mut events = vec![Vec::new(); short_turn_windows.len()];
    append_events(&reference_clipped, &reference_index, &mut events, 0);
    append_events(
        &hypothesis_clipped,
        &hypothesis_index,
        &mut events,
        reference_ids.len(),
    );

    for (region_index, region_events) in events.iter_mut().enumerate() {
        region_events.sort_by(|left, right| {
            left.time
                .total_cmp(&right.time)
                .then(left.start.cmp(&right.start))
                .then(left.speaker.cmp(&right.speaker))
        });
        let mut active_reference = vec![false; reference_ids.len()];
        let mut active_hypothesis = vec![false; hypothesis_ids.len()];
        let mut cursor = short_turn_windows[region_index].start;
        let mut event_index = 0;
        while event_index < region_events.len() {
            let time = region_events[event_index].time;
            accumulate_score(
                time - cursor,
                &active_reference,
                &active_hypothesis,
                &mapping,
                &mut score,
                &mut scored_reference_seconds,
            );
            while event_index < region_events.len() && region_events[event_index].time == time {
                let event = region_events[event_index];
                if event.speaker < reference_ids.len() {
                    active_reference[event.speaker] = event.start;
                } else {
                    active_hypothesis[event.speaker - reference_ids.len()] = event.start;
                }
                event_index += 1;
            }
            cursor = time;
        }
        accumulate_score(
            short_turn_windows[region_index].end - cursor,
            &active_reference,
            &active_hypothesis,
            &mapping,
            &mut score,
            &mut scored_reference_seconds,
        );
    }
    finish_score(&mut score);

    let speakers = reference_ids
        .iter()
        .enumerate()
        .map(|(index, speaker)| {
            let source_intervals = &reference[speaker];
            let source_region_count = source_intervals.len();
            let source_region_seconds = source_intervals
                .iter()
                .map(|interval| interval.end - interval.start)
                .sum();
            let (short_region_count, short_region_seconds) = source_intervals
                .iter()
                .filter_map(|interval| {
                    let duration = interval.end - interval.start;
                    (duration < SHORT_REGION_SECONDS).then_some(duration)
                })
                .fold((0, 0.0), |(count, duration), value| {
                    (count + 1, duration + value)
                });
            SpeakerStratum {
                speaker: speaker.clone(),
                source_region_count,
                source_region_seconds,
                short_region_count,
                short_region_seconds,
                short_window_reference_seconds: scored_reference_seconds[index],
            }
        })
        .collect();

    let boundaries = boundary_strata(
        &reference,
        &hypothesis,
        &reference_ids,
        &hypothesis_ids,
        &mapping,
        &uem,
    );

    Ok(DiarizationStrataReport {
        uem,
        short_turn_windows,
        score,
        speakers,
        boundaries,
    })
}

fn normalize_turns(
    turns: &[SpeakerTurn],
    kind: &'static str,
) -> Result<BTreeMap<String, Vec<Interval>>, StrataError> {
    if turns.len() > MAX_TURNS {
        return Err(StrataError::TooManyTurns { kind });
    }
    let mut intervals: BTreeMap<String, Vec<Interval>> = BTreeMap::new();
    for (index, turn) in turns.iter().enumerate() {
        validate_interval(kind, index, turn.start, turn.end)?;
        if turn.speakers.is_empty()
            || turn.speakers.iter().any(|speaker| {
                speaker.is_empty()
                    || speaker.len() > MAX_ID_LENGTH
                    || speaker.chars().any(char::is_control)
            })
            || turn.speakers.iter().collect::<BTreeSet<_>>().len() != turn.speakers.len()
        {
            return Err(StrataError::InvalidSpeakerList { kind, index });
        }
        for speaker in &turn.speakers {
            intervals
                .entry(speaker.clone())
                .or_default()
                .push(Interval {
                    start: turn.start,
                    end: turn.end,
                });
        }
    }
    if intervals.len() > MAX_SPEAKERS {
        return Err(StrataError::TooManySpeakers { kind });
    }
    for speaker_intervals in intervals.values_mut() {
        speaker_intervals.sort_by(|left, right| {
            left.start
                .total_cmp(&right.start)
                .then(left.end.total_cmp(&right.end))
        });
        let mut union: Vec<Interval> = Vec::with_capacity(speaker_intervals.len());
        for interval in speaker_intervals.drain(..) {
            if let Some(last) = union.last_mut() {
                if interval.start <= last.end {
                    last.end = last.end.max(interval.end);
                    continue;
                }
            }
            union.push(interval);
        }
        *speaker_intervals = union;
    }
    Ok(intervals)
}

fn validate_interval(
    kind: &'static str,
    index: usize,
    start: f64,
    end: f64,
) -> Result<(), StrataError> {
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end > MAX_TIME_SECONDS
        || start >= end
    {
        return Err(StrataError::InvalidInterval {
            kind,
            index,
            start,
            end,
        });
    }
    Ok(())
}

fn normalize_uem(uem: &[ScoringRegion]) -> Result<Vec<ScoringRegion>, StrataError> {
    if uem.len() > MAX_UEM_REGIONS {
        return Err(StrataError::TooManyUemRegions);
    }
    let mut normalized = uem.to_vec();
    for (index, region) in normalized.iter().enumerate() {
        validate_interval("UEM", index, region.start, region.end)?;
    }
    normalized.sort_by(|left, right| {
        left.start
            .total_cmp(&right.start)
            .then(left.end.total_cmp(&right.end))
    });
    for index in 1..normalized.len() {
        if normalized[index].start < normalized[index - 1].end {
            return Err(StrataError::OverlappingUem { index });
        }
    }
    Ok(normalized)
}

fn validate_mapping(
    evaluation: &EvaluationReport,
    reference: &BTreeMap<String, Vec<Interval>>,
    hypothesis: &BTreeMap<String, Vec<Interval>>,
) -> Result<Vec<Option<usize>>, StrataError> {
    let reference_ids: Vec<_> = reference.keys().collect();
    let hypothesis_ids: Vec<_> = hypothesis.keys().collect();
    let reference_index: BTreeMap<_, _> = reference_ids
        .iter()
        .enumerate()
        .map(|(index, id)| ((*id).clone(), index))
        .collect();
    let hypothesis_index: BTreeMap<_, _> = hypothesis_ids
        .iter()
        .enumerate()
        .map(|(index, id)| ((*id).clone(), index))
        .collect();
    let mut mapping = vec![None; hypothesis_ids.len()];
    let mut targets = BTreeSet::new();
    for (hypothesis_id, reference_id) in &evaluation.speaker_mapping {
        let Some(&hypothesis) = hypothesis_index.get(hypothesis_id) else {
            return Err(StrataError::UnknownHypothesisSpeaker {
                hypothesis: hypothesis_id.clone(),
            });
        };
        let Some(&reference) = reference_index.get(reference_id) else {
            return Err(StrataError::UnknownReferenceSpeaker {
                reference: reference_id.clone(),
            });
        };
        if !targets.insert(reference) {
            return Err(StrataError::DuplicateMapping {
                reference: reference_id.clone(),
            });
        }
        mapping[hypothesis] = Some(reference);
    }
    Ok(mapping)
}

fn clip_all(
    intervals: &BTreeMap<String, Vec<Interval>>,
    uem: &[ScoringRegion],
) -> BTreeMap<String, Vec<ClippedInterval>> {
    intervals
        .iter()
        .map(|(speaker, source)| (speaker.clone(), clip_intervals(source, uem)))
        .collect()
}

fn intersect_short_regions(
    reference: &BTreeMap<String, Vec<Interval>>,
    uem: &[ScoringRegion],
) -> Vec<ScoringRegion> {
    let mut source = reference
        .values()
        .flat_map(|intervals| intervals.iter().copied())
        .filter(|interval| interval.end - interval.start < SHORT_REGION_SECONDS)
        .collect::<Vec<_>>();
    source.sort_by(|left, right| {
        left.start
            .total_cmp(&right.start)
            .then(left.end.total_cmp(&right.end))
    });
    let mut union: Vec<Interval> = Vec::with_capacity(source.len());
    for interval in source {
        if let Some(last) = union.last_mut() {
            if interval.start <= last.end {
                last.end = last.end.max(interval.end);
                continue;
            }
        }
        union.push(interval);
    }
    let mut output: Vec<ScoringRegion> = Vec::new();
    let mut first_region = 0;
    for interval in union {
        while first_region < uem.len() && uem[first_region].end <= interval.start {
            first_region += 1;
        }
        let mut region = first_region;
        while region < uem.len() && uem[region].start < interval.end {
            let start = interval.start.max(uem[region].start);
            let end = interval.end.min(uem[region].end);
            if start < end {
                if let Some(last) = output.last_mut() {
                    if start <= last.end {
                        last.end = last.end.max(end);
                    } else {
                        output.push(ScoringRegion { start, end });
                    }
                } else {
                    output.push(ScoringRegion { start, end });
                }
            }
            region += 1;
        }
    }
    output
}

fn clip_intervals(source: &[Interval], uem: &[ScoringRegion]) -> Vec<ClippedInterval> {
    let mut output = Vec::new();
    let mut first_region = 0;
    for interval in source {
        while first_region < uem.len() && uem[first_region].end <= interval.start {
            first_region += 1;
        }
        let mut region = first_region;
        while region < uem.len() && uem[region].start < interval.end {
            let start = interval.start.max(uem[region].start);
            let end = interval.end.min(uem[region].end);
            if start < end {
                output.push(ClippedInterval { start, end, region });
            }
            region += 1;
        }
    }
    output
}

fn append_events(
    intervals: &BTreeMap<String, Vec<ClippedInterval>>,
    speaker_index: &BTreeMap<String, usize>,
    events: &mut [Vec<Event>],
    speaker_offset: usize,
) {
    for (speaker, speaker_intervals) in intervals {
        let index = speaker_index[speaker] + speaker_offset;
        for interval in speaker_intervals {
            events[interval.region].push(Event {
                time: interval.start,
                speaker: index,
                start: true,
            });
            events[interval.region].push(Event {
                time: interval.end,
                speaker: index,
                start: false,
            });
        }
    }
}

fn accumulate_score(
    duration: f64,
    active_reference: &[bool],
    active_hypothesis: &[bool],
    mapping: &[Option<usize>],
    score: &mut StrataScore,
    scored_reference_seconds: &mut [f64],
) {
    if duration <= 0.0 {
        return;
    }
    let reference_count = active_reference.iter().filter(|active| **active).count();
    let hypothesis_count = active_hypothesis.iter().filter(|active| **active).count();
    let correct = active_hypothesis
        .iter()
        .enumerate()
        .filter(|(hypothesis, active)| {
            **active && mapping[*hypothesis].is_some_and(|reference| active_reference[reference])
        })
        .count();
    score.duration_seconds += duration;
    score.reference_speaker_seconds += reference_count as f64 * duration;
    score.miss_seconds += reference_count.saturating_sub(hypothesis_count) as f64 * duration;
    score.false_alarm_seconds += hypothesis_count.saturating_sub(reference_count) as f64 * duration;
    score.confusion_seconds += (reference_count.min(hypothesis_count) - correct) as f64 * duration;
    for (index, active) in active_reference.iter().enumerate() {
        if *active {
            scored_reference_seconds[index] += duration;
        }
    }
}

fn finish_score(score: &mut StrataScore) {
    if score.reference_speaker_seconds > 0.0 {
        score.der = Some(
            (score.miss_seconds + score.false_alarm_seconds + score.confusion_seconds)
                / score.reference_speaker_seconds,
        );
    }
}

fn boundary_strata(
    reference: &BTreeMap<String, Vec<Interval>>,
    hypothesis: &BTreeMap<String, Vec<Interval>>,
    reference_ids: &[String],
    hypothesis_ids: &[String],
    mapping: &[Option<usize>],
    uem: &[ScoringRegion],
) -> Vec<SpeakerBoundaryStratum> {
    let mut reference_points = vec![(Vec::new(), Vec::new()); reference_ids.len()];
    for (index, speaker) in reference_ids.iter().enumerate() {
        let (starts, ends) = &mut reference_points[index];
        add_boundary_points(&reference[speaker], uem, starts, ends);
    }
    let mut hypothesis_points = vec![(Vec::new(), Vec::new()); reference_ids.len()];
    for (index, speaker) in hypothesis_ids.iter().enumerate() {
        if let Some(reference) = mapping[index] {
            let (starts, ends) = &mut hypothesis_points[reference];
            add_boundary_points(&hypothesis[speaker], uem, starts, ends);
        }
    }
    reference_ids
        .iter()
        .enumerate()
        .map(|(index, speaker)| SpeakerBoundaryStratum {
            speaker: speaker.clone(),
            starts: match_boundaries(
                BoundaryKind::Start,
                &reference_points[index].0,
                &hypothesis_points[index].0,
                uem.len(),
            ),
            ends: match_boundaries(
                BoundaryKind::End,
                &reference_points[index].1,
                &hypothesis_points[index].1,
                uem.len(),
            ),
        })
        .collect()
}

fn add_boundary_points(
    intervals: &[Interval],
    uem: &[ScoringRegion],
    starts: &mut Vec<BoundaryPoint>,
    ends: &mut Vec<BoundaryPoint>,
) {
    for interval in intervals {
        if let Some(region) = boundary_region(interval.start, uem) {
            starts.push(BoundaryPoint {
                region,
                time: interval.start,
            });
        }
        if let Some(region) = boundary_region(interval.end, uem) {
            ends.push(BoundaryPoint {
                region,
                time: interval.end,
            });
        }
    }
    starts.sort_by(|left, right| {
        left.region
            .cmp(&right.region)
            .then(left.time.total_cmp(&right.time))
    });
    ends.sort_by(|left, right| {
        left.region
            .cmp(&right.region)
            .then(left.time.total_cmp(&right.time))
    });
}

fn boundary_region(time: f64, uem: &[ScoringRegion]) -> Option<usize> {
    let right = lower_bound_by_start(uem, time);
    if right < uem.len() && uem[right].start == time {
        return Some(right);
    }
    let candidate = right.checked_sub(1)?;
    (time <= uem[candidate].end).then_some(candidate)
}

fn lower_bound_by_start(uem: &[ScoringRegion], time: f64) -> usize {
    let mut left = 0;
    let mut right = uem.len();
    while left < right {
        let middle = left + (right - left) / 2;
        if uem[middle].start < time {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    left
}

fn match_boundaries(
    kind: BoundaryKind,
    reference: &[BoundaryPoint],
    hypothesis: &[BoundaryPoint],
    region_count: usize,
) -> BoundaryErrorSummary {
    let mut signed_errors_seconds = Vec::new();
    let mut matched_count = 0;
    let mut unmatched_reference = 0;
    let mut unmatched_hypothesis = 0;
    let mut reference_cursor = 0;
    let mut hypothesis_cursor = 0;
    for region in 0..region_count {
        let reference_start = reference_cursor;
        while reference_cursor < reference.len() && reference[reference_cursor].region == region {
            reference_cursor += 1;
        }
        let hypothesis_start = hypothesis_cursor;
        while hypothesis_cursor < hypothesis.len() && hypothesis[hypothesis_cursor].region == region
        {
            hypothesis_cursor += 1;
        }
        let reference_times: Vec<_> = reference[reference_start..reference_cursor]
            .iter()
            .map(|point| point.time)
            .collect();
        let hypothesis_times: Vec<_> = hypothesis[hypothesis_start..hypothesis_cursor]
            .iter()
            .map(|point| point.time)
            .collect();
        let available: BTreeSet<usize> = (0..hypothesis_times.len()).collect();
        let (errors, matched, unmatched_ref, unmatched_hyp) =
            nearest_matches(&reference_times, &hypothesis_times, available);
        signed_errors_seconds.extend(errors);
        matched_count += matched;
        unmatched_reference += unmatched_ref;
        unmatched_hypothesis += unmatched_hyp;
    }
    BoundaryErrorSummary {
        kind,
        signed_errors_seconds,
        matched_count,
        unmatched_reference,
        unmatched_hypothesis,
    }
}

fn nearest_matches(
    reference: &[f64],
    hypothesis: &[f64],
    mut available: BTreeSet<usize>,
) -> (Vec<f64>, usize, usize, usize) {
    let mut errors = Vec::new();
    for reference_time in reference {
        if available.is_empty() {
            break;
        }
        let insertion = lower_bound(hypothesis, *reference_time);
        let left = available.range(..insertion).next_back().copied();
        let right = available.range(insertion..).next().copied();
        let candidate = match (left, right) {
            (Some(left), Some(right)) => {
                let left_distance = (*reference_time - hypothesis[left]).abs();
                let right_distance = (hypothesis[right] - *reference_time).abs();
                (right_distance < left_distance)
                    .then_some(right)
                    .or(Some(left))
            }
            (Some(left), None) => Some(left),
            (None, Some(right)) => Some(right),
            (None, None) => None,
        };
        let Some(candidate) = candidate else { break };
        available.remove(&candidate);
        errors.push(hypothesis[candidate] - reference_time);
    }
    let matched = errors.len();
    (errors, matched, reference.len() - matched, available.len())
}

fn lower_bound(values: &[f64], target: f64) -> usize {
    let mut left = 0;
    let mut right = values.len();
    while left < right {
        let middle = left + (right - left) / 2;
        if values[middle] < target {
            left = middle + 1;
        } else {
            right = middle;
        }
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarization_evaluation::{evaluate, EvaluationOptions};

    fn turn(start: f64, end: f64, speakers: &[&str]) -> SpeakerTurn {
        SpeakerTurn {
            start,
            end,
            speakers: speakers
                .iter()
                .map(|speaker| (*speaker).to_owned())
                .collect(),
        }
    }

    fn uem(start: f64, end: f64) -> ScoringRegion {
        ScoringRegion { start, end }
    }

    fn evaluation(
        reference: &[SpeakerTurn],
        hypothesis: &[SpeakerTurn],
        uem: &[ScoringRegion],
    ) -> EvaluationReport {
        evaluate(
            reference,
            hypothesis,
            uem,
            EvaluationOptions {
                collar_seconds: 0.0,
            },
        )
        .unwrap()
    }

    #[test]
    fn short_regions_union_before_uem_and_keep_third_speaker() {
        let reference = [
            turn(0.0, 1.0, &["a"]),
            turn(0.5, 1.5, &["a"]),
            turn(2.0, 4.0, &["b"]),
            turn(4.0, 5.0, &["c"]),
        ];
        let hypothesis = [turn(0.0, 5.0, &["x"])];
        let uem = [uem(0.75, 4.5)];
        let report = measure_strata(
            &reference,
            &hypothesis,
            &uem,
            &evaluation(&reference, &hypothesis, &uem),
        )
        .unwrap();
        assert_eq!(report.speakers[0].speaker, "a");
        assert_eq!(report.speakers[0].source_region_count, 1);
        assert_eq!(report.speakers[0].short_region_count, 1);
        assert_eq!(report.speakers[0].short_region_seconds, 1.5);
        assert_eq!(report.speakers[2].speaker, "c");
        assert_eq!(report.speakers[2].short_window_reference_seconds, 0.5);
    }

    #[test]
    fn fixed_mapping_preserves_overlap_silence_and_uem_hole() {
        let reference = [turn(0.0, 1.0, &["a", "b"]), turn(0.0, 4.0, &["long"])];
        let hypothesis = [turn(0.0, 1.0, &["x"]), turn(2.0, 4.0, &["y"])];
        let uem = [uem(0.0, 1.0), uem(1.5, 4.0)];
        let evaluation = evaluation(&reference, &hypothesis, &uem);
        let report = measure_strata(&reference, &hypothesis, &uem, &evaluation).unwrap();
        assert_eq!(
            report.short_turn_windows,
            vec![ScoringRegion {
                start: 0.0,
                end: 1.0
            }]
        );
        assert_eq!(report.score.duration_seconds, 1.0);
        assert_eq!(report.score.reference_speaker_seconds, 3.0);
        assert_eq!(report.score.miss_seconds, 2.0);
        assert_eq!(report.score.false_alarm_seconds, 0.0);
        assert_eq!(report.score.confusion_seconds, 0.0);
        assert_eq!(report.speakers[0].scored_reference_seconds, 1.0);
        assert_eq!(report.speakers[1].scored_reference_seconds, 1.0);
        assert_eq!(report.speakers[2].scored_reference_seconds, 1.0);
    }

    #[test]
    fn boundary_signs_are_signed_and_clip_edges_are_not_invented() {
        let reference = [turn(0.0, 1.0, &["a"]), turn(2.0, 3.0, &["a"])];
        let hypothesis = [turn(0.0, 0.8, &["x"]), turn(2.25, 2.4, &["x"])];
        let uem = [uem(0.5, 2.5)];
        let report = measure_strata(
            &reference,
            &hypothesis,
            &uem,
            &evaluation(&reference, &hypothesis, &uem),
        )
        .unwrap();
        let boundaries = &report.boundaries[0];
        assert_eq!(boundaries.starts.signed_errors_seconds, vec![0.25]);
        assert!((boundaries.ends.signed_errors_seconds[0] + 0.2).abs() < 1e-9);
        assert_eq!(boundaries.starts.unmatched_reference, 0);
        assert_eq!(boundaries.ends.unmatched_hypothesis, 1);
    }

    #[test]
    fn global_mapping_is_required_and_duplicate_targets_fail() {
        let reference = [turn(0.0, 2.0, &["a"]), turn(2.0, 4.0, &["b"])];
        let hypothesis = [turn(0.0, 4.0, &["x"]), turn(0.0, 4.0, &["y"])];
        let uem = [uem(0.0, 4.0)];
        let mut evaluation = evaluation(&reference, &hypothesis, &uem);
        evaluation.speaker_mapping.insert("x".into(), "a".into());
        evaluation.speaker_mapping.insert("y".into(), "a".into());
        assert!(matches!(
            measure_strata(&reference, &hypothesis, &uem, &evaluation),
            Err(StrataError::DuplicateMapping { .. })
        ));
    }

    #[test]
    fn global_mapping_switch_is_honored_without_local_reoptimization() {
        let reference = [turn(0.0, 1.0, &["a"]), turn(1.0, 2.0, &["b"])];
        let hypothesis = [turn(0.0, 1.0, &["x"]), turn(1.0, 2.0, &["y"])];
        let uem = [uem(0.0, 2.0)];
        let mut evaluation = evaluation(&reference, &hypothesis, &uem);
        evaluation.speaker_mapping.clear();
        evaluation.speaker_mapping.insert("x".into(), "b".into());
        evaluation.speaker_mapping.insert("y".into(), "a".into());

        let report = measure_strata(&reference, &hypothesis, &uem, &evaluation).unwrap();
        assert_eq!(report.score.duration_seconds, 2.0);
        assert_eq!(report.score.confusion_seconds, 2.0);
        assert_eq!(report.score.der, Some(1.0));
    }
}
