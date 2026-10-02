//! Context biasing (shallow-fusion phrase boosting) for the greedy TDT decoder.
//!
//! Dictionary terms are tokenized with the model's SentencePiece pieces into a prefix trie.
//! While decoding, a token that starts a term, or continues a live partial match, gets a
//! logit bonus before the argmax. Matches are abandoned when the emitted token does not
//! continue them, and after a gap of silence. Mirrors the Swift Core ML host
//! (`ContextBiasing.swift`); original implementation.

use crate::vocab::Vocab;
use std::collections::HashMap;

pub const MAX_BOOST_TERMS: usize = 500;
pub const MAX_BOOST_TERM_CHARS: usize = 64;
const MAX_PIECE_CHARS: usize = 24;

/// Segmentations of one word (`▁` prepended): fewest pieces, and longest-match-first.
pub fn segment_word(word: &str, pieces: &HashMap<&str, u32>) -> Vec<Vec<u32>> {
    let chars: Vec<char> = std::iter::once('▁').chain(word.chars()).collect();
    let n = chars.len();
    let slice = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
    let mut best: Vec<Option<Vec<u32>>> = vec![None; n + 1];
    best[0] = Some(Vec::new());
    for end in 1..=n {
        for start in end.saturating_sub(MAX_PIECE_CHARS)..end {
            let Some(prefix) = best[start].clone() else { continue };
            let Some(&id) = pieces.get(slice(start, end).as_str()) else { continue };
            if best[end].as_ref().is_none_or(|b| prefix.len() + 1 < b.len()) {
                let mut seq = prefix;
                seq.push(id);
                best[end] = Some(seq);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(min) = best[n].take() {
        out.push(min);
    }
    let mut greedy = Vec::new();
    let mut pos = 0;
    'outer: while pos < n {
        let mut end = n.min(pos + MAX_PIECE_CHARS);
        while end > pos {
            if let Some(&id) = pieces.get(slice(pos, end).as_str()) {
                greedy.push(id);
                pos = end;
                continue 'outer;
            }
            end -= 1;
        }
        greedy.clear();
        break;
    }
    if !greedy.is_empty() && !out.contains(&greedy) {
        out.push(greedy);
    }
    out
}

#[derive(Debug, Default)]
pub struct BoostTrie {
    children: Vec<HashMap<u32, usize>>, // node 0 = root
    pub weight: f32,
    phrases: usize,
}

impl BoostTrie {
    pub fn new(terms: &[String], weight: f32, vocab: &Vocab) -> Self {
        let blank = vocab.blank_id();
        let mut pieces: HashMap<&str, u32> = HashMap::new();
        for id in 0..vocab.len() as u32 {
            if let Some(p) = vocab.piece(id) {
                if id != blank && !p.is_empty() && !p.starts_with('<') {
                    pieces.insert(p, id);
                }
            }
        }
        let mut trie = Self { children: vec![HashMap::new()], weight, phrases: 0 };
        for raw in terms.iter().take(MAX_BOOST_TERMS) {
            let term = raw.trim();
            if term.is_empty() || term.chars().count() > MAX_BOOST_TERM_CHARS {
                continue;
            }
            let mut forms = vec![term.to_string()];
            let mut cs = term.chars();
            let capitalized: String = cs.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default() + cs.as_str();
            if capitalized != term {
                forms.push(capitalized);
            }
            for form in forms {
                let mut sequences: Vec<Vec<u32>> = vec![Vec::new()];
                for word in form.split_whitespace() {
                    let options = segment_word(word, &pieces);
                    if options.is_empty() {
                        sequences.clear();
                        break;
                    }
                    sequences = sequences
                        .iter()
                        .flat_map(|p| options.iter().map(move |o| [p.as_slice(), o.as_slice()].concat()))
                        .collect();
                }
                for seq in sequences.into_iter().filter(|s| !s.is_empty()) {
                    trie.insert(&seq);
                }
            }
        }
        trie
    }

    pub fn insert(&mut self, seq: &[u32]) {
        let mut node = 0;
        for &token in seq {
            node = match self.children[node].get(&token) {
                Some(&next) => next,
                None => {
                    self.children.push(HashMap::new());
                    let next = self.children.len() - 1;
                    self.children[node].insert(token, next);
                    next
                }
            };
        }
        self.phrases += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.children[0].is_empty()
    }

    #[cfg(test)]
    pub fn phrase_count(&self) -> usize {
        self.phrases
    }
}

/// Live partial matches for one decode (the root is implicit).
pub struct BiasState<'a> {
    trie: &'a BoostTrie,
    start_factor: f32,
    max_gap_frames: usize,
    active: Vec<usize>,
    last_emit_frame: usize,
}

impl<'a> BiasState<'a> {
    pub fn new(trie: &'a BoostTrie, start_factor: f32) -> Self {
        Self { trie, start_factor, max_gap_frames: 8, active: Vec::new(), last_emit_frame: 0 }
    }

    /// Apply bonuses to `logits[..vocab]` and return the chosen token, or `None` when the
    /// model's own argmax stands (so output without a dictionary is unchanged).
    pub fn choose(&mut self, token_logits: &[f32], frame: usize) -> Option<usize> {
        if !self.active.is_empty() && frame.saturating_sub(self.last_emit_frame) > self.max_gap_frames {
            self.active.clear();
        }
        let mut best = 0;
        for (i, v) in token_logits.iter().enumerate() {
            if *v > token_logits[best] {
                best = i;
            }
        }
        let mut chosen = best;
        let mut score = token_logits[best];
        let w = self.trie.weight;
        let mut consider = |token: u32, bonus: f32| {
            let i = token as usize;
            if i < token_logits.len() && token_logits[i] + bonus > score {
                score = token_logits[i] + bonus;
                chosen = i;
            }
        };
        // Root children first at the reduced start bonus; continuations override with full weight.
        let mut bonuses: HashMap<u32, f32> = HashMap::new();
        for &t in self.trie.children[0].keys() {
            bonuses.insert(t, w * self.start_factor);
        }
        for &node in &self.active {
            for &t in self.trie.children[node].keys() {
                bonuses.insert(t, w);
            }
        }
        // Deterministic order for ties.
        let mut entries: Vec<_> = bonuses.into_iter().collect();
        entries.sort_by_key(|(t, _)| *t);
        for (t, b) in entries {
            consider(t, b);
        }
        (chosen != best).then_some(chosen)
    }

    /// Advance partial matches after a non-blank `token` was emitted at `frame`.
    pub fn emitted(&mut self, token: u32, frame: usize) {
        let mut next = Vec::new();
        let roots = std::iter::once(0usize).chain(self.active.to_vec());
        for node in roots {
            if let Some(&child) = self.trie.children[node].get(&token) {
                if !self.trie.children[child].is_empty() && !next.contains(&child) {
                    next.push(child);
                }
            }
        }
        self.active = next;
        self.last_emit_frame = frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vocab {
        // ids: 0 "▁Mag", 1 "nus", 2 "▁M", 3 "ag", 4 "▁hej", 5 "▁Gil", 6 "le", 7 "<blk>"
        Vocab::parse("▁Mag 0\nnus 1\n▁M 2\nag 3\n▁hej 4\n▁Gil 5\nle 6\n<blk> 7\n").unwrap()
    }

    fn terms(list: &[&str], w: f32) -> BoostTrie {
        BoostTrie::new(&list.iter().map(|s| s.to_string()).collect::<Vec<_>>(), w, &vocab())
    }

    fn logits(pairs: &[(usize, f32)]) -> Vec<f32> {
        let mut l = vec![-10.0; 8];
        for (i, v) in pairs {
            l[*i] = *v;
        }
        l
    }

    #[test]
    fn tokenizes_with_alternative_segmentations() {
        let pieces: HashMap<&str, u32> = [("▁Mag", 0), ("nus", 1), ("▁M", 2), ("ag", 3)].into_iter().collect();
        let segs = segment_word("Magnus", &pieces);
        assert_eq!(segs, vec![vec![0, 1]]); // min == greedy here
        // "ag" alone has two segmentations (min-count differs from nothing here): single piece.
        assert_eq!(segment_word("Mag", &pieces), vec![vec![0]]);
        assert!(segment_word("zzz", &pieces).is_empty());
    }

    #[test]
    fn builds_multiword_phrases_and_skips_untokenizable() {
        let t = terms(&["Magnus Gille", "Qzx"], 2.0);
        assert!(!t.is_empty());
        assert!(t.phrase_count() >= 1);
        assert!(terms(&["Qzx"], 2.0).is_empty());
    }

    #[test]
    fn empty_dictionary_never_changes_the_choice() {
        let t = terms(&[], 5.0);
        let mut s = BiasState::new(&t, 1.0);
        assert_eq!(s.choose(&logits(&[(4, 1.0), (0, 0.9)]), 0), None);
    }

    #[test]
    fn bonus_flips_a_close_call_but_not_a_clear_one() {
        let t = terms(&["Magnus"], 2.0);
        let mut s = BiasState::new(&t, 1.0);
        // "▁Mag"(0) is 1.5 behind blank(7): bonus 2 wins.
        assert_eq!(s.choose(&logits(&[(7, 3.0), (0, 1.5)]), 0), Some(0));
        // 5 behind: bonus 2 is not enough.
        assert_eq!(s.choose(&logits(&[(7, 8.0), (0, 3.0)]), 0), None);
    }

    #[test]
    fn continuation_is_boosted_only_after_the_prefix_matched() {
        let t = terms(&["Magnus"], 2.0);
        let mut s = BiasState::new(&t, 0.0); // no start bonus: prefix must come from the model
        assert_eq!(s.choose(&logits(&[(7, 1.0), (1, 0.0)]), 0), None);
        s.emitted(0, 0);
        assert_eq!(s.choose(&logits(&[(7, 1.0), (1, 0.0)]), 1), Some(1));
    }

    #[test]
    fn abandoned_match_is_dropped() {
        let t = terms(&["Magnus"], 2.0);
        let mut s = BiasState::new(&t, 0.0);
        s.emitted(0, 0);
        s.emitted(4, 1); // "▁hej" does not continue the path
        assert_eq!(s.choose(&logits(&[(7, 1.0), (1, 0.0)]), 2), None);
    }

    #[test]
    fn silence_gap_resets_partial_matches() {
        let t = terms(&["Magnus"], 2.0);
        let mut s = BiasState::new(&t, 0.0);
        s.emitted(0, 0);
        assert_eq!(s.choose(&logits(&[(7, 1.0), (1, 0.0)]), 30), None);
    }

    #[test]
    fn multiple_partial_matches_are_tracked() {
        let t = terms(&["Magnus", "Mag Gille"], 2.0);
        let mut s = BiasState::new(&t, 0.0);
        s.emitted(0, 0); // "▁Mag" starts both phrases
        // "nus"(1) continues the first; "▁Gil"(5) continues the second.
        assert_eq!(s.choose(&logits(&[(7, 1.0), (1, 0.0)]), 1), Some(1));
        assert_eq!(s.choose(&logits(&[(7, 1.0), (5, 0.0)]), 1), Some(5));
    }

    #[test]
    fn weight_zero_start_factor_keeps_unrelated_audio_unchanged() {
        let t = terms(&["Magnus"], 2.0);
        let mut s = BiasState::new(&t, 0.0);
        assert_eq!(s.choose(&logits(&[(7, 1.0), (0, 0.9)]), 0), None);
    }
}
