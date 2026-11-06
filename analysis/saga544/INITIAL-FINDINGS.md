# SAGA-544 initial evidence

## Frozen evaluation split

Development IDs: 14347918279741910315, 10411502267097546866, 9092741274593827020,
9191614290588137327, 682426172675355782, 13767655196060956559,
14782532088492245120, 13930291059846386070, 4748525319668878312,
3922683486251864943.

Holdout IDs: 1657, 1616, 1646, 1645, 1635, 1516, 1636, 1637, 1624, 1549.

The split and audio identities are frozen in analysis/saga544/frozen-split.json.
Development is the ten existing public clips represented in the per-clip host sweep.
Holdout is ten unseen FLEURS-SV development sentence IDs, selected by deterministic
SHA-256 ordering from the local public archive. The two new FLEURS rows already present
in the 180s fixture (1604, 1525) are excluded because the aggregate transcript was viewed.
The holdout WAVs are extracted under audio/dev/. The scorer does not read them.

## What the supplied transcripts show

analysis/saga544/score_existing.ps1 assigns merged result words to a fixture item by each
word segment's start timestamp, and uses Unicode-aware word normalization. It reports
development IDs only.

| Run | Window | Development edits / 219 words | Repeats |
| --- | ---: | ---: | ---: |
| CPU, separate utterances (cpu-current-paired.json) | 30 s max | 15 | 3; one normalized transcript per clip |
| CPU, long CLI result (cpu-long.stdout.jsonl) | 30 s | 19 | 1 |
| NPU, separate utterances (shift-native-full.json) | 20 s | 13 | 3; one normalized transcript per clip |
| NPU, long CLI result (npu-long.stdout.jsonl) | 20 s | 16 | 1 |

The CPU and NPU comparisons are descriptive, not proof of a merge regression:
the CPU per-clip and matrix CLI use different binaries. The NPU per-clip host and
matrix runtime host executables have identical SHA-256
a22868e323fe5f81434a2d1b0a9f54e31b465f6bc64d371ccf4f91fd85f1b080, the same Git SHA,
and both load 20 s windows; the NPU encoder file identity used by the two runs still
needs verification before attributing the difference to the merge path.

The clearest recurring item is 10411502267097546866: its NPU per-clip output has
4 reference edits, while the long NPU result assigned to that interval has 8.
The direct host output uses the spoken ordinals “trettonde” and “tjugofjärde”;
the long output uses 13:e and 24:e and changes the spellings of the Polish and
South Korean names. CPU shows the same direction (4 to 9). This looks like an
acoustic/context change around overlapping windows. A postprocessor that rewrites
these names or ordinals would encode this fixture rather than a general Swedish rule.

The long output is not uniformly worse: the NPU 14782532088492245120 clip stays
at 0 edits, and 13767655196060956559 improves from 3 to 2. Other errors such as
mangrovetesk for mangroveträsk, egypter for egyptier, and byzantiska for bysantinska
are already present in the separate-clip results. These do not establish safe general
normalization fixes.

chunk_merge.rs documents exact parity with the upstream window-merging reference
and already has golden fixtures. No merge behavior defect is isolated from the
available final transcripts, and there is no demonstrated text-quality improvement
to justify changing product code or adding reference-specific corrections.

## Next measured experiment

1. Confirm that the per-clip NPU and long NPU runs use the same encoder ONNX SHA-256.
2. Run the exact same host, model, encoder, language, and decoder settings on the ten
   development utterances separately and in a single concatenated fixture. Capture
   raw per-window token outputs and timing metadata.
3. Replay the captured windows through the current merge and any candidate merge
   offline. Keep model outputs fixed so this isolates merge selection from acoustic
   context. For CPU/GPU, optionally compare 20 s vs 30 s windows at the same 6 s
   overlap; for NPU, the encoder has a fixed 20 s input, so compare supported overlap
   values only.
4. Select any candidate on development IDs, then evaluate it once on the frozen ten
   holdout IDs. Retain the ticket if no candidate improves held-out word errors.

No inference, decoder tuning, or product code change was run in this worktree.
