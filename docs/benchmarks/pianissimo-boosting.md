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
  token that starts a term gets `+weight x 0.25`; the argmax is taken over the adjusted logits. Several
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

## Results (start factor 0.25, the shipped default of the prototype)

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

## Review follow-up (2026-10-02, second pass)

Independent review of the first pass (`docs/benchmarks/data/pianissimo-boosting-20261002/review-followup/`
holds the scripts, `report3.out`, `boundary.out`, `latency.out`; dictionaries are regenerated from
`mk.py` seeds). Same Core ML host, ANE, audio files only. Changes under test: top-64 candidates and scalar
lengths in both hosts, blank-override gating (a start-of-term bonus may override blank only when its raw logit
is within 4.0 of blank) and a 1-frame skip cap for a boosted token that replaced blank, trie cache. Start
factor 0.25 unless a row says `sf0` (start factor 0: only continuations of a prefix the model itself emitted
are boosted). Dictionaries use Swedish surnames and place names that do not occur in any reference
(481 after filtering, public names only). WER is the README normalisation; "changed" is the number of words
that differ from the same audio without a dictionary.

### Ablation: unrelated dictionary (fix 2)

The 47-term unrelated dictionary of the first pass, weight 5. "old" = blank-override gating off (margin 1000)
and blank duration kept (cap 4), i.e. the first-pass behaviour.

| Audio (baseline WER) | old | new default (margin 4, cap 1) | margin 0.5 | sf0 |
|---|---|---|---|---|
| FLEURS 15 (6.62) | 6.99, 12 changed | 6.99 | 7.06 | 6.68 |
| FLEURS 60 (7.06) | 7.23, 48 changed | 7.23, 48 changed | 7.25, 46 | 7.07, 4 |
| Riksdag (23.40) | 23.37, 24 changed | 23.37, 24 changed | 23.36, 21 | 23.40, 2 |

**The gate and the duration cap do not remove the cost.** Output is identical with and without them at
margin 4 (a 1.25 start bonus can only flip calls that are already within 1.25 logits, so a margin of 4 never
binds) and a tighter margin of 0.5 or 0 does not help (7.06 on FLEURS 15 at margin 0). A 1-frame cap versus
4 is also identical: boosted overrides of blank do not happen on frames where the duration head asks for a
long skip. What produces the +0.2 to +0.4 is the start-of-term bonus itself: a handful of low-confidence words
are pulled toward a dictionary prefix ("örter" to "öster", "sjönas" to "tusenskjönas") and a few neighbouring
words change (digits versus words). With start factor 0 the same dictionary changes 4 of 6293 words.
The unit test `a_boosted_token_that_overrides_blank_does_not_inherit_the_blanks_long_skip` and the shared
vectors `blank_margin_*` cover the mechanism; they do not fix the accuracy cost.

### (a) Realistic dictionary: about 5% names in the audio, 95% distractors

FLEURS 60 min (6293 words, 272 name tokens): per dictionary, P names that occur in the audio plus distractors,
N = 100 (P = 5) or 400 (P = 20); 5 dictionaries (seeds) per size, totals over the 5 runs (same audio, so the
baseline is counted 5 times). Riksdag (8553 words, 201 name tokens): the 20 speaker terms plus 380 distractors,
5 seeds. "Target tokens" = occurrences in the reference of the dictionary's present names. "Other names" =
all name tokens in the reference, so it includes collateral change on names not in the dictionary.
"Distractor insertions" = occurrences of distractor tokens in the output beyond the reference.

| Audio, N | Weight | Target tokens hit (base to boosted) | All name tokens hit (base to boosted, of) | Distractor insertions (base to boosted) | Mean WER delta | Changed words (5 runs) |
|---|---|---|---|---|---|---|
| FLEURS 60, 100 | 3 | 18/25 to 17/25 | 910 to 911 of 1360 | 0 to 0 | +0.10 | 185 |
| FLEURS 60, 100 | 5 | 18/25 to 18/25 | 910 to 914 | 0 to 3 | +0.12 | 325 |
| FLEURS 60, 100 | 5 sf0 | 18/25 to 20/25 | 910 to 902 | 0 to 1 | +0.03 | 46 |
| FLEURS 60, 400 | 3 | 81/127 to 92/127 | 910 to 925 | 5 to 9 | +0.07 | 269 |
| FLEURS 60, 400 | 5 | 81/127 to 90/127 | 910 to 915 | 5 to 21 | +0.18 | 463 |
| FLEURS 60, 400 | 5 sf0 | 81/127 to 91/127 | 910 to 907 | 5 to 12 | +0.02 | 154 |
| Riksdag, 400 | 3 | 150/190 to 190/190 | 935 to 975 of 1005 | 12 to 6 | -0.04 | 222 |
| Riksdag, 400 | 5 | 150/190 to 185/190 | 935 to 970 | 12 to 3 | +0.04 | 410 |
| Riksdag, 400 | 5 sf0 | 150/190 to 185/190 | 935 to 970 | 12 to 3 | -0.08 | 144 |

With the 5%-present dictionaries the target-name gain is real but smaller than the oracle numbers:
+9 to +11 of 127 target tokens at N = 400 (about +7 to +9 points), nothing measurable at N = 100 (25 target
tokens in total, 5 runs, differences of 1 to 2 tokens). The start bonus adds distractor insertions and
0.1 to 0.2 WER points; start factor 0 keeps the target gain and almost all of the stability, but loses
a few names that the baseline got right (collateral -3 of 1360 at N = 400). Riksdag speaker names are close
to ceiling (150 to 190 of 190 at weight 3).

### (c) Large dictionary, audio without those names

No-names corpus: 79 distinct FLEURS sentences (909 s, 1647 words, 0 name tokens, no digits). Dictionary of
500 terms (470 distractors plus 30 FLEURS names that are absent from this audio). FLEURS 60 and Riksdag with
a 481-term distractor-only dictionary. "False ins" = dictionary-term occurrences beyond the reference;
"changed" = words that differ from the no-dictionary output (of the ref words).

| Audio | Weight | WER (baseline) | False ins (baseline) | Changed words |
|---|---|---|---|---|
| No-names, 1647 words | 1 | 5.16 (4.98) | 0 (0) | 3 |
| | 2 | 5.34 | 0 | 7 |
| | 3 | 5.53 | 0 | 12 |
| | 5 | 5.83 | 0 | 17 |
| | 3 sf0 / 5 sf0 | 5.04 / 5.16 | 0 / 0 | 2 / 4 |
| FLEURS 60, 6293 words | 1 | 7.10 (7.06) | 1 (1) | 8 |
| | 2 | 7.09 | 1 | 27 |
| | 3 | 7.17 | 2 | 50 |
| | 5 | 7.26 | 5 | 85 |
| | 3 sf0 / 5 sf0 | 7.12 / 7.12 | 1 / 3 | 11 / 24 |
| Riksdag, 8553 words | 1 | 23.42 (23.40) | 4 (4) | 12 |
| | 2 | 23.42 | 4 | 23 |
| | 3 | 23.44 | 4 | 32 |
| | 5 | 23.51 | 4 | 70 |
| | 3 sf0 / 5 sf0 | 23.41 / 23.41 | 4 / 4 | 2 / 8 |

False insertions of dictionary terms stay at or near zero, but the dictionary still changes words: 12 of
1647 (0.7%) at weight 3 and 17 (1.0%) at weight 5 on the no-names audio, WER +0.55 and +0.85 points there.
The changed words are rare or out-of-vocabulary words pulled toward a prefix of a dictionary term, not
inserted terms, so a term-insertion count alone understates the cost (the first pass reported only that
count). With start factor 0 the damage drops to 2-4 words (+0.06 / +0.18). The four baseline false
insertions on Riksdag are common words that are also surnames.

### (d) Names across window cuts

Windows are 15 s with 2 s overlap (step 13 s) on Core ML. Partial matches reset at each window start by
design; a name that straddles a cut is decoded whole in the neighbouring window (the overlap is longer than
any name) and the existing token-overlap merge chooses between the two. 60 names (30 that the full-length
baseline recognised, 30 that it missed) cut out of the FLEURS 60 audio in three positions: the name starts
0.3 s before the end of window 1 ("cutA"), starts 0.3 s before the start of window 2 ("cutB"), or sits mid
window ("mid"). All 60 names plus 100 distractors in the dictionary (oracle for these names).

| Names | Position | No dictionary | Weight 3 | Weight 5 | Weight 5 sf0 |
|---|---|---|---|---|---|
| recognised in full context | cutA | 27/30 | 28/30 | 29/30 | 29/30 |
| | cutB | 29/30 | 28/30 | 29/30 | 28/30 |
| | mid | 30/30 | 30/30 | 30/30 | 30/30 |
| missed in full context | cutA | 5/30 | 11/30 | 16/30 | 7/30 |
| | cutB | 6/30 | 9/30 | 11/30 | 9/30 |
| | mid | 4/30 | 11/30 | 14/30 | 7/30 |

The gain at a cut matches the gain mid-window (no boundary penalty is visible; counts are small, +-3). The
overlap merge did not need changes. Limitation: only the Core ML windowing (2 s overlap) was tested; the
ONNX host uses a 6 s overlap and the same merge.

### (e) Dictation-length latency

74 distinct FLEURS clips of 2-8 s (median 6.9 s), one warm host per run, weight 5, five alternating repeats
of off / 100 terms / 500 terms; the first clip of each run is excluded from the statistics. Numbers are
whole-CLI per-file milliseconds (WAV decode and resample, IPC, encoder, decoder, merge) and the host's own
decode time.

| | off | 100 terms | 500 terms |
|---|---|---|---|
| CLI per clip p50 / p95 (ms) | 67.2 / 79.2 | 66.0 / 77.4 | 65.9 / 77.3 |
| Added vs off (paired, per-clip median of 5 repeats), p50 / p95 | | -1.5 / +1.4 | -1.5 / +1.2 |
| Host decode_ms p50 / p95 | 22 / 28 | 18 / 25 | 19 / 24 |
| First-use trie build (5 fresh hosts) | | 9.0-10.0 ms | 19.2-20.6 ms |

Run-to-run noise on the same configuration is +-4.5 ms at p95 (off versus off), so the per-request added
latency is not measurable (-1.5 ms p50, +1.4 ms p95). The trie is built once per loaded model and term list
(9-10 ms for 100 terms, 19-21 ms for 500) and served from the cache for every later window and dictation, so
the 500-term first dictation pays about 20 ms once. The ONNX host (CPU, per-frame top-64 selection over about
8k logits) was not timed.

## Recommendation

Still viable as an opt-in per-profile setting (Pianissimo only; never default-on), now with the cost
side measured: a realistic dictionary (5% of the terms occur in the audio, 95% do not) gains about +7 to +9
points of target-name recall and +4 to +16 distractor insertions per 5 x 6293 words, and every dictionary
changes 0.4% to 0.8% of the words at weight 3 (up to 1.4% at weight 5) of audio that has none of its names
(WER +0.04 to +0.55 at weight 3, up to +0.85 at weight 5). The
oracle-dictionary numbers above overstate real use. Concretely:

- Default weight **3**, not 5; keep the dictionary under about 200 terms in the UI (the 400-term runs are the
  stress case); automatically off with an empty dictionary.
- Start factor: 0.25 recovers the most names on an oracle dictionary, **0** (continuations only) gives nearly
  the same gain with a realistic dictionary and 3 to 9 times fewer changed words. Prefer 0 as the shipped
  default until unseen-name audio says otherwise; the env override `SAGASCRIPT_BOOST_START_FACTOR` and
  the vectors make the choice a one-line change.
- Blank-override gate and duration cap stay (cheap, tested, no downside measured) but are not an accuracy lever.
- Add a visible "biasing not applied" state: hosts report `boost_active`; the CLI warns when a host ignores
  the dictionary, the app must do the same.

Acceptance mapping for #298:

| Criterion | Status |
|---|---|
| +15 pp on unseen names with WER within +0.3 on FLEURS long-form | Not met on realistic dictionaries: +7 to +9 pp on present names, WER +0.02 to +0.18 (sf0 / sf 0.25, w5); unseen-name audio still to be recorded |
| No dictionary: identical output | By construction (no top-k backing / no trie); covered by tests and by `hello`/result fields being omitted |
| Core ML and ONNX hosts, CLI parity | Shared vectors pass in both hosts; ONNX not measured end to end |
| Latency budget (<= 200 terms, ~100 ms) | Met on Core ML: no measurable per-request cost, 9-20 ms one-off trie build; ONNX pending |

Still open: held-out unseen and mispronounced names; ONNX host measured end to end on Windows on ARM; alias
entries versus boosting for homophone spellings; settings toggle and meeting-view wiring (phase 2);
confidence intervals (all cells are single deterministic runs on one corpus; the 5-seed rows vary only the
dictionary).
