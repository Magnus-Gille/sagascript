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

/// Pieces shorter than this (single-token words and timestamps inherited by the
/// DTW grouper have `start == end`) are judged by containment, not overlap.
const POINT_PIECE_SECONDS: f64 = 0.02;

fn piece_is_active(piece: &TimestampedSegment, activity: &[(f64, f64)]) -> bool {
    if piece.end - piece.start < POINT_PIECE_SECONDS {
        let mid = (piece.start + piece.end) / 2.0;
        return activity.iter().any(|&(s, e)| mid >= s && mid <= e);
    }
    let span = (piece.start, piece.end);
    let inside: f64 = activity.iter().map(|&a| overlap(span, a)).sum();
    inside / (piece.end - piece.start) >= MIN_ACTIVITY_OVERLAP
}

/// Attribute the microphone transcript to the local user, keeping only pieces
/// that lie inside microphone speech activity.
pub fn local_segments(
    transcript: &[TimestampedSegment],
    activity: &[(f64, f64)],
) -> Vec<DiarizedSegment> {
    transcript
        .iter()
        .filter(|piece| !piece.text.trim().is_empty() && piece_is_active(piece, activity))
        .map(|piece| DiarizedSegment {
            start: piece.start,
            end: piece.end,
            speaker: LOCAL_SPEAKER.to_string(),
            text: piece.text.clone(),
        })
        .collect()
}

/// Padded, merged regions of the microphone channel worth transcribing: each
/// activity interval grown by `pad` seconds, regions closer than `gap` joined,
/// clamped to `[0, duration]`.
pub fn transcription_regions(
    activity: &[(f64, f64)],
    pad: f64,
    gap: f64,
    duration: f64,
) -> Vec<(f64, f64)> {
    union_intervals(
        activity
            .iter()
            .map(|&(s, e)| ((s - pad).max(0.0), (e + pad).min(duration)))
            .collect(),
        gap,
    )
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
/// Windows shorter than this many frames are never judged.
const MIN_FRAMES: usize = 25; // 0.5 s
/// Length of the windows the guard judges independently.
const WINDOW_SECONDS: f64 = 1.0;
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

/// How well the system channel explains the microphone over one window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EchoTest {
    /// Envelope correlation at the best acoustic delay (-1..1).
    pub correlation: f32,
    /// Share of the microphone envelope energy left unexplained by the best
    /// scaled copy of the system envelope (0 = pure echo, 1 = unrelated).
    pub residual: f32,
}

/// Compare the microphone and system loudness envelopes over `[start, end]`
/// seconds. `None` when the window is too short or the system channel is silent.
pub fn echo_test(mic: &[f32], system: &[f32], start: f64, end: f64) -> Option<EchoTest> {
    let from = (start.max(0.0) * 16_000.0) as usize;
    let frames = (((end - start) * 16_000.0) as usize) / FRAME;
    if frames < MIN_FRAMES {
        return None;
    }
    let sys = envelope(system, from, frames);
    if sys.iter().sum::<f32>() / (frames as f32) < SYSTEM_FLOOR_RMS {
        return None;
    }
    let sys_energy: f32 = sys.iter().map(|x| x * x).sum();
    let mut best: Option<EchoTest> = None;
    for lag in -MAX_LAG_FRAMES..=MAX_LAG_FRAMES {
        // The microphone hears the system audio `lag` frames late.
        let shifted = from as i64 + lag * FRAME as i64;
        if shifted < 0 {
            continue;
        }
        let mic_env = envelope(mic, shifted as usize, frames);
        let correlation = pearson(&mic_env, &sys);
        let mic_energy: f32 = mic_env.iter().map(|x| x * x).sum();
        let gain = mic_env.iter().zip(&sys).map(|(m, s)| m * s).sum::<f32>() / sys_energy.max(1e-12);
        let residual_energy: f32 =
            mic_env.iter().zip(&sys).map(|(m, s)| (m - gain * s) * (m - gain * s)).sum();
        let residual = if mic_energy > 0.0 { (residual_energy / mic_energy).min(1.0) } else { 1.0 };
        if best.is_none_or(|b| correlation > b.correlation) {
            best = Some(EchoTest { correlation, residual });
        }
    }
    best
}

/// Envelope correlation at or above which a window may be speaker echo.
pub const CROSSTALK_CORRELATION: f32 = 0.6;
/// ... and the system channel must explain at least half the microphone energy,
/// so a local voice at its own level on top of the echo is kept.
pub const CROSSTALK_MAX_RESIDUAL: f32 = 0.5;

fn window_is_echo(mic: &[f32], system: &[f32], start: f64, end: f64, threshold: f32) -> bool {
    let test = echo_test(mic, system, start, end);
    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        eprintln!("CROSSTALK\t{start:.3}\t{end:.3}\t{test:?}");
    }
    test.is_some_and(|t| t.correlation >= threshold && t.residual <= CROSSTALK_MAX_RESIDUAL)
}

/// Drop the parts of microphone activity that look like the system audio
/// leaking into the microphone (no headphones). Each interval is judged in
/// windows of about a second, so a local reply next to echo survives and local
/// speech on top of echo (energy the system channel does not explain) is kept.
pub fn drop_crosstalk(
    mic: &[f32],
    system: &[f32],
    activity: &[(f64, f64)],
    threshold: f32,
) -> Vec<(f64, f64)> {
    let mut kept: Vec<(f64, f64)> = Vec::new();
    for &(s, e) in activity {
        let n = (((e - s) / WINDOW_SECONDS).floor() as usize).max(1);
        let step = (e - s) / n as f64;
        for i in 0..n {
            let (ws, we) = (s + step * i as f64, s + step * (i + 1) as f64);
            if !window_is_echo(mic, system, ws, we, threshold) {
                match kept.last_mut() {
                    Some(last) if (last.1 - ws).abs() < 1e-9 => last.1 = we,
                    _ => kept.push((ws, we)),
                }
            }
        }
    }
    kept
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
    fn zero_duration_words_are_kept_by_containment() {
        // Real grouped single-token words have start == end.
        let words = [ts(1.5, 1.5, "ja"), ts(2.0, 2.0, "nej"), ts(7.0, 7.0, "hallucination")];
        let out = local_segments(&words, &[(1.0, 3.0)]);
        assert_eq!(out.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["ja", "nej"]);
    }

    #[test]
    fn transcription_regions_pad_merge_and_clamp() {
        let r = transcription_regions(&[(0.1, 1.0), (2.0, 3.0), (30.0, 31.0)], 0.5, 1.5, 31.2);
        assert_eq!(r, vec![(0.0, 3.5), (29.5, 31.2)]);
        // A silent channel has no activity, hence nothing to transcribe.
        assert!(transcription_regions(&[], 0.5, 1.5, 60.0).is_empty());
    }

    #[test]
    fn merged_timeline_is_ordered_and_keeps_original_clock() {
        let merged = merge_timelines(
            vec![ds(10.0, 12.0, "Me"), ds(0.0, 1.0, "Me"), ds(20.0, 21.0, "Me")].into_iter().rev().collect(),
            vec![ds(1.0, 9.0, "SPEAKER_0"), ds(10.0, 12.0, "SPEAKER_1"), ds(13.0, 19.0, "SPEAKER_0")],
        );
        let starts: Vec<f64> = merged.iter().map(|s| s.start).collect();
        assert_eq!(starts, vec![0.0, 1.0, 10.0, 10.0, 13.0, 20.0]);
        assert_eq!(merged[2].speaker, "Me", "identical start and end: the local user first");
        assert_eq!(merged[3].speaker, "SPEAKER_1");
        assert_eq!((merged[4].start, merged[4].end), (13.0, 19.0));
        // Ordering is by start, then end, then channel: a shorter other segment
        // with the same start precedes the local one.
        let m = merge_timelines(vec![ds(5.0, 9.0, "Me")], vec![ds(5.0, 7.0, "SPEAKER_0")]);
        assert_eq!(m[0].speaker, "SPEAKER_0");
    }

    /// Noise bursts with a syllable-like loudness modulation (a flat envelope
    /// has no correlation to measure). `delay` shifts the modulation like an
    /// acoustic path would.
    fn noise(len: usize, on: &[(usize, usize)], gain: f32, seed: u32, mod_seed: u32, delay: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        let mut state = seed;
        let (f1, f2) = (3.0 + (mod_seed % 5) as f32 * 0.7, 0.9 + (mod_seed % 3) as f32 * 0.4);
        let (p1, p2) = (mod_seed as f32 * 1.3, mod_seed as f32 * 0.7);
        let tau = std::f32::consts::TAU / 16_000.0;
        for &(a, b) in on {
            for (i, v) in x[a..b.min(len)].iter_mut().enumerate() {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let t = (a + i).saturating_sub(delay) as f32;
                let m = 0.3 + 0.7 * (f1 * t * tau + p1).sin().abs() * (0.5 + 0.5 * (f2 * t * tau + p2).sin());
                *v = ((state >> 16) as f32 / 32768.0 - 1.0) * gain * m;
            }
        }
        x
    }

    fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    fn secs(v: &[(f64, f64)]) -> f64 {
        v.iter().map(|(s, e)| e - s).sum()
    }

    fn overlap_secs(a: &[(f64, f64)], b: &[(f64, f64)]) -> f64 {
        a.iter().flat_map(|&x| b.iter().map(move |&y| overlap(x, y))).sum()
    }

    // 20 s of system speech (bursts of 2-4 s), leaked at 0.1x into the microphone.
    fn scenario() -> (Vec<f32>, Vec<f32>, Vec<(f64, f64)>) {
        let n = 16_000 * 20;
        let on: Vec<(usize, usize)> = vec![(0, 56_000), (72_000, 120_000), (136_000, 200_000), (216_000, 288_000), (296_000, 320_000)];
        let system = noise(n, &on, 0.3, 1, 7, 0);
        let late: Vec<(usize, usize)> = on.iter().map(|&(a, b)| (a + 640, b + 640)).collect();
        let echo = noise(n, &late, 0.03, 2, 7, 640);
        // Local speech: independent bursts, two of which overlap the remote voice.
        let own_on = [(24_000, 40_000), (88_000, 104_000), (150_000, 166_000), (230_000, 246_000)];
        let own = noise(n, &own_on, 0.1, 3, 11, 0);
        let own_iv: Vec<(f64, f64)> = own_on.iter().map(|&(a, b)| (a as f64 / 16_000.0, b as f64 / 16_000.0)).collect();
        (system, add(&echo, &own), own_iv)
    }

    #[test]
    fn echo_alone_is_dropped_and_silent_system_keeps_everything() {
        let (system, _, _) = scenario();
        let echo = noise(system.len(), &[(640, 56_640), (72_640, 120_640)], 0.03, 2, 7, 640);
        let whole = [(0.0, 7.0), (4.5, 7.5)];
        let kept = drop_crosstalk(&echo, &system, &whole[..1], CROSSTALK_CORRELATION);
        assert!(secs(&kept) < 0.5, "pure echo is removed, kept {kept:?}");
        let silence = vec![0.0; system.len()];
        assert_eq!(drop_crosstalk(&echo, &silence, &whole[..1], CROSSTALK_CORRELATION), whole[..1].to_vec());
        // Windows shorter than 0.5 s are never judged.
        assert_eq!(drop_crosstalk(&echo, &system, &[(0.0, 0.3)], CROSSTALK_CORRELATION).len(), 1);
    }

    #[test]
    fn double_talk_keeps_local_speech_and_drops_the_echo_around_it() {
        // Microphone activity as a voice-activity detector would report it: one
        // interval per stretch where anything is audible (local speech or echo).
        let (system, mic, own) = scenario();
        let activity = [(0.0, 3.5), (4.5, 7.5), (8.5, 12.5), (13.5, 18.0), (18.5, 20.0)];
        let kept = drop_crosstalk(&mic, &system, &activity, CROSSTALK_CORRELATION);
        let own_total = secs(&own);
        let own_kept = overlap_secs(&kept, &own);
        assert!(own_kept / own_total >= 0.9, "local speech surviving: {own_kept}/{own_total}");
        // Echo-only time (activity outside local speech) mostly removed.
        let echo_total = secs(&activity) - overlap_secs(&activity, &own);
        let echo_kept = secs(&kept) - own_kept;
        assert!(echo_kept / echo_total < 0.5, "echo still kept: {echo_kept}/{echo_total}");
    }
}
