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

# SAGA-565: which resource saturates

Measured 2026-11-07, branch `claude/saga-565-lane-experiment` (main `09dd9807` plus the
measurement-only host trace in commit `d9284566`). Question left open by SAGA-558: at the default 3
batch lanes, is the saturated resource the Neural Engine (encoder), the CPU (per-token decoder +
joint loop), or Core ML dispatch?

## Answer

**The Neural Engine (encoder) saturates; the decoder loop does not.** Encoder predictions are
executed one at a time below the `MLModel` object: with N predictions in flight each takes about
N x the solo time (31 ms), the encoder is busy 92% of the wall time at 3 lanes and 98% at 4, and
loading one encoder instance per lane changes nothing. The decoder/joint loop is CPU work that
scales across lanes (3.8 of 10 cores in use at 3 lanes) and is not what limits throughput. Both
still matter for the time per run, because the run is the larger of an encoder floor and a
lane-latency term (below), and at 3 lanes the two are within about 15% of each other.

## Setup and conditions

- Same machine, file (`uu24-60-r1.mp3`, 277 windows), model (`pianissimo-sv`, compute units `ane`) and
  CLI build as SAGA-558. Host built from `d9284566`. Temp `SAGASCRIPT_SETTINGS_PATH`,
  `SAGASCRIPT_ENGINE_HOST` pointing at the built host. `SAGASCRIPT_ENGINE_STAGE_TRACE=<path>` makes
  the host append one JSON line per event with a monotonic nanosecond timestamp and the worker
  thread id: `received`, `work_start`, `pre_start`, `enc_start`, `enc_predict_start/end`, `dec_start`,
  `dec_end`, `sent`, and per window the count and summed wall time of decoder and joint `prediction()`
  calls. Windows are keyed by `offset_samples`. A lane is a worker thread holding a slot.
- "L<n>" means n batch lanes = `SAGASCRIPT_ENGINE_MAX_IN_FLIGHT` n+1 (the client keeps one slot for
  dictation; default 4 slots = L3). SAGA-558 used the same convention.
- **Power: AC, charging, 86-98% battery for every run in the tables. Thermal: `pmset -g therm`
  reported no thermal, performance or CPU-power warning before any run.**
- **Load average (1 min) before each run: 4.5-5.0 for all runs in the main tables.** The Mac was not
  idle: other sessions kept the load at 5-30 for the whole session (a first sweep taken while it
  reached 30-78 was discarded as unusable, except its round-1 GPU rows, flagged below). The runner
  waited for the 1-minute load to drop below 5.0 before each run; my own previous run keeps it from
  going lower. Treat absolute seconds as "moderately busy machine"; the relative picture and the
  trace ratios are the evidence.
- 3 runs per config, rounds interleaved (config order rotated), after one discarded warm-up run. Medians.
  Per-run totals (s): L1 21.77 / 21.39 / 21.68; L2 13.81 / 12.54 / 13.48; L3 10.71 / 10.08 / 10.24;
  L4 9.41 / 10.00 / 9.74; L3 decoder-cpu 11.64 / 9.80 / 10.19. Spread inside one config is up to about
  0.6 s (1.2 s once), so differences below about 0.6 s are noise (L3 vs L4, L3 vs decoder-cpu).
- `powermetrics` needs superuser (`must be invoked as the superuser`); skipped, no sudo used. CPU use
  below is `getrusage(RUSAGE_CHILDREN)` user+sys of the CLI and host, divided by wall time.

## Results: lane sweep (clean set, medians of 3)

| Config | total s | CPU cores in use | encoder busy (>=1 running) | mean encoder concurrency | max enc. concurrency | time with >=2 encoders | encoder call ms (median) | decoder loop ms (median) | wait received to encoder start ms |
|---|---|---|---|---|---|---|---|---|---|
| L1 | 21.68 | 1.43 | 40.4% | 0.40 | 1 | 0% | 31.1 | 45.4 | 2.0 |
| L2 | 13.48 | 2.87 | 70.8% | 0.75 | 2 | 4.7% | 32.4 | 57.2 | 2.9 |
| L3 (default) | 10.24 | 3.81 | 92.5% | 1.20 | 3 | 25.9% | 40.1 | 63.6 | 3.3 |
| L4 | 9.74 | 4.03 | 97.9% | 1.90 | 4 | 72.7% | 62.4 | 67.7 | 3.8 |
| L3, decoder units cpu | 10.19 | 3.78 | 92.1% | 1.17 | 3 | 23.8% | 37.4 | 65.4 | 3.4 |

Mean decoder-loop concurrency: L1 0.56, L2 1.17, L3 1.67, L4 1.92 (max equals the lane count). Mean
preprocess 1.7-3.2 ms. Queueing: a window waits 2-4 ms from `received` to encoder start, nearly all of
it its own mel preprocessing; the host scheduler start delay is under 0.1 ms. The client never has
more windows outstanding than lanes, so no window waits for a slot; the contention shows up as longer
calls, not as queueing.

### Encoder predictions are serialised

If predictions executed in parallel, the call time would stay at 31 ms as concurrency rises. It does
not: it rises in proportion to how many predictions are running at once.

| Config | mean encoder concurrency while busy | predicted call ms (31.1 x concurrency) | measured call ms |
|---|---|---|---|
| L2 | 1.07 | 33.1 | 32.4 |
| L3 | 1.29 | 40.2 | 40.1 |
| L4 | 1.94 | 60.3 | 62.4 |

Encoder predictions overlap in time (max concurrency 3 and 4 in the trace, 26% / 73% of the time at
two or more), but each overlapped prediction is slowed to its share, so the *throughput* of the
encoder is one window per about 31 ms regardless of lanes. That is 277 x 31.1 ms = **8.6 s, the
encoder floor** for this file. The run takes 10.2 s at L3 (the encoder is busy 92.5% of it) and 9.7 s
at L4 (97.9% busy, 13% above the floor, so the interleaving costs a little). Going from L3 to L4
gains 0.5 s while every encoder call gets 56% slower; that is what a saturated serial resource looks
like.

### The decoder loop is not the saturated resource

- The decoder loop is almost entirely Core ML `prediction()` calls: per window 67 decoder calls
  (272 us each at L1) and 85 joint calls (293 us each), 43.2 ms of the 45.4 ms loop. About 0.3 ms per
  call with tiny networks: dispatch overhead, not arithmetic (confirms the SAGA-558 estimate; it is
  152 calls per window, not the 2 x 52 assumed).
- It scales across lanes: mean concurrency 1.67 at L3 and 1.92 at L4, using 3.8 and 4.0 of 10 cores.
  A CPU-bound loop would stop scaling and saturate the cores; here most cores are free. The per-call
  time does rise with load (272 us at L1, 374 at L3, 406 at L4), which is the 45 to 64 ms growth of
  the loop: shared dispatch/memory/driver contention, not core exhaustion.
- `SAGASCRIPT_ENGINE_DECODER_UNITS=cpu` is indistinguishable from the default (10.19 vs 10.24 s,
  call times 380/421 us vs 374/413 us): the default decoder already runs on the CPU in practice.

### Per-lane latency, and why both terms matter

Lane latency per window at L3 = preprocess 3 + encoder 40 + decoder 64 = 107 ms; with 3 lanes the
lane-limited time is 277 x 107 / 3 = 9.9 s, which is what is measured (10.2 s). The encoder floor
is 8.6 s. The measured time sits just above the larger of the two, because encoder calls from
different lanes wait on each other and the decoder phases of other lanes fill the gaps. Reducing only
one term moves the run only until the other binds.

### Is the serialisation per `MLModel` instance (Core ML) or below it (ANE)? Experiment, not committed

An uncommitted throw-away patch loaded N independent `MLModel` encoder instances and sent windows to
them round-robin (`SAGASCRIPT_EXP_ENCODER_COPIES`, not in the branch). If Core ML serialised calls
per model instance, N instances would restore parallelism.

| Config (same session, interleaved) | total s (3 runs) | median | encoder call ms | mean enc. concurrency |
|---|---|---|---|---|
| L3, one encoder | 10.47 / 13.51 (load spike) / 10.07 | 10.47 | 39.8 | 1.15 |
| L3, 3 encoder instances | 10.41 / 10.57 / 10.58 | 10.57 | 40.1 | 1.18 |
| L4, 4 encoder instances | 10.93 / 11.22 / 9.37 | 10.93 | 63.1 | 1.73 |

No change in encoder call time or total. The serialisation is below the model instance: the Neural
Engine (or its driver queue) executes one encoder program at a time. This cannot separate ANE
hardware from the ANE driver/firmware queue; both are outside anything the host controls.

### Encoder on the GPU (`SAGASCRIPT_ENGINE_COMPUTE_UNITS=gpu`, decoder kept on ane or cpu)

Only the first round is usable (load 7-8 before; the second and third rounds ran at load 10-76):

| Config (round 1) | total s | encoder call ms | mean enc. concurrency | decoder loop ms | CPU cores |
|---|---|---|---|---|---|
| L3, encoder gpu, decoder ane | 64.3 | 238 | 2.1 | 219 | 1.75 |
| L3, encoder gpu, decoder cpu | 65.1 | 247 | 2.0 | 216 | 1.88 |

The GPU encoder is serial too (call time about 2.1 x 113 ms), and 113 ms per window is 3.6 times the
Neural Engine's 31 ms. The 6x slower totals also include the decoder loop getting 3.4x slower (about
220 ms, with only 1.8 cores in use: the CPU side waits on the busy GPU/driver). Moving the encoder
entirely to the GPU does not change the saturation point, it only moves it to a much slower resource.
The other two rounds were taken under heavy foreign load (64.8-95 s) and are not used.

## Recommendation

1. **Do both of the following; neither alone gives more than about 10-12%.**
   - **(a) Cut the per-token Core ML call count** (fuse Decoder + JointDecisionv3 into one model, or run
     the greedy loop on the CPU outside Core ML, as SAGA-558 proposed). It halves the loop at best, from
     64 to about 32 ms per window at 3 lanes. Lane-limited time falls to 277 x (3 + 40 + 32) / 3 = 6.9 s,
     but the encoder floor (8.6 s at 31 ms, in practice 8.7-9.0 s) now binds: **estimated 10.2 s to
     about 9.0 s, a 10-12% gain**, not the 30% SAGA-558 estimated when the decoder was assumed to be the
     only limiter. It may also lower contention, so the encoder call time at 3 lanes could fall back
     toward 31-35 ms.
   - **(b) Reduce ANE time per window** (cheaper encoder: the W8A8 build, banded attention, a shorter
     padded input). This lowers the hard floor. With (a) in place, a 22% cheaper encoder gives
     277 x 24 ms = 6.7 s floor and a lane-limited 277 x (3 + 24 + 32) / 3 = 5.4 s: **estimated about
     7 s in total, about 30% faster than now**. Without (a), a cheaper encoder alone gives lane-limited
     277 x (3 + 31 + 64) / 3 = 9.0 s, about 10%. This corrects the SAGA-558 suggestion that encoder
     work is not worth doing: on its own it is not, combined with (a) it is what produces the 30%.
   - If only one can be done, start with (a): it is a host-only change, it also helps dictation
     latency, and its gain does not depend on the encoder model.
2. **Do not resume the GPU lane (P2b) for the encoder.** A GPU encoder is 3.6x slower per window than
   the Neural Engine. As a *second* lane beside the ANE it could at best take about 21% of the windows
   (31 / (31 + 113)), lowering the encoder floor from 8.6 to about 6.8 s in an ideal split, but the
   all-GPU run also slowed the CPU-side decoder loop 3.4x (shared dispatch/driver path), so the real
   gain is probably smaller and the complexity is high. Revisit only after (a) and (b), if the ANE is
   still the binding resource.
3. **More lanes: no.** L4 gains 0.5 s (about 5%, near the noise of this set) and makes every window
   slower; the default of 3 stays.
4. Before building (a) or (b), an encoder-only and a decoder-only microbenchmark would confirm the solo
   31 ms and 45 ms and size the gains better than this two-term model.

## What limits confidence

- The machine was not idle (load 4.5-5.0 before each run, 5-30 around them, and a discarded earlier
  sweep at 30-78). Run-to-run spread is up to 0.6-1.2 s; the conclusions rest on ratios inside each run
  (encoder busy, call time versus concurrency, call counts), which are stable across the three runs.
- "ANE" means "below `MLModel`": the experiment excludes a per-instance Core ML lock but cannot tell the
  Neural Engine from the ANE driver. `powermetrics` (ANE power) was not available without sudo.
- The tracing costs a few microseconds per event under one lock; traced L3 runs (10.1-10.7 s) match the
  untraced SAGA-558 quiet figure (10.5 s), so the effect is small, but an untraced A/B was not run.
- The GPU rows are single runs taken at load 7-8; the other two GPU rounds were disturbed.
- The recommended gains are estimates from a two-term model (encoder floor, lane latency), not
  measurements of the optimised builds.

## Commands

```bash
# host with trace: SAGASCRIPT_ENGINE_STAGE_TRACE=<file> (1 writes to stderr)
SAGASCRIPT_ENGINE_HOST=src-tauri/engine-host/coreml/.build/out/Products/Release/sagascript-engine-host \
SAGASCRIPT_SETTINGS_PATH=<tmp> SAGASCRIPT_ENGINE_STAGE_TRACE=<file> \
SAGASCRIPT_ENGINE_MAX_IN_FLIGHT=4 \
  src-tauri/target/release/sagascript transcribe --language sv --model pianissimo-sv --json <file>
# configs: MAX_IN_FLIGHT=2..5; +SAGASCRIPT_ENGINE_DECODER_UNITS=cpu;
#          +SAGASCRIPT_ENGINE_COMPUTE_UNITS=gpu (+SAGASCRIPT_ENGINE_DECODER_UNITS=ane|cpu)
```
