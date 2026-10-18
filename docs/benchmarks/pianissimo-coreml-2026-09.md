# Pianissimo on Core ML: benchmark report (September 2026)

Measured 2026-09-29. All numbers are computed from the raw result files listed under Reproduction; times are medians over 2 reps unless noted.

## 0. Re-check on the published release (2026-09-30)

The 60-minute measurement was repeated on the signed, published **Sagascript 1.4.0** (`af1f5af`, downloaded from the GitHub release) on the same MacBook Air M4, on AC power, thermal pressure level 0, 2 runs each, same file (`fleurs-sv-distinct-60min.wav`: 307 distinct FLEURS sv_se test sentences read by many speakers, joined with 0.5 s silences, 3,611 s, 6,293 reference words; one long file, not a continuous conversation).

| Engine | Time (2 runs) | WER |
| --- | --- | ---: |
| Sagascript 1.4.0 CLI, whole command start to text | 11.79 s, 11.23 s | 7.06% (both runs) |
| Klang MLX 8-bit reference, transcription only (load excluded) | 105.9 s, 160.1 s (second run under thermal pressure level 2) | 6.53% (both runs) |

The 1.4.0 transcript had 6,262 words against 6,293 in the reference (341 substitutions, 67 deletions, 36 insertions); the longest run of dropped words was 2 and of inserted words 3, and there were no repeated 6-grams. The earlier numbers below are from the pre-release CLI in a thermally gated batch: WER matches (7.05% vs 7.06%), and the time was somewhat lower (9.9 s vs 11.2–11.8 s); the re-check is the figure we quote publicly.

## 1. Summary

- Swedish file transcription is 11x to 18x faster than Sagascript 1.3.2, end to end (one CLI invocation, start to text): a 60-minute recording takes 9.9 s instead of 174 s (366x real time, against 21x).
- The new engine runs Klang AI's Swedish Pianissimo model on the Neural Engine through Core ML. It is also 6x to 10x faster than Klang's own MLX 8-bit reference build (GPU), even though the MLX times exclude model loading.
- The speed costs a little accuracy. Over 11,589 reference words, the new CLI has a pooled word error rate (WER) of 6.73%, against 6.37% for 1.3.2 and 6.38% for the MLX reference. That is 42 more wrong words than 1.3.2 (0.36 percentage points, about 6% relative).
- Peak memory drops from 3.3 to 5.3 GiB (1.3.2, one process) to about 0.67 GiB for the CLI process plus about 0.45 GiB for the engine host process (sampled separately on a 60 min file), and the Neural Engine path recorded no thermal pressure on a fanless laptop.
- Separately, a KB-Whisper bug is fixed: 1.3.2 dropped much of the audio in long files (WER 13.9% to 17.0%). Now it is 6.0% to 6.1%. On the same 5 and 15 minute files KB-Whisper large is slightly more accurate than Pianissimo (6.02% vs 6.72%) but 30x to 50x slower.

## 2. Results

"Time" is `transcribe_s`: for the two CLIs, the wall time of one complete CLI invocation (process start, model load from the on-device cache, decode, text out); for FluidAudio and MLX ref, which were run as warm engines, the transcription time excluding their model load (so those rows are, if anything, flattered). "x RT" is audio duration divided by time. WER was identical across reps for every engine/file. Engines:

New CLI = new product CLI; 1.3.2 CLI = NeMo-Speech.cpp CPU with Q8 GGUF; FluidAudio = v0.17.4 with a community Core ML conversion ("markstrom"); MLX ref = Klang's official MLX 8-bit build, 120 s chunks.

### Long-form speed and WER (`final-20260929/longform.jsonl`)

| Audio | Engine | Time (s) | x RT | WER (%) | Errors (S/D/I) |
|---|---|---|---|---|---|
| 5 min (306.1 s, 540 words) | New CLI | 1.27 | 241 | 7.04 | 30/7/1 |
|  | 1.3.2 CLI | 14.37 | 21 | 6.67 | 28/7/1 |
|  | FluidAudio | 1.33 | 230 | 5.93 | 25/5/2 |
|  | MLX ref | 7.72 | 40 | 6.85 | 30/7/0 |
| 15 min (905.6 s, 1587 words) | New CLI | 2.78 | 325 | 6.62 | 80/19/6 |
|  | 1.3.2 CLI | 40.96 | 22 | 6.17 | 75/21/2 |
|  | FluidAudio | 4.30 | 211 | 6.55 | 78/19/7 |
|  | MLX ref | 23.61 | 38 | 6.30 | 78/19/3 |
| 30 min (1807.8 s, 3169 words) | New CLI | 5.17 | 350 | 6.09 | 146/33/14 |
|  | 1.3.2 CLI | 84.67 | 21 | 5.81 | 142/33/9 |
|  | FluidAudio | 7.98 | 227 | 6.34 | 149/33/19 |
|  | MLX ref | 47.86 | 38 | 6.03 | 150/31/10 |
| 60 min (3611.0 s, 6293 words) | New CLI | 9.87 | 366 | 7.05 | 341/67/36 |
|  | 1.3.2 CLI | 174.04 | 21 | 6.67 | 325/63/32 |
|  | FluidAudio | 11.98 | 301 | 7.34 | 349/60/53 |
|  | MLX ref | 96.68 | 37 | 6.53 | 323/59/29 |

Speed-up of the new CLI at 5/15/30/60 min: vs 1.3.2 11.3x, 14.7x, 16.4x, 17.6x; vs MLX ref 6.1x, 8.5x, 9.3x, 9.8x; vs FluidAudio 1.0x, 1.5x, 1.5x, 1.2x.

### Pooled WER over all four files (11,589 reference words)

| Engine | Errors | Pooled WER |
|---|---|---|
| 1.3.2 CLI | 738 | 6.37% |
| MLX ref | 739 | 6.38% |
| New CLI | 780 | 6.73% |
| FluidAudio | 799 | 6.89% |

The host-alone run (engine host via a Python runner) gives identical error counts to the new CLI on all four files.

### Peak memory (max RSS, MiB, `/usr/bin/time`, 5/15/30/60 min)

| Engine | 5 | 15 | 30 | 60 |
|---|---|---|---|---|
| New CLI (CLI process only) | 670 | 668 | 668 | 668 |
| 1.3.2 CLI | 3359 | 4704 | 4903 | 5293 |
| MLX ref | 885 | 921 | 976 | 1087 |
| FluidAudio | 158 | 197 | 253 | 365 |

Caveat: for the new CLI, RSS covers only the measured process, not the engine host child that does the Core ML work. Sampling the host process separately during a 60 min transcription (`ps -o rss`, every 0.2 s) gave a peak of about 457 MiB, so the product total is roughly 1.1 GiB. The host-alone runner reported 1447 to 5966 MiB (growing with length) but is a benchmark arrangement, not the product. Whole-process-tree memory was not measured. The 1.3.2 and MLX figures are single-process.

### Short utterances (10 clips, 4.3 to 10.5 s, 138 reference words)

| Engine | Median decode (s) | Median process wall (s) | WER |
|---|---|---|---|
| 1.3.2 per-utterance spawn (`bakeoff-20260929`) | 0.684 | 0.731 (0.56 to 0.86) | 9/138 = 6.52% |
| Our engine host, warm (`final-20260929`) | 0.059 | 0.731* | 8/138 = 5.80% |
| FluidAudio, warm | 0.057 | 0.556 | 9/138 = 6.52% |

Warm decode is about 11.6x faster than 1.3.2's per-utterance spawn (0.684 s vs 0.059 s). *The host's wall time includes Python runner startup and is not a product number. The new CLI itself was not run on the clips, and 1.3.2's numbers come from an earlier session (same method). Clip 05 has a 58% WER on every engine (7 of 12 words wrong), so the 10-clip WERs differ by at most one word and say little about relative accuracy.

### KB-Whisper before and after (large unless noted; single rep)

| Audio | 1.3.2 time (s) | 1.3.2 WER | Fixed time (s) | Fixed WER |
|---|---|---|---|---|
| 5 min | 55.4 | 16.67% (23/63/4) | 62.2 | 6.11% (24/7/2) |
| 15 min | 181.0 | 16.95% (54/209/6) | 174.4 | 5.99% (72/18/5) |

1.3.2 medium measured 14.7% pooled on the same two files; the fixed build was only measured with large. The cause was decoding with timestamps off: whisper.cpp skipped the rest of a 30 s window after an early end-of-text, visible as 63 and 209 deletions in the old runs vs 7 and 18 now (GitHub issue #236). Speed is unchanged within run-to-run noise. On the same two files Whisper large has 128 errors in 2,127 words (6.02%) vs 143 (6.72%) for the new CLI, at 30x and 50x the time.

### Swedish model lineup (same files, fixed KB-Whisper, new CLI)

| Model | Download | WER 5 min | WER 15 min | Time 5 / 15 min |
|---|---|---|---|---|
| Pianissimo (Core ML) | 608 MB | 7.04% | 6.62% | 1.27 / 2.78 s |
| KB-Whisper Large | 1,031 MB | 6.11% | 5.99% | 62 / 174 s |
| KB-Whisper Medium | 514 MB | 7.96% | 8.57% | 31 / 91 s |
| KB-Whisper Small | 190 MB | 12.04% | 10.21% | 13 / 28 s |
| KB-Whisper Base | 60 MB | 12.41% | 11.91% | 6 / 11 s |
| KB-Whisper Tiny | 40 MB | 64.63% | 46.25% | 7 / 14 s |

Pianissimo and Large come from the AC-power runs above; Tiny to Medium were measured the same way but
on battery power (`results/lineup-20260929.jsonl`, one rep), so their times are indicative while WER
is deterministic. Tiny collapses on long Swedish audio (171 and 303 deleted words). Pianissimo is both
faster and more accurate than Base, Small and Medium; Large is 0.6–1.0 points more accurate but about
60x slower.

## 3. Method

- **Hardware:** MacBook Air 13" M4 (Mac16,12), 4 performance + 6 efficiency cores, 10-core GPU, 16-core Neural Engine, 32 GB, fanless, macOS 27.0, on AC power (battery 97 to 100% in every final-run row).
- **New engine:** KlangAI Pianissimo (Swedish FastConformer-TDT fine-tune of Parakeet TDT 0.6B v3, CC BY 4.0), converted by us to Core ML: fp16 compute on the Neural Engine, int8 per-channel encoder weights, fixed 15 s windows with 2 s overlap, token merge ported from parakeet-mlx. It runs in a persistent Swift sidecar with up to 3 windows in flight. Published at https://huggingface.co/magnusgille/pianissimo-sv-coreml (commit 6e33b64e).
- **Old engine (1.3.2):** NVIDIA NeMo-Speech.cpp v0.1.0 on CPU, Q8 GGUF, one process per file and per dictation utterance.
- **What "end to end" includes:** process start, model load from cache, decode, text out. The model was already compiled on the device (see first-use cost below). The new CLI's own report shows model load about 0.35 s and total 0.86 s (5 min) to 9.3 to 9.6 s (60 min), with `engine_warm: false` in each run, i.e. a fresh sidecar per run.
- **Data:** FLEURS sv_se test (CC BY 4.0). Each file concatenates the first reading of every distinct sentence, with 0.5 s silence between, into about 5/15/30/60 minutes (22/69/145/307 clips). Earlier concatenations repeated sentences and made Whisper look worse than it is. Ten separate clips serve the latency test.
- **Normalisation:** NFC, lowercase, punctuation replaced by spaces, whitespace collapsed (`norm.py`), scored with jiwer. The reference word counts in the results (540, 1587, 3169, 6293) come from this normalisation and differ slightly from the raw counts in the manifest.
- **Thermal gating:** the driver (`bench.py`) waits for macOS thermal pressure 0 before every run, then a 20 s cooldown, and records thermal state during the run. In the final run every row started at level 0 after a 30.1 s wait.
- **Statistics:** 2 reps is a small sample. Wall times of the two reps differ by at most about 5 s (MLX ref 60 min: 101.8 vs 96.5 s).

## 4. Findings and caveats

**Quality cost.** The new CLI is 0.28 to 0.44 percentage points worse than 1.3.2 on every file length (5 min 7.04 vs 6.67; 15 min 6.62 vs 6.17; 30 min 6.09 vs 5.81; 60 min 7.05 vs 6.67) and 0.35 points worse than Klang's MLX reference pooled. That is 7 to 24 more errors on the 15/30/60 min files (2 on the 5 min file), mostly substitutions. The same model on 1.3.2's Q8 path and Klang's MLX build agree closely (738 vs 739 errors), so the loss comes from our Core ML path: fixed 15 s windows with token merging, int8 encoder weights, and fp16 on the Neural Engine. We have not separated these causes. FLEURS is read speech from one corpus; real dictation may differ.

**Window-size experiment (MLX, not Core ML).** With one rep on the 5 and 15 min files (2,127 words), pooled WER by window/overlap was: 15 s/2 s 6.58%, 15 s/4 s 6.72%, 30 s/4 s 6.30%, 30 s/6 s 6.06%, 60 s/10 s 6.21%, 120 s/15 s (Klang default) 6.44%. The largest spread is 14 errors, so this is weak evidence; it hints that longer windows might recover some accuracy, which was not tested on the fixed-15 s Core ML model.

**Seam experiment (negative result).** We tried silence-aware window boundaries on our host (`seams/exp1.jsonl`, one rep, four file lengths, 11,589 words). Fixed windows with 2 s overlap (the shipped configuration) scored 6.73%. The silence-aware variants scored between 6.65% and 7.11%, and the best (`sil-o2-s3`, 771 errors) is 9 words better than shipped, well within noise. None was adopted. Timing in this experiment is not comparable to the rest: it ran on battery (78 to 85%) with thermal pressure elevated in every run.

**Thermal behaviour.** In the final gated run no engine recorded pressure above 0, GPU engines included, so that run does not show throttling. The earlier gated bakeoff (same hardware, `bakeoff-20260929/core.jsonl`) did: on 60 min files the MLX build reached level 2 (elevated for 59% and 23% of samples across two reps) and the Metal NeMo build level 2 (49% and 43%); optimised MLX variants reached level 1 or 0. The Core ML/Neural Engine variants recorded level 0 throughout, in both runs. The 60 min MLX time was 90.3 s in the bakeoff and 96.7 s in the final run, so no clear heat slow-down is visible. We report the recorded levels only.

**First-use compile.** Core ML compiles the model for the Neural Engine the first time it is loaded on a device. All timed runs above used an already compiled model. Measured once end to end (fresh `HOME`, public download of the published artifact, `sagascript engine doctor`): the first load took 19.0 s; later loads take about 0.4 s. Earlier development runs without a working compile cache took 78 to 95 s per load, which the shipped cache design avoids after the first use.

**Not measured.** New CLI on the short clips (dictation uses the warm engine host, measured above); exact whole-process-tree memory; energy use; other hardware; other languages; noisy or conversational speech; more than 2 reps; KB-Whisper medium after the fix.

## 5. Reproduction

The benchmark tooling is not in this repository; it lives in the local bench directory `~/.cache/sagascript-bench/bench/`. Paths below are relative to it.

- `build_distinct.py` builds the long-form files (`../longform/distinct-manifest.json` has durations, word counts and SHA-256); `build_clips.py` builds the 10 clips (`../longform/clips-manifest.json`).
- `norm.py` holds the WER normalisation.
- `bench.py` is the driver: it runs each engine command under `/usr/bin/time`, applies the thermal gate, and appends JSON Lines rows.
- Final results: `results/final-20260929/{longform,clips,whisper}.jsonl`. Earlier baselines: `results/bakeoff-20260929/{whisper,clips,core,window,baseline}.jsonl`. Seam experiment: `results/seams/exp1.jsonl`.
- To recompute: group rows by (`engine`, `file`), take the median of `transcribe_s` over `rep` (not `process_wall_s`, which also includes the runner's separate warm-up invocation for warm engines), check `wer` agrees across reps, and pool `sub+del+ins` over `ref_words`.

## 6. Attribution

- Pianissimo Swedish model: Klang AI AB, licensed CC BY 4.0. The Core ML conversion is a derivative under the same terms.
- Evaluation audio and transcripts: Google FLEURS, sv_se test split, CC BY 4.0.
- FluidAudio v0.17.4 and the mobius Core ML tooling: Apache-2.0. The community Core ML conversion used in the comparison carries its own model-card license. Token merge is ported from parakeet-mlx.
- NeMo-Speech.cpp, Parakeet TDT 0.6B v3 and KB-Whisper are used under their own licenses; check each upstream repository.

## 7. Follow-up (issue #268): local attention and 30/40 s windows

Measured 2026-09-30 to 2026-10-01 on the same MacBook Air M4 (AC power, thermal pressure 0 before every timed run, 1-min load 2.7 to 5.6, median 3.8 during the long-form batches, so slightly above the requested 4 at times). Raw results and tools: `docs/benchmarks/data/issue-268/`. Candidate model packages are local only (nothing published, manifest pin unchanged).

**Was local attention already exported? Yes, but only as a dense masked form.** The checkpoint config has `self_attention_model: rel_pos_local_attn` and `att_context_size: [256, 256]`; `convert.py` asserts both and `components.py` replaces every layer with `DenseLocalAttention`, which computes full T x T scores and adds a -10000 mask where `abs(key - query) > 256`. Consequences: (1) for the shipped 15 s window (188 encoder frames) the band covers the whole window, so the mask is a no-op and r1 is exactly the model's local attention; (2) the export is not banded in compute, but a band of 512 frames cannot save much at 376 or 501 frames (about 10% and 24% of the score matrix is masked), so a cheaper banded formulation would not have helped. The issue's hypothesis (full attention export) is therefore not the cause of the slow 30 s build.

**Actual cause of the slow 30 s build.** Core ML placement of the 30 s encoder (`placement-30s-gather.json`): 95.6% of ops on the ANE, but the 24 per-layer `gather_along_axis` ops (relative-position alignment) and 24 `select` ops run on the CPU (r1 has 4 CPU ops of 1234). Each layer therefore round-trips a 1 x 8 x T x T tensor between ANE and CPU. Replacing the gather with an exact reshape/slice skew (Transformer-XL rel-shift; `--rel-shift skew`, automatic for 256 to 512 encoder frames) gives 1354 ops with 4 on the CPU (99.7% ANE), identical to r1. The skew is exact inside the band (checked numerically against the gather for 376 and 501 frames; long-form WER of the 30 s gather and skew builds is identical on all four files). Interface (float32 I/O, names, shapes) is unchanged, so the host loads the packages with only the window differing.

**Per-clip WER** (FLEURS sv_se test, all 759 clips available locally, 15,390 reference words, Klang normalization, each clip scored separately; Core ML through the engine host with its window/overlap plan; Klang's published 6.5% is per clip):

| Engine | Pooled WER | Mean of per-clip WER | Median |
|---|---|---|---|
| Core ML r1 15 s | 6.56% | 6.87% | 4.35% |
| Klang MLX 8-bit | 6.48% | 6.79% | 4.55% |
| Core ML 30 s local (skew) | 6.39% | 6.77% | 4.76% |
| Core ML 40 s local (skew) | 6.37% | 6.75% | 4.65% |

Klang ONNX was not run per clip. All clips are below 35 s, so the 30/40 s builds see each clip in one window.

**Long-form WER and speed** (host runner, warm host, median of 3 reps, interleaved with r1; `transcribe_s` excludes host start; not the same as the CLI wall times in section 2). Overlap is 2 s for r1 and 6 s for the 30 s build, 8 s for the 40 s build. WER is identical across reps.

| Variant | 5 min WER / s | 15 min WER / s | 30 min WER | 60 min WER / s | 4-file pooled WER | 60 min vs r1 |
|---|---|---|---|---|---|---|
| r1 15 s / 2 s | 7.04% / 0.80 | 6.62% / 2.34 | 6.09% | 7.05% / 9.37 | 6.73% (780/11589) | 1.00x |
| r2 30 s / 6 s, gather (first export) | 5.93% / 3.26 | 5.80% / 9.39 | 5.62% | 6.53% / 37.3 | 6.15% | 3.98x |
| r2 30 s / 6 s, skew | 5.93% / 1.19 | 5.80% / 3.26 | 5.62% | 6.53% / 12.67 | 6.15% (713/11589) | 1.35x |
| r2 40 s / 8 s, gather | 7.22% / 3.60 | 7.50% / 10.65 | 7.26% | 7.66% / 41.7 | 7.51% | 4.45x |
| r2 40 s / 8 s, skew | 7.22% / 1.12 | 7.69% / 3.18 | 7.42% | 7.74% / 12.41 | 7.6% | 1.32x |

For reference, Klang MLX 8-bit pooled is 6.38% (section 2). The 30 min speed was measured once without interleaving and is not tabulated. The 40 s build is worse than r1 in long-form even though it is the best per clip. On the 15 min file the overlap matters a lot: 40 s with 4/8/12 s overlap gives 8.32/7.69/6.68% and 30 s with 4/6/10 s gives 6.55/5.80/6.11% (one rep, `overlap-15min.txt`), so the loss is in window merging, not in the encoder. We did not investigate further.

**Latency, memory, first use** (skew builds; r1 in the last row was re-measured after the skew runs):

| Variant | Short-utterance latency, warm host (median / p90, 5 clips of 2 to 8.5 s) | Host peak RSS, 60 min | First load, empty cache (compile) | Load, warm cache |
|---|---|---|---|---|
| r1 15 s | 51.8 / 56.1 ms (51.3 / 55.6 re-run) | 387 MB | 9.3 s | 0.4 s |
| r2 30 s skew | 100.1 / 103.4 ms | 453 MB | 16.9 s | 0.3 s |
| r2 40 s skew | 132.3 / 135.9 ms | 447 MB | 18.6 s | 0.3 s |
| r2 30 s gather | 256.9 / 262.3 ms | 3647 MB | 258 s (thermal level 2) | 134 s, then 47 s on the next load |
| r2 40 s gather | 402.6 / 407.3 ms | 3509 MB | 327 s (thermal level 2) | 283 s on the next load |

Short clips are padded to the fixed window, so dictation latency scales with the window: the 30 s build roughly doubles r1's 52 ms to 100 ms. This fails the issue's "no latency regression" criterion, although 100 ms remains short in absolute terms. No run of the skew builds recorded thermal pressure above 0.

**Encoder parity** (`validate.py`, 4 consecutive windows from t=0, `CPU_AND_NE`): encoder relative L2 against NeMo fp32 is 0.060 (max 0.088) for 30 s and 0.051 (max 0.065) for 40 s, similar to r1 (0.038, max 0.053; int8 weights dominate the error). Mel error 0. Token agreement of the full Core ML pipeline with NeMo greedy is 0.998 (3 of 4 windows identical) for 30 s and 0.877 for 40 s (0 of 4 identical). For 40 s the same disagreement appears with NeMo's own fp32 encoder output, so the validator's ported decode loop diverges from NeMo's decode on 501-frame windows; this was not resolved. MLX parity was not measured separately; the per-clip WER above is the end-to-end check.

**Verdict against the acceptance criteria.** Per-clip WER reported and compared: done. Long-form WER at or below 6.38% and within 2x of r1 on 60 min with no thermal pressure: met by the 30 s / 6 s skew build (6.15%, 1.35x, level 0); not met by 40 s (7.6% pooled). Short-utterance latency not regressing: not met (52 to 100 ms). Recommendation: ship the 30 s / 6 s skew build as r2 only if a 50 ms dictation regression is acceptable, or ship it with a short-window fallback for dictation (a second 15 s model, which needs a host/client change and was not built). Otherwise keep r1. Unverified: the skew build was not run on other hardware, through the real CLI end to end, or on noisy speech; r2 has not been published, pinned, or installed.
