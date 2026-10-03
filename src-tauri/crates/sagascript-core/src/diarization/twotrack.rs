//! "Me versus the others" for two-track recordings (left = microphone, right =
//! system audio). The microphone channel is one fixed speaker ([`LOCAL_SPEAKER`])
//! found by voice-activity segmentation only; ordinary diarization runs on the
//! system channel alone. This module holds the model-free merge logic.

use super::{DiarizedSegment, TimestampedSegment};

/// Label and meeting speaker id of the local user.
pub const LOCAL_SPEAKER: &str = "Me";

/// Share of a transcript piece that must lie inside microphone speech activity
/// for it to be attributed to the local user (drops Whisper hallucinations on
/// quiet stretches of the microphone channel).
const MIN_ACTIVITY_OVERLAP: f64 = 0.5;

/// Sort intervals and join those separated by less than `gap` seconds.
pub fn union_intervals(mut intervals: Vec<(f64, f64)>, gap: f64) -> Vec<(f64, f64)> {
    intervals.retain(|(s, e)| s.is_finite() && e.is_finite() && e >= s);
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (s, e) in intervals {
        match out.last_mut() {
            Some(last) if s - last.1 < gap => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn overlap(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0.0)
}

/// Attribute the microphone transcript to the local user, keeping only pieces
/// that overlap microphone speech activity.
pub fn local_segments(
    transcript: &[TimestampedSegment],
    activity: &[(f64, f64)],
) -> Vec<DiarizedSegment> {
    transcript
        .iter()
        .filter(|piece| {
            let span = (piece.start, piece.end);
            let inside: f64 = activity.iter().map(|&a| overlap(span, a)).sum();
            let length = (piece.end - piece.start).max(1e-6);
            !piece.text.trim().is_empty() && inside / length >= MIN_ACTIVITY_OVERLAP
        })
        .map(|piece| DiarizedSegment {
            start: piece.start,
            end: piece.end,
            speaker: LOCAL_SPEAKER.to_string(),
            text: piece.text.clone(),
        })
        .collect()
}

/// Merge the two channels' segments into one timeline ordered by start time.
/// Both inputs already use the original file's clock (the channels are
/// sample-aligned), so no offset is applied. Ties put the local user first.
pub fn merge_timelines(
    local: Vec<DiarizedSegment>,
    others: Vec<DiarizedSegment>,
) -> Vec<DiarizedSegment> {
    let mut all: Vec<(u8, DiarizedSegment)> = local
        .into_iter()
        .map(|s| (0, s))
        .chain(others.into_iter().map(|s| (1, s)))
        .collect();
    all.sort_by(|(ra, a), (rb, b)| {
        a.start
            .total_cmp(&b.start)
            .then_with(|| a.end.total_cmp(&b.end))
            .then_with(|| ra.cmp(rb))
    });
    all.into_iter().map(|(_, s)| s).collect()
}

/// Frame length of the energy envelopes used by the crosstalk guard.
const FRAME: usize = 320; // 20 ms at 16 kHz
/// Largest acoustic delay (frames) searched between loudspeaker and microphone.
const MAX_LAG_FRAMES: i64 = 15;
/// Intervals shorter than this many frames are never judged.
const MIN_FRAMES: usize = 25; // 0.5 s
/// System audio quieter than this (RMS) counts as silence: nothing to leak.
const SYSTEM_FLOOR_RMS: f32 = 1e-3;

fn envelope(samples: &[f32], start: usize, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let from = (start + i * FRAME).min(samples.len());
            let to = (from + FRAME).min(samples.len());
            let chunk = &samples[from..to];
            if chunk.is_empty() {
                0.0
            } else {
                (chunk.iter().map(|x| x * x).sum::<f32>() / chunk.len() as f32).sqrt()
            }
        })
        .collect()
}

fn pearson(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len()) as f32;
    if n < 2.0 {
        return 0.0;
    }
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut cov, mut va, mut vb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        cov += (x - ma) * (y - mb);
        va += (x - ma) * (x - ma);
        vb += (y - mb) * (y - mb);
    }
    if va <= 0.0 || vb <= 0.0 {
        0.0
    } else {
        cov / (va.sqrt() * vb.sqrt())
    }
}

/// Correlation (-1..1) between the microphone and system loudness envelopes
/// over `[start, end]` seconds, maximised over a small acoustic delay. `None`
/// when the interval is too short or the system channel is silent there.
pub fn envelope_correlation(mic: &[f32], system: &[f32], start: f64, end: f64) -> Option<f32> {
    let from = (start.max(0.0) * 16_000.0) as usize;
    let frames = (((end - start) * 16_000.0) as usize) / FRAME;
    if frames < MIN_FRAMES {
        return None;
    }
    let sys = envelope(system, from, frames);
    if sys.iter().sum::<f32>() / (frames as f32) < SYSTEM_FLOOR_RMS {
        return None;
    }
    let mut best = f32::MIN;
    for lag in -MAX_LAG_FRAMES..=MAX_LAG_FRAMES {
        // The microphone hears the system audio `lag` frames late.
        let shifted = from as i64 + lag * FRAME as i64;
        if shifted < 0 {
            continue;
        }
        best = best.max(pearson(&envelope(mic, shifted as usize, frames), &sys));
    }
    (best > f32::MIN).then_some(best)
}

/// Envelope correlation above which a microphone interval is judged to be the
/// loudspeaker echo of the system channel rather than the local user.
pub const CROSSTALK_CORRELATION: f32 = 0.6;

/// Drop microphone activity intervals that look like the system audio leaking
/// into the microphone (no headphones). Returns the surviving intervals.
pub fn drop_crosstalk(
    mic: &[f32],
    system: &[f32],
    activity: &[(f64, f64)],
    threshold: f32,
) -> Vec<(f64, f64)> {
    activity
        .iter()
        .copied()
        .filter(|&(s, e)| {
            let r = envelope_correlation(mic, system, s, e);
            if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
                eprintln!("CROSSTALK\t{s:.3}\t{e:.3}\t{r:?}");
            }
            r.is_none_or(|r| r < threshold)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(start: f64, end: f64, text: &str) -> TimestampedSegment {
        TimestampedSegment { start, end, text: text.into() }
    }
    fn ds(start: f64, end: f64, speaker: &str) -> DiarizedSegment {
        DiarizedSegment { start, end, speaker: speaker.into(), text: String::new() }
    }

    #[test]
    fn union_joins_close_intervals_only() {
        let u = union_intervals(vec![(5.0, 6.0), (0.0, 1.0), (1.2, 2.0), (3.0, 4.0)], 0.5);
        assert_eq!(u, vec![(0.0, 2.0), (3.0, 4.0), (5.0, 6.0)]);
    }

    #[test]
    fn local_segments_need_activity_and_text() {
        let activity = [(1.0, 3.0)];
        let out = local_segments(
            &[ts(1.1, 2.0, "hej"), ts(5.0, 6.0, "hallucination"), ts(1.5, 2.5, " "), ts(2.8, 3.6, "edge")],
            &activity,
        );
        let texts: Vec<_> = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, vec!["hej"]); // "edge" is only 0.2/0.8 inside
        assert!(out.iter().all(|s| s.speaker == LOCAL_SPEAKER));
    }

    #[test]
    fn merged_timeline_is_ordered_and_keeps_original_clock() {
        let merged = merge_timelines(
            vec![ds(10.0, 12.0, "Me"), ds(0.0, 1.0, "Me"), ds(20.0, 21.0, "Me")].into_iter().rev().collect(),
            vec![ds(1.0, 9.0, "SPEAKER_0"), ds(10.0, 12.0, "SPEAKER_1"), ds(13.0, 19.0, "SPEAKER_0")],
        );
        let starts: Vec<f64> = merged.iter().map(|s| s.start).collect();
        assert_eq!(starts, vec![0.0, 1.0, 10.0, 10.0, 13.0, 20.0]);
        assert_eq!(merged[2].speaker, "Me", "ties put the local user first");
        assert_eq!(merged[3].speaker, "SPEAKER_1");
        // Timestamps pass through untouched.
        assert_eq!((merged[4].start, merged[4].end), (13.0, 19.0));
    }

    fn tone_bursts(len: usize, on: &[(usize, usize)], gain: f32, seed: u32) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        let mut state = seed;
        for &(a, b) in on {
            for v in &mut x[a..b.min(len)] {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (state >> 16) as f32 / 32768.0 - 1.0;
                *v = noise * gain;
            }
        }
        x
    }

    #[test]
    fn echo_of_system_is_dropped_but_independent_speech_is_kept() {
        let n = 16_000 * 10;
        // System: bursts with an irregular on/off pattern.
        let on = [(0, 16_000), (24_000, 40_000), (56_000, 72_000), (88_000, 100_000), (120_000, 150_000)];
        let system = tone_bursts(n, &on, 0.3, 1);
        // Echo: the same envelope, quieter and 40 ms late, different fine structure.
        let late: Vec<(usize, usize)> = on.iter().map(|&(a, b)| (a + 640, b + 640)).collect();
        let echo = tone_bursts(n, &late, 0.03, 2);
        // Independent local speech with its own on/off pattern.
        let own = tone_bursts(n, &[(8_000, 22_000), (44_000, 52_000), (76_000, 86_000), (104_000, 118_000)], 0.1, 3);
        let whole = [(0.0, 9.5)];
        assert!(envelope_correlation(&echo, &system, 0.0, 9.5).unwrap() > 0.8);
        assert!(drop_crosstalk(&echo, &system, &whole, CROSSTALK_CORRELATION).is_empty());
        assert!(envelope_correlation(&own, &system, 0.0, 9.5).unwrap() < CROSSTALK_CORRELATION);
        assert_eq!(drop_crosstalk(&own, &system, &whole, CROSSTALK_CORRELATION), whole.to_vec());
        // Silent system channel: nothing to leak, microphone is kept.
        let silence = vec![0.0; n];
        assert_eq!(drop_crosstalk(&echo, &silence, &whole, CROSSTALK_CORRELATION), whole.to_vec());
        // Too-short intervals are never judged.
        assert_eq!(drop_crosstalk(&echo, &system, &[(0.0, 0.3)], CROSSTALK_CORRELATION).len(), 1);
    }
}
