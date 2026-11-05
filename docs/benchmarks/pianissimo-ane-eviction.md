# Pianissimo and Neural Engine eviction (#397): measurements, 2026-11-04

Issue #397: in about one dictation in ten, push-to-talk stalled for 11-17 s because
`ANECompilerService` recompiled the Pianissimo encoder at the first `prediction()` after macOS had
evicted its Neural Engine program. On 2026-11-04 a 64 s dictation failed after the 45 s window
timeout, most likely the same recompile on a loaded machine (hotfix #541 now restarts the host and
retries). There is no public Core ML API to pin a program, so the fix hides the recompile instead:
the `warm` op (`docs/engine-host-protocol.md`) runs one canary prediction when push-to-talk starts,
and the Core ML host runs canaries of its own after memory pressure and after wake.

This page records what was measured while building that fix, and what could not be.

## Setup

- MacBook with 32 GB RAM, macOS 27.2 (26B123). The machine was heavily loaded throughout: about
  19.9 of 20 GB swap in use, other sessions compiling Core ML models in parallel.
- Model `pianissimo-sv-coreml-r1`, compute units `ane`.
- Hosts: the installed 1.4.7 host, and the host from this branch (`swift build -c release`). Each
  ran under its own executable name, so it had its own Core ML caches
  (`~/Library/Caches/<executable>/com.apple.e5rt.e5bundlecache`).
- Driver: a small protocol-v1 script that times `load`, `transcribe_window` and `warm`, and reads
  the host's `proc_pid_rusage` (CPU time, CPU energy `ri_energy_nj`, Neural Engine footprint
  `ri_neural_footprint`). Windows are 15 s of silence, `priority: interactive`.

## Results

| What | Measured |
|---|---|
| `load` with an empty e5 cache (first ANE compile) | 10.0-10.5 s; the first window after it: 39 ms encode |
| `load` with the cache present | 0.46-0.92 s |
| Resident encoder, 15 s window | 31-41 ms encode |
| Host Neural Engine footprint after a prediction | 628 MB, released 10.4-10.7 s after the last prediction (4 of 4) |
| First window after that release (no canary) | 48-80 ms encode |
| First window after 5 min idle (no canary) | 117-223 ms encode |
| Same, with a canary 3 s earlier (key-down emulated) | window 32-36 ms encode; the canary took 65-263 ms |
| Canary on the new host, warm model | 65-82 ms wall time |
| Canary cost | 17.5 ms CPU and 21.7 mJ CPU energy per canary (20 canaries, idle baseline subtracted) |

Findings:

1. **A cold compile happens in `load`, not in the first prediction.** With an empty program cache
   the 10 s compile lands in `MLModel(contentsOf:)`, and the first window afterwards is as fast as
   any. So a canary right after `load` (#397's first proposal) would buy nothing and was not added.
2. **Losing residency is routine and cheap; a recompile is rare and expensive.** macOS releases the
   host's 628 MB of Neural Engine memory about 10.5 s after the last prediction, every time. The
   next prediction maps the program back in for 20-200 ms, more the longer the host was idle (its
   memory is swapped out on this machine). The 11-17 s stall needs more than that: the compiled
   program itself is gone and is rebuilt.
3. **The key-down canary already pays on the cheap path.** After 5 min idle the first window went
   from 117-223 ms to 32-36 ms encode when a canary had run 3 s earlier, which the canary absorbed
   instead (65-263 ms) while the user would still be speaking.
4. **A canary never delayed a window.** In every probe the window sent 3 s or 8 s after a canary,
   and in the fake-host test one sent while a 1.5 s canary was still running, was answered as fast
   as without it (`a_running_canary_does_not_delay_the_dictation`; the Swift and ONNX host tests
   check that a running canary takes no window slot).

### What could not be measured: a real eviction

The recompile could not be induced without root:

- Deleting the host's e5 program cache after its Neural Engine footprint was released: the next
  window took 104 and 131 ms and recompiled nothing (the cache was not even recreated).
- Three foreign Neural Engine compiles in other processes while the host idled: no eviction.
- The compiled programs that matter are kept by `aned` in a system location; purging them, and
  `memory_pressure -S` (simulated pressure), need root. Real memory pressure was not applied: the
  machine was already swapping hard and other work ran on it.
- The unified log shows 301 Neural Engine compiles on this machine in two days, 185 of them longer
  than 3 s and the longest 181.5 s, but `ANECompilerService` does not log which process asked, so
  they cannot be tied to Sagascript after the fact.

Two soak runs probed a loaded, idle host at growing gaps (1-20 min), alternating a window without a
canary and one 3 s after a canary, to catch a natural eviction (the installed host for 17 min, the
new host with the real `warm` op for up to 3 h). Neither saw one while this page was written. A real
memory-pressure event during the second run did fire the host's own canary
(`warm canary trigger=memory_pressure ms=105 probable_recompile=false`), so that wiring works on a
live system.

Without an observed eviction it is still unverified that a window sent during a canary's recompile
waits for that same compile rather than starting a second one; the protocol document marks this as
expected, not observed. The way to catch an eviction on a real machine is the CLI:

```bash
sagascript engine warm --seconds 10800 --canary-every 300
```

A canary line with `(probable Neural Engine recompile)` is an eviction caught in the act; the app
logs the same at INFO (`Pianissimo canary absorbed a probable Neural Engine recompile`, and the
host's `warm canary ... probable_recompile=true`).

### Keep-alive: off by default

`SAGASCRIPT_ENGINE_KEEPALIVE_S` adds a periodic canary in the Core ML host. Its power cost per canary
is small (17.5 ms CPU, 21.7 mJ CPU energy; Neural Engine energy is not in `rusage`), but it is not
clearly worth anything:

- It cannot keep the program resident: that would take a canary every 10 s or less, about 360 an
  hour (some 8 J of CPU energy per hour alone), and each would map 628 MB of Neural Engine memory
  back in that macOS had just reclaimed, on a machine that is already swapping.
- Whether a slower one (every 1-5 min) keeps the compiled program from being evicted is unknown,
  for want of a reproducible eviction.

So it stays off by default and exists for measurement (minimum 60 s).

## Memory pressure and the app

The app unloads an idle Pianissimo model on a memory-pressure warning (#277), so the host's
pressure canary (10 s after a warning, 5 s after the return to normal) mostly finds nothing loaded.
It matters when the model stayed loaded because a dictation or file job was running at that moment.
Since #397, the 5 min cooldown after such an unload no longer holds back the key-down warm: the
dictation that follows loads the model anyway, so loading at key-down moves that load in front of
the user's speech. A wake within the cooldown still does not reload.
