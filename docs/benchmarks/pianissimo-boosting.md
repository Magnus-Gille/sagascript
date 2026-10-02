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
  8 frames (0.64 s) without a token. Greedy decoding cannot retroactively undo a committed prefix, so the
  rollback penalty of beam-based boosting trees is replaced by the reduced start bonus (the main
  false-positive control, see below). No dictionary: the joint's own argmax is used, output unchanged.
- **Where:** Swift `ContextBiasing.swift` + `TdtDecoder.swift` (Core ML host), Rust `boost.rs` + `tdt.rs`
  (ONNX host, unit-tested; not measured end to end in this phase), protocol fields `boost_terms` (<= 500
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

## Speed

Total CLI time and summed per-window decode time with and without a 68 / 226 term dictionary (weight 5,
three alternating repeats) are inside run-to-run noise (FLEURS 60: off 16.7-22.2 s total, on 18.9-21.2 s;
decode_ms sums are across concurrent windows). The trie is rebuilt per 15 s window from at most 500 terms and
the per-step cost is a handful of dictionary lookups, so the <= 200-term / ~100 ms dictation budget is not
threatened (measured window timings show no slowdown; a dedicated dictation latency run is phase 2).

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

## Recommendation

Not a complete result yet, but viable: ship as an opt-in per-profile setting ("Use dictionary while
transcribing", Pianissimo only), default weight 5 with the 0.25 start factor, automatically off when the
dictionary is empty, with the dictionary capped at 200 terms in the UI. Acceptance mapping for #298:

| Criterion | Status |
|---|---|
| +15 pp on unseen names with WER within +0.3 on FLEURS long-form | WER holds (-0.3 to -0.1 at weight 5); recall gain +6.6 / +9.9 on FLEURS names, +4 on Riksdag; unseen-name set still to be recorded (phase 2) |
| No dictionary: identical output | By construction (no top-k backing / no trie); covered by tests |
| Core ML and ONNX hosts, CLI parity | Both implemented; CLI `--glossary-boost`; ONNX measurement pending |
| Latency budget (<= 200 terms, ~100 ms) | No measurable decode slowdown; dedicated dictation measurement pending |

Next steps: held-out unseen-name audio, ONNX host measurement on Windows on ARM, settings toggle and
meeting-view wiring (phase 2), re-tune weight on the unseen set, optionally pronunciation-variant entries.
