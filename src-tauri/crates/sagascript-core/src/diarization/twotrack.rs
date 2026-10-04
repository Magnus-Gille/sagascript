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
/// seconds. The microphone window stays fixed; the system reference is shifted
/// by the acoustic delay (the microphone hears the system `lag` frames late, so
/// `mic[t]` is compared with `system[t - lag]`). Delays whose reference window
/// falls outside the system channel are skipped. `None` when the window is too
/// short, extends past the microphone channel, the system channel is silent, or
/// no delay has a valid aligned reference: such audio is never judged.
pub fn echo_test(mic: &[f32], system: &[f32], start: f64, end: f64) -> Option<EchoTest> {
    let from = (start.max(0.0) * 16_000.0) as usize;
    let frames = (((end - start) * 16_000.0) as usize) / FRAME;
    if frames < MIN_FRAMES || from + frames * FRAME > mic.len() {
        return None;
    }
    let mic_env = envelope(mic, from, frames);
    let mic_energy: f32 = mic_env.iter().map(|x| x * x).sum();
    let mut best: Option<EchoTest> = None;
    for lag in -MAX_LAG_FRAMES..=MAX_LAG_FRAMES {
        let reference = from as i64 - lag * FRAME as i64;
        if reference < 0 || reference as usize + frames * FRAME > system.len() {
            continue; // no valid aligned comparison at this delay
        }
        let sys = envelope(system, reference as usize, frames);
        if sys.iter().sum::<f32>() / (frames as f32) < SYSTEM_FLOOR_RMS {
            continue; // nothing playing at this alignment
        }
        let sys_energy: f32 = sys.iter().map(|x| x * x).sum();
        let correlation = pearson(&mic_env, &sys);
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

/// Envelope correlation at or above which a window may be speaker echo. Taking
/// the best of 31 delays inflates chance correlation between two unrelated
/// speech envelopes, so the bar is high: pure echo scores 0.98 and above.
pub const CROSSTALK_CORRELATION: f32 = 0.8;
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

/// Echo is considered present when the guard test flags more than 3 s, and
/// more than a tenth, of the microphone speech activity.
pub fn echo_detected(echo_seconds: f64, activity_seconds: f64) -> bool {
    echo_seconds > 3.0_f64.max(0.1 * activity_seconds)
}

/// Whether the guard is applied: forced by `On`/`Off`, otherwise (`Auto`)
/// when echo is detected.
pub fn guard_applies(
    mode: crate::meeting::CrosstalkGuardMode,
    echo_seconds: f64,
    activity_seconds: f64,
) -> bool {
    use crate::meeting::CrosstalkGuardMode as M;
    match mode {
        M::On => true,
        M::Off => false,
        M::Auto => echo_detected(echo_seconds, activity_seconds),
    }
}

/// Drop the parts of microphone activity that look like the system audio
/// leaking into the microphone (no headphones). Each interval is judged in
/// windows of about a second; the dropped span is exactly the judged window.
///
/// Known limitation (see the equal-energy test): the test works on loudness
/// envelopes, so a window in which independent local speech carries as much
/// energy as the echo can still look like pure echo and is deleted. This is why
/// the guard is opt-in.
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
    fn auto_mode_applies_the_guard_only_when_echo_is_detected() {
        use crate::meeting::CrosstalkGuardMode::{Auto, Off, On};
        // Headphones: nothing flagged. A few stray seconds stay below the bar.
        assert!(!guard_applies(Auto, 0.0, 120.0));
        assert!(!guard_applies(Auto, 3.0, 20.0));
        assert!(!guard_applies(Auto, 10.0, 120.0), "under a tenth of the activity");
        // Echo: more than 3 s and more than a tenth of the activity.
        assert!(guard_applies(Auto, 20.0, 120.0));
        assert!(guard_applies(Auto, 4.0, 30.0));
        // Forced modes ignore the measurement.
        assert!(guard_applies(On, 0.0, 120.0));
        assert!(!guard_applies(Off, 100.0, 120.0));
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
    fn noise(len: usize, on: &[(usize, usize)], gain: f32, seed: u32, mod_seed: u32, delay: i64) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        let mut state = seed;
        let (f1, f2) = (3.0 + (mod_seed % 5) as f32 * 0.7, 0.9 + (mod_seed % 3) as f32 * 0.4);
        let (p1, p2) = (mod_seed as f32 * 1.3, mod_seed as f32 * 0.7);
        let tau = std::f32::consts::TAU / 16_000.0;
        for &(a, b) in on {
            for (i, v) in x[a..b.min(len)].iter_mut().enumerate() {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let t = ((a + i) as i64 - delay) as f32;
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
        let own = noise(n, &own_on, 0.4, 3, 11, 0); // user close to the microphone: ~20 dB above the echo
        let own_iv: Vec<(f64, f64)> = own_on.iter().map(|&(a, b)| (a as f64 / 16_000.0, b as f64 / 16_000.0)).collect();
        (system, add(&echo, &own), own_iv)
    }

    #[test]
    fn echo_alone_is_dropped_and_silent_system_keeps_everything() {
        let (system, _, _) = scenario();
        let echo = noise(system.len(), &[(640, 56_640), (72_640, 120_640)], 0.03, 2, 7, 640);
        let whole = [(0.0, 7.0), (4.5, 7.5)];
        let kept = drop_crosstalk(&echo, &system, &whole[..1], CROSSTALK_CORRELATION);
        // The window at the start of the file has no negative-delay reference and
        // sits on the burst onset; the rest of the 7 s of pure echo goes.
        assert!(secs(&kept) <= 1.0, "pure echo is removed, kept {kept:?}");
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

    #[test]
    fn equal_energy_double_talk_is_a_known_limitation() {
        // Codex's counterexample: system level alternates between a and 2a,
        // the microphone hears it at gain 1 plus independent local audio of
        // constant RMS sqrt(2.5)a, i.e. 38-71 % of its energy is the user's.
        // The envelope tests cannot tell, so the guard deletes the window.
        let n = 16_000 * 4;
        let a = 0.05f32;
        let mut system = vec![0.0f32; n];
        let mut local = vec![0.0f32; n];
        let (mut s1, mut s2) = (11u32, 23u32);
        for (i, (sv, lv)) in system.iter_mut().zip(local.iter_mut()).enumerate() {
            s1 = s1.wrapping_mul(1664525).wrapping_add(1013904223);
            s2 = s2.wrapping_mul(22695477).wrapping_add(1);
            let level = if (i / 3200) % 2 == 0 { a } else { 2.0 * a };
            *sv = ((s1 >> 16) as f32 / 32768.0 - 1.0) * level * 3f32.sqrt();
            *lv = ((s2 >> 16) as f32 / 32768.0 - 1.0) * (2.5f32).sqrt() * a * 3f32.sqrt();
        }
        let mic = add(&system, &local);
        let t = echo_test(&mic, &system, 0.5, 3.5).unwrap();
        assert!(t.correlation > 0.95 && t.residual < 0.1, "{t:?}");
        let kept = drop_crosstalk(&mic, &system, &[(0.5, 3.5)], CROSSTALK_CORRELATION);
        assert!(secs(&kept) < 0.5, "documented limitation: genuine local speech deleted, kept {kept:?}");
    }

    #[test]
    fn replies_next_to_window_boundaries_survive_with_either_delay_sign() {
        // Echo for 4 s, at +300 ms and at -300 ms (the search range is symmetric);
        // loud local replies sit just before and just after the internal
        // window boundary at 2 s, inside the neighbouring windows.
        for delay in [4_800i64, -4_800] {
            let n = 16_000 * 6;
            let on = [(0usize, 16_000 * 4)];
            let system = noise(n, &on, 0.3, 1, 7, 0);
            let echo_start = delay.max(0) as usize;
            let echo = noise(n, &[(echo_start, echo_start + 16_000 * 4)], 0.03, 2, 7, delay);
            // Replies at [1.6, 1.95] and [2.05, 2.4] s: both on different sides of 2.0.
            let replies = noise(n, &[(25_600, 31_200), (32_800, 38_400)], 0.4, 3, 11, 0);
            let mic = add(&echo, &replies);
            let kept = drop_crosstalk(&mic, &system, &[(0.5, 4.5)], CROSSTALK_CORRELATION);
            let reply = [(1.6, 1.95), (2.05, 2.4)];
            let survived = overlap_secs(&kept, &reply) / secs(&reply);
            assert!(survived >= 0.9, "delay {delay}: replies survived {survived}, kept {kept:?}");
        }
    }

    #[test]
    fn audio_without_a_valid_aligned_reference_is_never_judged() {
        let n = 16_000 * 3;
        let system = noise(n, &[(0, n)], 0.3, 1, 7, 0);
        let echo = noise(n, &[(0, n)], 0.03, 2, 7, 0);
        // Window at the very start: negative-lag references fall before 0 and
        // are skipped, the zero/positive ones remain valid, so it is judged.
        assert!(echo_test(&echo, &system, 0.0, 1.0).is_some());
        // Window running past either channel is not judged and survives.
        assert!(echo_test(&echo, &system, 2.5, 3.5).is_none());
        let kept = drop_crosstalk(&echo, &system, &[(2.7, 3.2)], CROSSTALK_CORRELATION);
        assert_eq!(kept.len(), 1);
        // A system channel shorter than the microphone leaves no valid reference.
        assert!(echo_test(&echo, &system[..16_000], 1.5, 2.5).is_none());
    }
}
