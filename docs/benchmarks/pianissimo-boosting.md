# Pianissimo context biasing (dictionary-steered decoding), phase 1

Issue #298 (epic #299). Date 2026-10-02. Offline measurements on the Mac (Core ML host, ANE), no device
control, audio files only. Scripts and term lists: `docs/benchmarks/data/pianissimo-boosting-20261002/`.

## Approach

- **Feasibility:** the installed Core ML joint (`JointDecisionv3`) emits `token_id`, `token_prob`, `duration`
  and also `top_k_ids` / `top_k_logits` (K = 64 token logits, blank included), so biasing needs no model
  change. The ONNX host sees all logits.
- **Method:** shallow-fusion phrase boosting with a token trie (original implementation; the NeMo
  boosting-tree idea, no code ported). Dictionary terms are tokenized with the model's SentencePiece pieces
  (fewest-pieces and longest-match segmentations, plus a capitalised variant) into a prefix trie. In the
  greedy TDT loop every candidate token that continues a live partial match gets `+weight` on its logit, a
  token that starts a term gets `+weight x start factor` (0.25 in the first pass, **0 now**); the argmax is taken over the adjusted logits. Several
  partial matches are tracked at once; a match is dropped when the emitted token does not continue it or after
  8 frames (0.64 s) without a token (partial matches never carry across a window, see "Window boundaries").
  Greedy decoding cannot retroactively undo a committed prefix, so the
  rollback penalty of beam-based boosting trees is replaced by the reduced start bonus (the main
  false-positive control, see below). No dictionary: the joint's own argmax is used, output unchanged.
- **Where:** Swift `ContextBiasing.swift` + `TdtDecoder.swift` (Core ML host), Rust `boost.rs` + `tdt.rs`
  (ONNX host, unit-tested; not measured end to end in this phase). Both hosts load the same test vectors
  (`src-tauri/engine-host/test-vectors/context-biasing.json`: vocabulary, terms, logit sequences, expected
  tokens), use the same candidate set (top-64 logits), count lengths in Unicode scalars and cache the trie
  per term list. Protocol fields `boost_terms` (<= 500
  terms, <= 64 chars) and `boost_weight` (0..=20) on `transcribe_window` (backwards compatible, documented in
  `docs/engine-host-protocol.md`), Rust client `BoostSpec`, CLI `sagascript transcribe --model pianissimo-sv
  --glossary-boost <weight> [--boost-terms-file <file>]` (terms = profile glossary canonical terms plus file).

## Setup

FLEURS sv distinct 15 min (76 name tokens) and 60 min (272), dictionary = all capitalised non-sentence-initial
reference words (68 / 226 terms, an oracle dictionary); Riksdag debate (73 min, 201 name tokens), dictionary
= the 7 speakers' full names, first names and surnames (20 terms). Name recall = README method
(`docs/benchmarks/data/names-20260930/names.py`). **False-positive check:** the same audio with ~47 unrelated
Swedish surnames, full names and company names that do not occur in the reference; `false ins` counts
occurrences of those terms in the output that the reference does not contain. Weight 0 is today's output.

## Results of the first pass (SUPERSEDED: oracle dictionary, start factor 0.25, measured before the scoring fix below)

| Audio | Weight | Name recall | WER | | Unrelated dictionary: name recall | WER | false ins |
|---|---|---|---|---|---|---|---|
| FLEURS 15 | 0 | 75.0 | 6.62 | | 75.0 | 6.62 | 0 |
| FLEURS 15 | 3 | 78.9 | 6.55 | | 73.7 | 6.93 | 0 |
| FLEURS 15 | 5 | **81.6** | 6.55 | | 73.7 | 6.99 | 0 |
| FLEURS 60 | 0 | 66.9 | 7.06 | | 66.9 | 7.06 | 0 |
| FLEURS 60 | 3 | 74.3 | 6.77 | | 65.8 | 7.20 | 0 |
| FLEURS 60 | 5 | **76.8** | 6.74 | | 66.2 | 7.23 | 0 |
| FLEURS 60 | 8 | 79.0 | 7.10 | | 65.1 | 7.50 | 4 |
| Riksdag | 0 | 93.0 | 23.40 | | 93.0 | 23.40 | 0 |
| Riksdag | 3 | 96.5 | 23.36 | | 93.5 | 23.36 | 0 |
| Riksdag | 5 | **97.0** | 23.38 | | 93.0 | 23.37 | 0 |
| Riksdag | 8 | 94.5 | 23.64 | | 93.0 | 23.55 | 1 |

Flat bonus (start factor 1.0) is clearly worse: FLEURS 60 weight 5 gives 76.1 recall but WER 9.85, and with
the unrelated dictionary recall 61.8, WER 10.07 and 7 false insertions (FLEURS 15 weight 5: WER 8.00).
Factors 0.5 and 0.1 were also tried and behave like 0.25 (0.1 is slightly safer, 0.5 slightly worse on the
unrelated dictionary); the full raw numbers are in `scores.jsonl` / `scores2.txt` in the data directory.

## Speed (first pass)

Total CLI time and summed per-window decode time with and without a 68 / 226 term dictionary (weight 5,
three alternating repeats) were inside run-to-run noise on the long files. The dictation-length measurement
is in "Review follow-up, e" below.

## Findings

- With the dictionary, name recall rises +6.6 (15 min) and +9.9 (60 min) points at weight 5 with WER equal
  or better (-0.07 / -0.32), and +4 points on the real Riksdag debate (93.0 to 97.0), WER unchanged.
- The Riksdag misses that remain are spelling variants of homophones, not decoding near-misses:
  the model writes "Karlsson" 14 times where the reference has "Carlsson", and boosting the trie for
  "Carlsson" does not flip it. Alias-style dictionary entries (`Carlsson = Karlsson`) already cover this
  through the existing replacement post-step; boosting does not replace it.
- Weight is sensitive: 8 begins to hurt WER and produces false insertions; 5 with start factor 0.25 is the
  best measured trade-off. A dictionary of names absent from the audio costs 0.2-0.4 WER points and ~1 point
  of name recall (unrelated terms occasionally hijack a real word) with no or very few false insertions at
  weight <= 5.
- Limits of this phase: the Core ML host can only boost candidates inside the joint's top 64; the dictionary
  here is an oracle (all names of the reference); **names the model has not seen** (the issue's +15 point
  target) were not measured because that needs new recorded audio; Windows on ARM (ONNX) is unit-tested and
  shares the algorithm but was not measured; one repeat per accuracy cell (decoding is deterministic).

## Review follow-up (2026-10-02/03): all numbers below describe the current head

Two review rounds. Round 2 found that both hosts skipped the model's own winner when applying bonuses (a
dictionary token that was already winning got no bonus and could be displaced by a weaker dictionary start);
every candidate is now scored by the same rules, and the default start factor is now **0** (only
continuations of a prefix the model itself emitted are boosted; start-of-term bonus = weight x start factor,
configurable with `SAGASCRIPT_BOOST_START_FACTOR`). All tables in this section were re-measured at that
head (Core ML host, ANE, audio files only). Rows marked "sf 0.25" use the old default start factor with the
scoring fix, to show what the factor costs; `data/.../review-followup/report3-pre-scoringfix.out` holds the
round-1 numbers (scoring bug present, sf 0.25), which are superseded. Scripts and outputs:
`docs/benchmarks/data/pianissimo-boosting-20261002/review-followup/` (dictionaries are regenerated from
`mk.py` seeds). Dictionaries use Swedish surnames and place names that occur in no reference (481 after
filtering, public names only). "Changed" = words that differ from the same audio without a dictionary.
Other mechanics: top-64 candidates, blank-override gate (raw margin 4.0) and 1-frame skip cap, trie cache.

### Ablation: unrelated 47-term dictionary, weight 5 (WER, words changed)

| Audio (baseline WER) | New default (sf 0) | sf 0.25 | sf 0.25, no blank gate, cap 4 |
|---|---|---|---|
| FLEURS 15 (6.62) | 6.68, 3 changed | 6.99, 15 | 6.99, 15 |
| FLEURS 60 (7.06) | 7.07, 3 | 7.23, 43 | 7.23, 43 |
| Riksdag (23.40) | 23.40, 2 | 23.37, 23 | 23.37, 23 |

The blank-override gate and the duration cap change nothing (columns 3 and 4 are identical); what removes the
+0.2 to +0.4 WER rise of the first pass is the start factor 0. (A start bonus of weight x 0.25 = 1.25 can
only flip calls within 1.25 logits, so a margin of 4 never binds.)

### (a) Realistic dictionary: about 5% names in the audio, 95% distractors

FLEURS 60 min (6293 words, 272 name tokens): per dictionary P names that occur in the audio plus distractors,
N = 100 (P = 5) or 400 (P = 20); 5 dictionaries (seeds) per size, totals over the 5 runs (same audio, so the
baseline is counted 5 times). Riksdag (8553 words, 201 name tokens): the 20 speaker terms plus 380
distractors, 5 seeds. "Target tokens" = occurrences in the reference of the dictionary's present names;
"all name tokens" includes collateral change on names not in the dictionary; "distractor insertions" =
distractor tokens in the output beyond the reference.

| Audio, N | Weight | Start factor | Target tokens hit | All name tokens hit | Distractor insertions (base) | Mean WER delta | Changed (5 runs) |
|---|---|---|---|---|---|---|---|
| FLEURS 60, 100 | 3 | 0 | 18/25 to 18/25 | 910 to 910 of 1360 | 0 (0) | +0.01 | 14 |
| FLEURS 60, 100 | 5 | 0 | 18/25 to 20/25 | 910 to 907 | 1 (0) | +0.02 | 38 |
| FLEURS 60, 100 | 5 | 0.25 | 18/25 to 20/25 | 910 to 911 | 1 (0) | +0.12 | 264 |
| FLEURS 60, 400 | 3 | 0 | 81/127 to 89/127 | 910 to 916 | 5 (5) | -0.02 | 58 |
| FLEURS 60, 400 | 5 | 0 | 81/127 to 92/127 | 910 to 915 | 9 (5) | -0.04 | 113 |
| FLEURS 60, 400 | 3 | 0.25 | 81/127 to 89/127 | 910 to 905 | 8 (5) | +0.13 | 198 |
| FLEURS 60, 400 | 5 | 0.25 | 81/127 to 92/127 | 910 to 908 | 9 (5) | +0.12 | 324 |
| Riksdag, 400 | 3 | 0 | 150/190 to 190/190 | 935 to 975 of 1005 | 6 (12) | -0.08 | 70 |
| Riksdag, 400 | 5 | 0 | 150/190 to 190/190 | 935 to 975 | 3 (12) | -0.09 | 114 |
| Riksdag, 400 | 3 | 0.25 | 150/190 to 190/190 | 935 to 975 | 6 (12) | -0.03 | 167 |
| Riksdag, 400 | 5 | 0.25 | 150/190 to 190/190 | 935 to 975 | 6 (12) | +0.03 | 336 |

With a realistic dictionary at the new default the target gain is +8 to +11 of 127 FLEURS target tokens at
N = 400 (about +6 to +9 points) and the WER does not rise (-0.04 to +0.02); at N = 100 the denominator is
only 25 tokens in total (0 to +2 hits). Start factor 0.25 gives the same target gain with about 2.5 to 4 times more
changed words and +0.12 WER. Riksdag speaker names reach 190/190.

### (c) Large dictionary, audio without those names

No-names corpus: 79 distinct FLEURS sentences (909 s, 1647 words, 0 name tokens, no digits), 500-term
dictionary (470 distractors plus 30 FLEURS names that are absent from this audio). FLEURS 60 and Riksdag with
a 481-term distractor-only dictionary. "False ins" = dictionary-term occurrences beyond the reference.

| Audio | Weight | WER (baseline) | False ins (baseline) | Changed, sf 0 | Changed / WER, sf 0.25 |
|---|---|---|---|---|---|
| No-names, 1647 words | 1 | 4.98 (4.98) | 0 (0) | 0 | |
| | 2 | 4.98 | 0 | 1 | |
| | 3 | 5.04 | 0 | 2 | 11 / 5.53 |
| | 5 | 5.16 | 0 | 4 | 16 / 5.83 |
| FLEURS 60, 6293 words | 1 | 7.06 (7.06) | 1 (1) | 0 | |
| | 2 | 7.07 | 1 | 3 | |
| | 3 | 7.07 | 1 | 6 | 36 / 7.26 |
| | 5 | 7.07 | 2 | 17 | 61 / 7.26 |
| Riksdag, 8553 words | 1 | 23.40 (23.40) | 4 (4) | 0 | |
| | 2 | 23.41 | 4 | 1 | |
| | 3 | 23.41 | 4 | 2 | 28 / 23.45 |
| | 5 | 23.41 | 4 | 8 | 61 / 23.55 |

At the new default a 500-term dictionary changes 0 to 4 of 1647 words (at most 0.24%) on audio without its
names and no dictionary term is inserted (the 4 Riksdag baseline hits are common words that are also
surnames); at weight 5 on FLEURS 60 two names recalled by the baseline are lost (180 vs 182 of 272). The old
start factor changed 11 to 16 words there (0.7 to 1.0%, WER +0.55 / +0.85).

### (d) Names across window cuts

Windows are 15 s with 2 s overlap (step 13 s) on Core ML. Partial matches reset at each window start by
design; a name that straddles a cut is decoded whole in the neighbouring window (the overlap is longer than
any name) and the existing token-overlap merge chooses between the two. 60 names (30 that the full-length
baseline recognised, 30 that it missed) cut out of the FLEURS 60 audio in three positions: the name starts
0.3 s before the end of window 1 ("cutA"), starts 0.3 s before the start of window 2 ("cutB"), or sits mid
window. All 60 names plus 100 distractors in the dictionary (oracle for these names).

| Names | Position | No dictionary | Weight 3 | Weight 5 | Weight 5, sf 0.25 |
|---|---|---|---|---|---|
| recognised in full context | cutA | 27/30 | 28/30 | 29/30 | 29/30 |
| | cutB | 29/30 | 28/30 | 28/30 | 29/30 |
| | mid | 30/30 | 30/30 | 30/30 | 30/30 |
| missed in full context | cutA | 5/30 | 7/30 | 7/30 | 9/30 |
| | cutB | 6/30 | 8/30 | 9/30 | 9/30 |
| | mid | 4/30 | 8/30 | 8/30 | 10/30 |

The gain at a cut matches the gain mid-window (+2 to +3 at cuts, +4 mid, counts small, +-3); at sf 0 it is
smaller than the old +11/+10 because start-of-term boosting is off. The overlap merge needed no change. Only
the Core ML windowing was tested; the ONNX host uses a 6 s overlap and the same merge. The Swift
end-of-audio flush now updates the bias state; only the shared state-machine vectors cover it (the decoder
itself needs a Core ML model), the Rust decode loop has a scripted prefix-then-continuation test.

### (e) Dictation-length latency

74 distinct FLEURS clips of 2-8 s (median 6.9 s), one warm host per run, weight 5, sf 0, five repeats of
off / 100 terms / 500 terms in that order (the off run goes first each round, so a small order effect is
possible); first clip of each run excluded. Whole-CLI per-file milliseconds.

| | off | 100 terms | 500 terms |
|---|---|---|---|
| CLI per clip p50 / p95 (ms) | 71.6 / 77.0 | 65.0 / 76.0 | 64.7 / 74.7 |
| Added vs off (paired, median of 5 repeats) p50 / p95 | | -2.7 / +3.3 | -2.8 / +3.1 |
| Host decode_ms p50 / p95 | 21 / 27 | 18 / 23.4 | 18 / 23 |
| First-use trie build (5 fresh hosts) | | 9.0-9.5 ms | 18.8-19.7 ms |

Noise on the same configuration is +-9.7 ms (off vs off, abs p95), so no per-request cost is measurable. The
trie is built once per loaded model and term list and served from the cache afterwards. The ONNX host (CPU,
per-frame top-64 selection over about 8k logits) was not timed.

## Recommendation

Viable as an opt-in per-profile setting (Pianissimo only; never default-on) at **weight 3-5 with start
factor 0 (now the default)**. With a realistic dictionary (5% of terms in the audio) it recovers +8 to +11 of
127 target name tokens (FLEURS) and all Riksdag speaker names with no WER cost, and on audio without those
names it changes at most 4 of 1647 words. Keep the dictionary under about 200 terms in the UI (400 is the
stress case), turn it off automatically when empty, and surface "biasing not applied" (hosts report
`boost_active` / `boost_reason`; the CLI warns). Start factor 0.25 recovers no extra names in these sets and
costs 2.5 to 4 times the changed words, so it is not recommended. Blank gate and duration cap are cheap and
tested but not an accuracy lever.

| Criterion | Status |
|---|---|
| +15 pp on unseen names with WER within +0.3 on FLEURS long-form | Not shown: +6 to +9 pp on present names with realistic dictionaries, WER -0.04 to +0.02; unseen-name audio still to be recorded |
| No dictionary: identical output | By construction; covered by tests and omitted protocol fields |
| Core ML and ONNX hosts, CLI parity | Shared vectors pass in both hosts; ONNX not measured end to end |
| Latency budget (<= 200 terms, ~100 ms) | Met on Core ML: no measurable per-request cost, 9-20 ms one-off trie build; ONNX pending |

Still open: unseen and mispronounced names; ONNX host end to end (Windows on ARM); alias entries versus
boosting for homophone spellings; settings toggle and meeting-view wiring (phase 2); confidence intervals
(single deterministic runs on one corpus; the 5-seed rows vary only the dictionary).
