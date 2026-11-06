# Pianissimo file transcription: where the host-side time goes (SAGA-558)

Measured 2026-11-06 on main (`03f369cd`, includes the streaming decode of #539). Purpose: after
W8A8 made the encoder 22% faster without changing end-to-end time (38.6 ms per window), find what
actually limits Pianissimo file throughput.

**Read this first: the fresh runs below were taken under bad conditions** (battery at 5-12%,
discharging; load average 8-22 from other sessions; thermal pressure 1-2). They are 1.5-3x slower
and far noisier than the quiet-machine runs of 2026-11-04. The per-stage *shares* in the quiet set
are the evidence for the conclusion; the fresh set is only used for the in-flight sweep, as a
consistency check, and is not a clean A/B. A clean re-run on AC power with an idle machine is
needed before quoting absolute numbers (see "Open points").

## Setup

- MacBook, Apple M4 (10 cores: 4 performance + 6 efficiency), 32 GB, macOS 27.2.
- Power: **battery, discharging, 12% falling to 5%**. `pmset -g batt` at start: "Battery Power, 14%".
  Other Claude sessions were compiling and running Core ML work (load average 8-22; `uptime`).
- Model: `pianissimo-sv` (installed, Core ML, compute units `ane`, the default). Host: the one built from
  this commit with `scripts/build-engine-host.sh`.
- File: `~/.cache/sagascript-bench/p2-gpu-lane/media/uu24-60-r1.mp3`, 57.6 MB, 3600.0 s, Swedish
  (Riksdag committee hearing UU24; the file used in the #539 A/B and the P2 benchmarks). 277 windows
  of 15 s with 2 s overlap.
- CLI: `cargo build --release -p sagascript-cli --features dev-overrides` from `src-tauri/`
  (`dev-overrides` only so `SAGASCRIPT_ENGINE_HOST` and the tuning variables are honoured).
- "Streaming off": there is no switch on main (the non-streaming default of `transcribe_file` is only
  used by tests). The stand-in is the installed 1.4.8 CLI (`dd14591`, built before #539) with its
  bundled 1.4.8 host. That is the same comparison as the "A-installed" arm of the #539 A/B, but it also
  differs in host build, so treat it as a reference, not a controlled toggle.
- No code was added: everything below comes from what the CLI already emits. `--json` carries
  `performance.decode_resample_seconds`, `total_seconds`, `model_load_seconds` and, per window,
  `preprocess_ms`, `encode_ms`, `decode_ms` (host-reported) and `round_trip_ms` (client-observed
  submit-to-result). No measurement-only commit exists.

## Method

`run.py OUTDIR CONFIG N FILE` (kept outside the repo, in the session scratchpad) runs
`sagascript transcribe --language sv --model pianissimo-sv --json FILE` with a clean environment,
`SAGASCRIPT_SETTINGS_PATH` pointing at a temp file, one run per config per round, three rounds
interleaved (config order rotated within each round), after one discarded warm-up run. Medians over
the 3 rounds. Window columns are the sum over the 277 windows. Configs differ only by:

| Config | Setting |
|---|---|
| main-stream | defaults (host advertises 4 slots, the client uses 3 for a batch) |
| installed-nostream | `/usr/local/bin/sagascript` 1.4.8, pre-#539 |
| inflight N | `SAGASCRIPT_ENGINE_MAX_IN_FLIGHT=N+1` (batch lanes = slots - 1) |
| deccpu | `SAGASCRIPT_ENGINE_DECODER_UNITS=cpu` |

The same JSON were also re-analysed for the three quiet-machine A/B sets of 2026-11-04 in
`~/.cache/sagascript-bench/p1-streaming/ab-mp3{,-b,-c}` (3 runs per arm each; `ab-mp3-c` is the
quietest: load 0.4, run-to-run spread under 1 s).

## What each stage is, and where it can be seen

| Stage | Source | Notes |
|---|---|---|
| File decode + resample | `decode_resample_seconds` | runs on the producer thread; overlaps inference when streaming |
| Windowing, overlap, queueing (client) | `round_trip_ms` minus the host's three times | see below |
| IPC | same | **audio is not sent over IPC.** The client appends float32 to a private PCM temp file and sends a small `transcribe_window` request (path, offset, length); the reply carries tokens and timings. JSON Lines framing is a few hundred bytes per window |
| Mel preprocessing (CPU) | `preprocess_ms` | host |
| Encoder (ANE) | `encode_ms` | host |
| Decoder + joint, per-token RNN-T loop | `decode_ms` | host; not separable further (one number for the loop) |
| Token merge, result assembly | not reported separately | inside `total_seconds`; negligible, see residual |

## Results

### Quiet machine, 2026-11-04 (`ab-mp3-c`, load 0.4, median of 3, seconds)

| Stage | installed (no streaming) | main (streaming) | Share of host work (main) |
|---|---|---|---|
| Model load | 0.4 | 0.4 | |
| File decode + resample (`decode_resample_seconds`) | 3.8, before inference | 4.8, **hidden under inference** | |
| Mel preprocessing, sum of 277 windows | 0.84 | 0.85 | 2.8% |
| Encoder, sum | 11.41 | 11.43 | 37.1% |
| Decoder + joint loop, sum | 18.76 | 18.56 | 60.2% |
| Host compute, total | 31.0 | 30.84 | 100% |
| Client round trip, sum | 31.05 | 31.03 | |
| Round trip minus host compute (IPC + queue + client) | 0.05 | 0.19 (0.7 ms/window) | |
| **End-to-end `total_seconds`** | **14.31** | **10.50** (10.45, 10.50, 10.58) | |
| Per window | 51.7 ms | 37.9 ms | |

Per-window medians (main): preprocess 3 ms, encode 35 ms, decode 66 ms, round trip 111 ms.

The other two quiet sets (`ab-mp3`, `ab-mp3-b`) give 14.57 / 10.66 s and 17.07 / 14.24 s
(installed / streaming); the first was taken with load 10 and is noisy; the relative picture is the
same (decode sum about 1.6x the encode sum in all three).

### Fresh runs, 2026-11-06, contended machine (median of 3, seconds)

| Config | total | decode+resample | pre | enc | dec | round-trip sum | enc med ms | dec med ms | rt med ms | ms/window |
|---|---|---|---|---|---|---|---|---|---|---|
| main-stream | 16.26 | 7.69 | 1.33 | 14.72 | 31.84 | 48.1 | 47 | 118 | 174 | 58.7 |
| installed-nostream | 23.14 | 3.97 | 1.53 | 14.91 | 40.23 | 57.1 | 49 | 146 | 209 | 83.5 |
| inflight 1 | 29.81 | 6.59 | 0.71 | 10.22 | 18.38 | 29.5 | 37 | 62 | 104 | 107.6 |
| inflight 2 | 20.16 | 8.03 | 1.03 | 11.99 | 26.68 | 39.9 | 40 | 87 | 132 | 72.8 |
| inflight 4 | 18.07 | 9.67 | 2.07 | 19.82 | 47.86 | 71.3 | 61 | 166 | 244 | 65.2 |
| inflight 7 | 16.84 | 12.77 | 2.92 | 44.85 | 63.69 | 115.4 | 169 | 215 | 406 | 60.8 |
| decoder units cpu | 19.34 | 8.39 | 1.63 | 19.20 | 37.92 | 57.3 | 55 | 137 | 202 | 69.8 |

Per-run totals (s): main-stream 14.8 / 22.1 / 16.3; installed 19.8 / 66.3 / 23.1; spread this large
means these rows cannot separate effects below roughly 30%. The streaming-vs-installed gap (16.3 vs
23.1) points the same way as the quiet set (10.5 vs 14.3) but is not a measurement of the gain.

## Where the time goes

1. **Client-side work is not the limiter.** Windowing, queueing, the PCM file and JSON framing cost
   0.19 s in total on a 10.5 s run (0.7 ms per window, 1.8%), by `round_trip_ms` minus the host's
   own three times. File decode + resample takes 4.8 s but runs beside inference; it never starves
   the engine (the inference is already 10 s long, decode 4.8 s ahead of it). Streaming hid it, which
   is the whole 3.8 s of the P1 gain; there is nothing more to hide.
2. **The host is saturated.** The sum of host compute over the windows is 30.84 s; with 3 windows in
   flight that is 10.28 s, against 10.50 s measured: 98% of the end-to-end time is host compute
   divided by the number of lanes. The median encode time with 3 lanes (35 ms) equals the solo
   figure in `pianissimo-ane-eviction.md` (31-41 ms), so the lanes are not visibly queueing behind
   one another inside the encoder call.
3. **Inside the host the decoder loop is the largest stage, not the encoder:** 60% against 37% for
   the encoder and under 3% for preprocessing. The decoder + joint loop takes 66 ms per window
   against 35 ms for the encoder. At about 52 tokens per window that is roughly 1.3 ms per token
   (estimate, from the 277 windows' token counts, not separately timed), which fits two tiny Core ML
   `prediction()` calls per token dominated by dispatch overhead, not arithmetic.
4. **Why W8A8 did not help.** Making the encoder 22% faster removes about 8% of host work
   (0.22 x 37%), and any gain is within the 1-2 s run-to-run spread. This is consistent with the
   measured 38.6 ms per window not moving. (The ratio is arithmetic on the shares above; the W8A8
   run itself is not repeated here.)
5. **Adding lanes has stopped paying.** In the contended sweep, total time falls from 107.6 ms per
   window (1 lane) to 72.8 (2), 58.7 (3, default), 65.2 (4) and 60.8 (7), while the per-window
   latency grows from 104 ms to 406 ms. Beyond 3 lanes the hardware behind the host is the
   limit; the extra lanes only lengthen every window's wait. `SAGASCRIPT_ENGINE_DECODER_UNITS=cpu` did
   not change the decoder time (37.9 s against 31.8 s sum, inside the noise), which suggests the decoder
   loop already runs on the CPU in practice.

### The limiter

Host-side per-window compute, with the per-token decoder loop as its largest part (60%), running at
saturation with the default 3 lanes. Client code, IPC and file decode are not on the critical path.

What is **not** established: whether the saturated resource at 3 lanes is the Neural Engine, the CPU
cores (the process used 54-83 CPU-seconds over a 15-20 s wall in the contended runs, 4-5 cores of 10),
or the Core ML dispatch path. In the quiet set 3 lanes x 35 ms encode / 104 ms window is about 100%
Neural Engine duty, so a saturated ANE explains the saturation just as well as the decoder loop does.
Discriminating needs a clean-power, idle-machine experiment (see below). The conclusion that does
not depend on it: encoder-only work (W8A8, banded attention) addresses 37% of host work, the decoder
loop 60%.

## Recommendation

1. **Next optimisation: shrink the per-token decoder loop** (largest stage, 60%). Candidates, in
   order of effort: (a) fuse `Decoder` and `JointDecisionv3` into one Core ML model so each token
   needs one `prediction()` instead of two, halving dispatch overhead; (b) run the greedy loop on the CPU
   without Core ML (the networks are tiny: LSTM state plus one joint projection), for example with
   Accelerate; (c) cache the decoder step for repeated blank/previous-token states. Expected gain
   (estimate): halving the decode stage takes the host work per window from 111 ms to about 78 ms,
   so about 30% less end-to-end (10.5 s to about 7.4 s on this file), if the Neural Engine is not the
   limiter; if it is, the gain is capped near 0-10% and the order should be reversed.
2. **Before building it, run the discriminating experiment** (about an hour): on AC power and an idle
   machine, log per-window start/end timestamps in the host (measurement-only, env-gated) at 1, 2, 3
   and 4 lanes, and compare with an encoder-only and a decoder-only microbenchmark. If the ANE is idle
   more than 15% of the time at 3 lanes, go for (1a)/(1b); if it is near 100%, only a cheaper
   encoder (or a second accelerator, see P2b) can help.
3. **P2b (GPU lane): park the branch, do not resume yet.** It adds a second engine for the *encoder*,
   which only pays if the ANE is the saturated resource; that is the undecided question above. Resume
   only if experiment 2 shows the ANE near 100% busy, and then with the decoder on the CPU.
4. **P2c (banded attention): drop or keep parked.** It reduces encoder cost, the 37% stage, and
   W8A8's 22% encoder gain already showed that encoder speed alone did not move end-to-end time.
5. Do not spend more on client-side work: file decode, resampling, windowing, the PCM file and
   JSON IPC together are under 2% of the run once decode is overlapped.

## Open points

- Absolute numbers and the streaming-on/off delta need a clean re-run on AC power with an idle
  machine; the fresh runs here were on a 5-12% battery under load average 8-22.
- No switch turns streaming off on main; a proper A/B needs either an env-gated switch (measurement
  only) or a build of `c9e2ce83`. I used the installed 1.4.8 CLI instead.
- The decoder + joint time is one number (`decode_ms`); splitting decoder from joint and counting the
  `prediction()` calls needs host-side instrumentation.
- A WAV input (no mp3 decode) was not run, so the pure mp3 decode cost is only seen through
  `decode_resample_seconds`, which also includes resampling.

## Commands

```bash
git fetch origin && git switch -c claude/saga-558-host-stage-profile origin/main
cp -cR <sibling worktree>/node_modules ./node_modules && npm run build
scripts/build-engine-host.sh
cd src-tauri && cargo build --release -p sagascript-cli --features dev-overrides
# one run (env: SAGASCRIPT_SETTINGS_PATH=<tmp>, SAGASCRIPT_ENGINE_HOST=<built host>)
src-tauri/target/release/sagascript transcribe --language sv --model pianissimo-sv --json \
  ~/.cache/sagascript-bench/p2-gpu-lane/media/uu24-60-r1.mp3
# sweeps: SAGASCRIPT_ENGINE_MAX_IN_FLIGHT=2|3|5|8, SAGASCRIPT_ENGINE_DECODER_UNITS=cpu
# baseline: /usr/local/bin/sagascript (1.4.8, dd14591)
```
