//! Engine-agnostic long-audio window planning and aligned-token merging.
//!
//! This is an exact port of the chunking and merge logic used by
//! [parakeet-mlx](https://github.com/senstella/parakeet-mlx) (Apache-2.0,
//! `parakeet_mlx/alignment.py`), which is also what KlangAI's official MLX
//! build of Pianissimo uses for long audio. An engine transcribes one window
//! and returns aligned tokens; this module plans the windows, shifts token
//! times to the global timeline, merges the per-window token streams and
//! renders the final text. See `THIRD_PARTY_NOTICES.md` for attribution.
//!
//! Behavioural parity is verified against golden fixtures recorded from the
//! Python reference (`tests/chunk_merge_golden.rs`). Deliberately preserved
//! reference quirks: strict `<`/`>` comparisons, first-best tie-breaking in the
//! contiguous search, LCS back-tracking that prefers moving `j` on ties, and
//! gap resolution that only takes window B's gap when it is strictly longer.

use serde::{Deserialize, Serialize};

/// One aligned SentencePiece token. `end` is always `start + duration`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlignedToken {
    pub id: u32,
    /// Token text; a leading space (already converted from `▁`) starts a word.
    pub text: String,
    pub start: f64,
    pub duration: f64,
    pub end: f64,
    /// Not used by merging; carried through for engines that report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

impl AlignedToken {
    pub fn new(id: u32, text: impl Into<String>, start: f64, duration: f64) -> Self {
        Self {
            id,
            text: text.into(),
            start,
            duration,
            end: start + duration,
            confidence: None,
        }
    }
}

/// A window of samples `[start_sample, end_sample)` to transcribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    pub start_sample: usize,
    pub end_sample: usize,
}

/// The contiguous merge found no run of matching tokens long enough.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("No pairs exceeding {enough_pairs}")]
pub struct MergeError {
    pub enough_pairs: usize,
}

/// Which strategy [`merge_window_traced`] used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePath {
    Contiguous,
    Lcs,
}

/// Plan windows like the reference runner: `size = window_s * sr`,
/// `step = (window_s - overlap_s) * sr` (both truncated to whole samples);
/// a start every `step` samples until a window reaches the end. Audio no longer
/// than one window yields a single window. Invalid parameters (zero window or
/// overlap >= window) also yield a single window covering everything.
pub fn plan_windows(
    total_samples: usize,
    sample_rate: u32,
    window_s: f64,
    overlap_s: f64,
) -> Vec<Window> {
    let sr = f64::from(sample_rate);
    let size = (window_s * sr) as usize;
    let step = ((window_s - overlap_s) * sr) as usize;
    if total_samples <= size || size == 0 || step == 0 {
        return vec![Window {
            start_sample: 0,
            end_sample: total_samples,
        }];
    }
    let mut out = Vec::new();
    let mut s = 0;
    while s < total_samples {
        out.push(Window {
            start_sample: s,
            end_sample: (s + size).min(total_samples),
        });
        if s + size >= total_samples {
            break;
        }
        s += step;
    }
    out
}

/// Shift window-relative tokens onto the global timeline
/// (`start += offset; end = start + duration`).
pub fn offset_tokens(tokens: &mut [AlignedToken], offset_s: f64) {
    for t in tokens {
        t.start += offset_s;
        t.end = t.start + t.duration;
    }
}

fn overlap_regions<'a>(
    a: &'a [AlignedToken],
    b: &'a [AlignedToken],
    overlap: f64,
) -> (Vec<&'a AlignedToken>, Vec<&'a AlignedToken>) {
    let a_end = a[a.len() - 1].end;
    let b_start = b[0].start;
    (
        a.iter().filter(|t| t.end > b_start - overlap).collect(),
        b.iter().filter(|t| t.start < a_end + overlap).collect(),
    )
}

fn cutoff_merge(a: &[AlignedToken], b: &[AlignedToken]) -> Vec<AlignedToken> {
    let cutoff = (a[a.len() - 1].end + b[0].start) / 2.0;
    a.iter()
        .filter(|t| t.end <= cutoff)
        .chain(b.iter().filter(|t| t.start >= cutoff))
        .cloned()
        .collect()
}

fn matches(x: &AlignedToken, y: &AlignedToken, overlap: f64) -> bool {
    x.id == y.id && (x.start - y.start).abs() < overlap / 2.0
}

/// Stitch `a` and `b` along matched index pairs (indices into `a` and `b`).
fn stitch(
    a: &[AlignedToken],
    b: &[AlignedToken],
    idx_a: &[usize],
    idx_b: &[usize],
) -> Vec<AlignedToken> {
    let mut result: Vec<AlignedToken> = a[..idx_a[0]].to_vec();
    for i in 0..idx_a.len() {
        result.push(a[idx_a[i]].clone());
        if i + 1 < idx_a.len() {
            let gap_a = &a[idx_a[i] + 1..idx_a[i + 1]];
            let gap_b = &b[idx_b[i] + 1..idx_b[i + 1]];
            result.extend_from_slice(if gap_b.len() > gap_a.len() { gap_b } else { gap_a });
        }
    }
    result.extend_from_slice(&b[idx_b[idx_b.len() - 1] + 1..]);
    result
}

/// `merge_longest_contiguous` from parakeet-mlx. Errors when the longest
/// contiguous matching run is shorter than half the overlap region of `a`.
pub fn merge_longest_contiguous(
    a: &[AlignedToken],
    b: &[AlignedToken],
    overlap_duration: f64,
) -> Result<Vec<AlignedToken>, MergeError> {
    if a.is_empty() || b.is_empty() {
        return Ok(if a.is_empty() { b.to_vec() } else { a.to_vec() });
    }
    if a[a.len() - 1].end <= b[0].start {
        return Ok([a, b].concat());
    }
    let (oa, ob) = overlap_regions(a, b, overlap_duration);
    let enough_pairs = oa.len() / 2;
    if oa.len() < 2 || ob.len() < 2 {
        return Ok(cutoff_merge(a, b));
    }

    let mut best: Vec<(usize, usize)> = Vec::new();
    for i in 0..oa.len() {
        for j in 0..ob.len() {
            if matches(oa[i], ob[j], overlap_duration) {
                let mut current = Vec::new();
                let (mut k, mut l) = (i, j);
                while k < oa.len() && l < ob.len() && matches(oa[k], ob[l], overlap_duration) {
                    current.push((k, l));
                    k += 1;
                    l += 1;
                }
                if current.len() > best.len() {
                    best = current;
                }
            }
        }
    }

    if best.len() >= enough_pairs {
        // Every reference caller has enough_pairs >= 1 here, so `best` is non-empty.
        let a_start = a.len() - oa.len();
        let idx_a: Vec<usize> = best.iter().map(|p| a_start + p.0).collect();
        let idx_b: Vec<usize> = best.iter().map(|p| p.1).collect();
        Ok(stitch(a, b, &idx_a, &idx_b))
    } else {
        Err(MergeError { enough_pairs })
    }
}

/// `merge_longest_common_subsequence` from parakeet-mlx.
pub fn merge_longest_common_subsequence(
    a: &[AlignedToken],
    b: &[AlignedToken],
    overlap_duration: f64,
) -> Vec<AlignedToken> {
    if a.is_empty() || b.is_empty() {
        return if a.is_empty() { b.to_vec() } else { a.to_vec() };
    }
    if a[a.len() - 1].end <= b[0].start {
        return [a, b].concat();
    }
    let (oa, ob) = overlap_regions(a, b, overlap_duration);
    if oa.len() < 2 || ob.len() < 2 {
        return cutoff_merge(a, b);
    }

    let mut dp = vec![vec![0usize; ob.len() + 1]; oa.len() + 1];
    for i in 1..=oa.len() {
        for j in 1..=ob.len() {
            dp[i][j] = if matches(oa[i - 1], ob[j - 1], overlap_duration) {
                dp[i - 1][j - 1] + 1
            } else {
                dp[i - 1][j].max(dp[i][j - 1])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (oa.len(), ob.len());
    while i > 0 && j > 0 {
        if matches(oa[i - 1], ob[j - 1], overlap_duration) {
            pairs.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    pairs.reverse();
    if pairs.is_empty() {
        return cutoff_merge(a, b);
    }
    let a_start = a.len() - oa.len();
    let idx_a: Vec<usize> = pairs.iter().map(|p| a_start + p.0).collect();
    let idx_b: Vec<usize> = pairs.iter().map(|p| p.1).collect();
    stitch(a, b, &idx_a, &idx_b)
}

/// Contiguous merge with LCS fallback, as in the reference runner; also
/// reports which strategy produced the result.
pub fn merge_window_traced(
    acc: &[AlignedToken],
    next: &[AlignedToken],
    overlap_duration: f64,
) -> (Vec<AlignedToken>, MergePath) {
    match merge_longest_contiguous(acc, next, overlap_duration) {
        Ok(r) => (r, MergePath::Contiguous),
        Err(_) => (
            merge_longest_common_subsequence(acc, next, overlap_duration),
            MergePath::Lcs,
        ),
    }
}

/// Contiguous merge with LCS fallback (see [`merge_window_traced`]).
pub fn merge_window(
    acc: &[AlignedToken],
    next: &[AlignedToken],
    overlap_duration: f64,
) -> Vec<AlignedToken> {
    merge_window_traced(acc, next, overlap_duration).0
}

/// A word assembled from SentencePiece tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

/// Group tokens into words: a token beginning with a space or `▁` starts a new
/// word; empty words are dropped (matches the bench `words_from_tokens`).
pub fn tokens_to_words(tokens: &[AlignedToken]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut cur: Option<Word> = None;
    for t in tokens {
        if cur.is_none() || t.text.starts_with(' ') || t.text.starts_with('▁') {
            if let Some(w) = cur.take() {
                if !w.word.is_empty() {
                    words.push(w);
                }
            }
            cur = Some(Word {
                word: t.text.trim().trim_start_matches('▁').to_string(),
                start: t.start,
                end: t.end,
            });
        } else if let Some(w) = cur.as_mut() {
            w.word.push_str(&t.text);
            w.end = t.end;
        }
    }
    if let Some(w) = cur {
        if !w.word.is_empty() {
            words.push(w);
        }
    }
    words
}

/// A sentence as produced by `tokens_to_sentences` (default config).
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    pub text: String,
    /// Tokens stably sorted by start time, like the reference.
    pub tokens: Vec<AlignedToken>,
}

fn push_sentence(out: &mut Vec<Sentence>, tokens: Vec<AlignedToken>) {
    let text: String = tokens.iter().map(|t| t.text.as_str()).collect();
    let mut tokens = tokens;
    tokens.sort_by(|x, y| x.start.total_cmp(&y.start));
    out.push(Sentence { text, tokens });
}

/// `tokens_to_sentences` with the default (unlimited) `SentenceConfig`:
/// a sentence ends after a token containing `!`, `?`, `。`, `？`, `！`, or a `.`
/// that is the last token or is followed by a token containing a space.
pub fn tokens_to_sentences(tokens: &[AlignedToken]) -> Vec<Sentence> {
    let mut sentences = Vec::new();
    let mut current: Vec<AlignedToken> = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        current.push(token.clone());
        let tx = &token.text;
        let is_punct = tx.contains('!')
            || tx.contains('?')
            || tx.contains('。')
            || tx.contains('？')
            || tx.contains('！')
            || (tx.contains('.') && (idx == tokens.len() - 1 || tokens[idx + 1].text.contains(' ')));
        if is_punct {
            push_sentence(&mut sentences, std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        push_sentence(&mut sentences, current);
    }
    sentences
}

/// Final text: concatenated sentence texts, stripped (`sentences_to_result(..).text`).
pub fn tokens_to_text(tokens: &[AlignedToken]) -> String {
    tokens_to_sentences(tokens)
        .iter()
        .map(|s| s.text.as_str())
        .collect::<String>()
        .trim()
        .to_string()
}

/// Merge per-window token streams (already offset to the global timeline, in
/// window order) exactly as the reference runner does.
pub fn merge_all(windows: Vec<Vec<AlignedToken>>, overlap_duration: f64) -> Vec<AlignedToken> {
    let mut acc: Vec<AlignedToken> = Vec::new();
    for w in windows {
        acc = if acc.is_empty() {
            w
        } else {
            merge_window(&acc, &w, overlap_duration)
        };
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk(id: u32, text: &str, start: f64) -> AlignedToken {
        AlignedToken::new(id, text, start, 0.08)
    }

    fn stream(n: u32) -> Vec<AlignedToken> {
        (0..n)
            .map(|i| tk(i + 1, &format!(" w{i}"), f64::from(i) * 0.1))
            .collect()
    }

    #[test]
    fn plan_single_window() {
        let w = plan_windows(16000 * 10, 16000, 15.0, 2.0);
        assert_eq!(w, vec![Window { start_sample: 0, end_sample: 160000 }]);
        assert_eq!(plan_windows(0, 16000, 15.0, 2.0).len(), 1);
    }

    #[test]
    fn plan_multi_window() {
        // 40s, 15s window, 2s overlap => step 13s: starts 0,13,26; 26+15>=40 stops.
        let w = plan_windows(16000 * 40, 16000, 15.0, 2.0);
        let starts: Vec<_> = w.iter().map(|x| x.start_sample).collect();
        assert_eq!(starts, vec![0, 208000, 416000]);
        assert_eq!(w[2].end_sample, 640000);
        // exact fit: 28s => 0,13; 13+15>=28 stops.
        assert_eq!(plan_windows(16000 * 28, 16000, 15.0, 2.0).len(), 2);
    }

    #[test]
    fn plan_invalid_params_single_window() {
        assert_eq!(plan_windows(1000, 16000, 0.0, 0.0).len(), 1);
        assert_eq!(plan_windows(16000 * 40, 16000, 5.0, 5.0).len(), 1);
    }

    #[test]
    fn empty_inputs() {
        let s = stream(3);
        assert_eq!(merge_window(&[], &s, 2.0), s);
        assert_eq!(merge_window(&s, &[], 2.0), s);
        assert!(merge_window(&[], &[], 2.0).is_empty());
    }

    #[test]
    fn no_overlap_concatenates() {
        let a = stream(3);
        let mut b = stream(3);
        offset_tokens(&mut b, 10.0);
        assert_eq!(merge_window(&a, &b, 2.0).len(), 6);
    }

    #[test]
    fn identical_tokens_dedupe() {
        let a = stream(10);
        let merged = merge_window(&a, &a, 2.0);
        assert_eq!(merged, a);
    }

    #[test]
    fn lcs_fallback_when_contiguous_fails() {
        // Alternating matches: longest contiguous run is 1, needs >= 3 of 6.
        let a: Vec<_> = (0..6).map(|i| tk(i + 1, "x", f64::from(i) * 0.1)).collect();
        let b: Vec<_> = (0..6)
            .map(|i| tk(if i % 2 == 0 { i + 1 } else { 100 + i }, "y", f64::from(i) * 0.1))
            .collect();
        assert!(merge_longest_contiguous(&a, &b, 2.0).is_err());
        let (_, path) = merge_window_traced(&a, &b, 2.0);
        assert_eq!(path, MergePath::Lcs);
    }

    #[test]
    fn words_and_text() {
        let toks = vec![
            tk(1, " Hej", 0.0),
            tk(2, "san", 0.1),
            tk(3, " du", 0.2),
            tk(4, ".", 0.3),
            tk(5, " Nu", 0.4),
        ];
        let words = tokens_to_words(&toks);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].word, "Hejsan");
        assert!((words[0].end - 0.18).abs() < 1e-9);
        assert_eq!(words[1].word, "du.");
        assert_eq!(tokens_to_text(&toks), "Hejsan du. Nu");
        assert_eq!(tokens_to_sentences(&toks).len(), 2);
    }

    #[test]
    fn property_exact_overlaps_reconstruct_stream() {
        // Deterministic LCG for varied stream lengths and window geometries.
        let mut seed = 12345u64;
        let mut rnd = |m: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % m
        };
        for _ in 0..200 {
            let n = 60 + rnd(200) as usize;
            let full: Vec<AlignedToken> = (0..n)
                .map(|i| tk(1 + (i as u32 * 7) % 997, &format!(" t{i}"), i as f64 * 0.1))
                .collect();
            let win = 20 + rnd(20) as usize;
            let ov = 6 + rnd(8) as usize;
            let step = win - ov;
            let overlap_s = ov as f64 * 0.1;
            let mut acc: Vec<AlignedToken> = Vec::new();
            let mut s = 0;
            loop {
                let e = (s + win).min(n);
                let w = full[s..e].to_vec();
                acc = if acc.is_empty() {
                    w
                } else {
                    merge_window(&acc, &w, overlap_s + 0.05)
                };
                if e >= n {
                    break;
                }
                s += step;
            }
            assert_eq!(acc, full, "n={n} win={win} ov={ov}");
        }
    }
}
