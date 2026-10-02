//! Context biasing (shallow-fusion phrase boosting) for the greedy TDT decoder.
//!
//! Dictionary terms are tokenized with the model's SentencePiece pieces into a prefix trie.
//! While decoding, a token that starts a term, or continues a live partial match, gets a
//! logit bonus before the argmax. Matches are abandoned when the emitted token does not
//! continue them, and after a gap of silence. Mirrors the Swift Core ML host
//! (`ContextBiasing.swift`); original implementation.
//!
//! Parity contract: both hosts load `engine-host/test-vectors/context-biasing.json`. Lengths and
//! segmentation use Unicode scalars (`char`), candidates are the 64 highest token logits (ties
//! to the lower id), and the model's own choice is the highest logit (lowest id on ties).

use crate::vocab::Vocab;
use std::collections::HashMap;

pub const MAX_BOOST_TERMS: usize = 500;
pub const MAX_BOOST_TERM_CHARS: usize = 64;
pub const MAX_NODES: usize = 50_000;
pub const TOP_K: usize = 64;
/// Segmentation alternatives are combined per term only up to this many token sequences.
pub const MAX_SEQUENCES_PER_TERM: usize = 16;
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

/// Immutable prefix trie over token ids; shared across windows through [`BoostTrieCache`].
#[derive(Debug, Default)]
pub struct BoostTrie {
    children: Vec<HashMap<u32, usize>>, // node 0 = root
    #[cfg_attr(not(test), allow(dead_code))]
    terminal: Vec<bool>,
}

impl BoostTrie {
    pub fn new(terms: &[String], vocab: &Vocab) -> Self {
        let blank = vocab.blank_id();
        let mut pieces: HashMap<&str, u32> = HashMap::new();
        for id in 0..vocab.len() as u32 {
            if let Some(p) = vocab.piece(id) {
                if id != blank && !p.is_empty() && !p.starts_with('<') {
                    pieces.insert(p, id);
                }
            }
        }
        let mut trie = Self { children: vec![HashMap::new()], terminal: vec![false] };
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
                        .take(MAX_SEQUENCES_PER_TERM)
                        .collect();
                }
                for seq in sequences.into_iter().filter(|s| !s.is_empty()) {
                    trie.insert(&seq);
                }
            }
        }
        trie
    }

    fn insert(&mut self, seq: &[u32]) {
        if self.children.len() + seq.len() > MAX_NODES {
            return;
        }
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
        self.terminal.resize(self.children.len(), false);
        self.terminal[node] = true;
    }

    pub fn is_empty(&self) -> bool {
        self.children[0].is_empty()
    }

    #[cfg(test)]
    pub fn node_count(&self) -> usize {
        self.children.len()
    }

    /// Every complete phrase as a token sequence, sorted (tests and diagnostics).
    #[cfg(test)]
    pub fn phrases(&self) -> Vec<Vec<u32>> {
        fn walk(trie: &BoostTrie, node: usize, path: &mut Vec<u32>, out: &mut Vec<Vec<u32>>) {
            if trie.terminal.get(node).copied().unwrap_or(false) {
                out.push(path.clone());
            }
            for (&token, &child) in &trie.children[node] {
                path.push(token);
                walk(trie, child, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(self, 0, &mut Vec::new(), &mut out);
        out.sort();
        out
    }
}

/// Small LRU of tries keyed by the term list, so consecutive windows and dictations with the same
/// dictionary build the trie once per loaded model (the cache lives with the vocabulary).
#[derive(Default)]
pub struct BoostTrieCache {
    entries: std::sync::Mutex<Vec<(String, std::sync::Arc<BoostTrie>)>>,
}

impl BoostTrieCache {
    const CAPACITY: usize = 4;

    /// Returns the trie and the build time in microseconds (0 on a cache hit).
    pub fn get(&self, terms: &[String], vocab: &Vocab) -> (std::sync::Arc<BoostTrie>, u64) {
        let key = terms.join("\u{1F}");
        {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(index) = entries.iter().position(|(k, _)| *k == key) {
                let entry = entries.remove(index);
                let trie = entry.1.clone();
                entries.push(entry);
                return (trie, 0);
            }
        }
        let started = std::time::Instant::now();
        let built = std::sync::Arc::new(BoostTrie::new(terms, vocab));
        let micros = started.elapsed().as_micros() as u64;
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.push((key, built.clone()));
        if entries.len() > Self::CAPACITY {
            entries.remove(0);
        }
        (built, micros)
    }
}

/// Bonus and gating parameters for one decode (the weight comes from the request).
#[derive(Debug, Clone, Copy)]
pub struct BiasParams {
    pub weight: f32,
    /// Start-of-term tokens get `weight * start_factor`; continuations get `weight`.
    pub start_factor: f32,
    /// A start-of-term bonus may override a blank only when the term's raw logit is within this
    /// margin of the blank logit (continuations are never gated).
    pub blank_margin: f32,
    /// Partial matches are dropped after this many frames without an emitted token.
    pub max_gap_frames: usize,
    /// A boosted token that overrode blank skips at most this many frames.
    pub override_duration_cap: usize,
}

impl BiasParams {
    pub fn new(weight: f32) -> Self {
        Self { weight, start_factor: 0.25, blank_margin: 4.0, max_gap_frames: 8, override_duration_cap: 1 }
    }

    /// Measurement overrides: `SAGASCRIPT_BOOST_START_FACTOR`, `SAGASCRIPT_BOOST_BLANK_MARGIN`,
    /// `SAGASCRIPT_BOOST_OVERRIDE_DURATION_CAP`.
    pub fn from_env(weight: f32) -> Self {
        let mut p = Self::new(weight);
        let get = |name: &str| std::env::var(name).ok();
        if let Some(v) = get("SAGASCRIPT_BOOST_START_FACTOR").and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()) {
            p.start_factor = v.clamp(0.0, 1.0);
        }
        if let Some(v) = get("SAGASCRIPT_BOOST_BLANK_MARGIN").and_then(|v| v.parse::<f32>().ok()).filter(|v| !v.is_nan()) {
            p.blank_margin = v.max(0.0);
        }
        if let Some(v) = get("SAGASCRIPT_BOOST_OVERRIDE_DURATION_CAP").and_then(|v| v.parse::<usize>().ok()) {
            p.override_duration_cap = v;
        }
        p
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BiasChoice {
    pub id: u32,
    /// The model's own choice was blank. The duration head described that blank, not this token.
    pub overrode_blank: bool,
}

/// Live partial matches for one decode (the root is implicit). Reuses its buffers across frames.
pub struct BiasState<'a> {
    trie: &'a BoostTrie,
    params: BiasParams,
    blank: u32,
    active: Vec<usize>,
    next: Vec<usize>,
    candidates: Vec<(f32, u32)>,
    last_emit_frame: usize,
}

/// Candidate order: higher logit first, ties to the lower id.
fn better(a: &(f32, u32), b: &(f32, u32)) -> std::cmp::Ordering {
    b.0.total_cmp(&a.0).then(a.1.cmp(&b.1))
}

impl<'a> BiasState<'a> {
    pub fn new(trie: &'a BoostTrie, params: BiasParams, blank: u32) -> Self {
        Self {
            trie,
            params,
            blank,
            active: Vec::new(),
            next: Vec::new(),
            candidates: Vec::new(),
            last_emit_frame: 0,
        }
    }

    pub fn params(&self) -> &BiasParams {
        &self.params
    }

    /// Apply bonuses to the top-64 of `token_logits` and return the chosen token, or `None` when
    /// the model's own choice (highest logit, lowest id on ties) stands, so output without a
    /// dictionary is unchanged.
    pub fn choose(&mut self, token_logits: &[f32], frame: usize) -> Option<BiasChoice> {
        if !self.active.is_empty() && frame.saturating_sub(self.last_emit_frame) > self.params.max_gap_frames {
            self.active.clear();
        }
        if token_logits.is_empty() {
            return None;
        }
        self.candidates.clear();
        self.candidates.extend(token_logits.iter().enumerate().map(|(i, v)| (*v, i as u32)));
        if self.candidates.len() > TOP_K {
            self.candidates.select_nth_unstable_by(TOP_K - 1, better);
            self.candidates.truncate(TOP_K);
        }
        let original = *self.candidates.iter().min_by(|a, b| better(a, b)).expect("non-empty");
        let overriding_blank = original.1 == self.blank;
        let blank_logit = self
            .candidates
            .iter()
            .find(|c| c.1 == self.blank)
            .map_or(f32::NEG_INFINITY, |c| c.0);
        let mut chosen = original;
        let mut chosen_score = original.0;
        for &(logit, token) in &self.candidates {
            if token == original.1 {
                continue;
            }
            let mut bonus = 0.0f32;
            if self.active.iter().any(|&n| self.trie.children[n].contains_key(&token)) {
                bonus = self.params.weight;
            } else if self.trie.children[0].contains_key(&token)
                && (!overriding_blank || logit >= blank_logit - self.params.blank_margin)
            {
                bonus = self.params.weight * self.params.start_factor;
            }
            if bonus <= 0.0 {
                continue;
            }
            let score = logit + bonus;
            if score > chosen_score || (score == chosen_score && token < chosen.1) {
                chosen = (logit, token);
                chosen_score = score;
            }
        }
        (chosen.1 != original.1).then_some(BiasChoice { id: chosen.1, overrode_blank: overriding_blank })
    }

    /// Advance partial matches after a non-blank `token` was emitted at `frame`.
    pub fn emitted(&mut self, token: u32, frame: usize) {
        self.next.clear();
        // The implicit root is tried first, then the live matches.
        for index in 0..=self.active.len() {
            let node = if index == 0 { 0 } else { self.active[index - 1] };
            if let Some(&child) = self.trie.children[node].get(&token) {
                if !self.trie.children[child].is_empty() && !self.next.contains(&child) {
                    self.next.push(child);
                }
            }
        }
        std::mem::swap(&mut self.active, &mut self.next);
        self.last_emit_frame = frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../test-vectors/context-biasing.json");

    fn vectors() -> (Value, Vocab) {
        let root: Value = serde_json::from_str(VECTORS).unwrap();
        let text: String = root["vocab"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{} {i}\n", p.as_str().unwrap()))
            .collect();
        let vocab = Vocab::parse(&text).unwrap();
        assert_eq!(vocab.blank_id() as u64, root["blank_id"].as_u64().unwrap());
        (root, vocab)
    }

    fn strings(v: &Value) -> Vec<String> {
        v.as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn shared_vectors_trie_construction() {
        let (root, vocab) = vectors();
        assert_eq!(root["top_k"].as_u64().unwrap() as usize, TOP_K);
        for case in root["trie_cases"].as_array().unwrap() {
            let trie = BoostTrie::new(&strings(&case["terms"]), &vocab);
            let expected: Vec<Vec<u32>> = case["phrases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_array().unwrap().iter().map(|t| t.as_u64().unwrap() as u32).collect())
                .collect();
            assert_eq!(trie.phrases(), expected, "trie case {}", case["name"]);
        }
    }

    #[test]
    fn shared_vectors_choice() {
        let (root, vocab) = vectors();
        let default = root["default_logit"].as_f64().unwrap() as f32;
        for case in root["choose_cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let p = &case["params"];
            let mut params = BiasParams::new(p["weight"].as_f64().unwrap() as f32);
            params.start_factor = p["start_factor"].as_f64().unwrap() as f32;
            params.blank_margin = p["blank_margin"].as_f64().unwrap() as f32;
            params.max_gap_frames = p["max_gap_frames"].as_u64().unwrap() as usize;
            let trie = BoostTrie::new(&strings(&case["terms"]), &vocab);
            let mut state = BiasState::new(&trie, params, vocab.blank_id());
            for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                if let Some(emit) = step.get("emit") {
                    state.emitted(emit["token"].as_u64().unwrap() as u32, emit["frame"].as_u64().unwrap() as usize);
                }
                let mut logits = vec![default; vocab.len()];
                for (key, value) in step["logits"].as_object().unwrap() {
                    logits[key.parse::<usize>().unwrap()] = value.as_f64().unwrap() as f32;
                }
                let pick = state.choose(&logits, step["frame"].as_u64().unwrap() as usize);
                match step["expected"].as_object() {
                    Some(e) => {
                        let want = BiasChoice {
                            id: e["id"].as_u64().unwrap() as u32,
                            overrode_blank: e["overrode_blank"].as_bool().unwrap(),
                        };
                        assert_eq!(pick, Some(want), "{name} step {index}");
                    }
                    None => assert_eq!(pick, None, "{name} step {index}"),
                }
            }
        }
    }

    #[test]
    fn node_cap_bounds_the_trie() {
        let mut text = String::from("▁a 0\n");
        for i in 1..200 {
            text.push_str(&format!("x{i} {i}\n"));
        }
        text.push_str("<blk> 200\n");
        let vocab = Vocab::parse(&text).unwrap();
        let terms: Vec<String> = (1..200).map(|i| format!("ax{i}")).collect();
        assert!(BoostTrie::new(&terms, &vocab).node_count() <= MAX_NODES);
    }

    #[test]
    fn cache_reuses_the_trie_for_the_same_terms() {
        let (_, vocab) = vectors();
        let cache = BoostTrieCache::default();
        let terms = vec!["Magnus".to_string()];
        let (first, _) = cache.get(&terms, &vocab);
        let (second, micros) = cache.get(&terms, &vocab);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
        assert_eq!(micros, 0);
        let (other, _) = cache.get(&["Gille".to_string()], &vocab);
        assert!(!std::sync::Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn choose_with_a_vocabulary_smaller_than_top_k_still_works() {
        let (_, vocab) = vectors();
        let trie = BoostTrie::new(&["Magnus".to_string()], &vocab);
        let mut params = BiasParams::new(2.0);
        params.start_factor = 1.0;
        let mut state = BiasState::new(&trie, params, 3);
        // 4 tokens only: blank is id 3, "▁Mag" is id 0.
        assert_eq!(state.choose(&[1.5, -5.0, -5.0, 3.0], 0).map(|c| c.id), Some(0));
    }
}
