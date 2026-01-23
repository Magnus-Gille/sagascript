# Performance plan (latency-first)

## Latency budgets (targets)

These are “feel fast” targets; measure and revise.

- Hotkey press -> mic capture started: **< 100ms**
- Hotkey press -> UI indicator visible: **< 50ms**
- Stop recording -> final transcript ready: **< 1–2s** for short snippets (local small model)
- Stop recording -> paste complete: **< 2s** typical

## Engineering tactics

### Reduce cold-start cost
- Preload model on app launch (or lazy-load with progress indicator)
- Warm up inference once (tiny silent buffer) to avoid first-run stalls

### Keep audio pipeline simple
- Fixed sample rate, single channel, consistent buffers
- Minimal copying; use ring buffer if streaming

### Work off main thread
- Audio capture callback must be lightweight
- Transcription runs in background tasks
- UI updates via main actor only

### Measure everything
Create:
- `docs/BENCHMARKS.md` for human-readable results
- A small benchmark harness in code:
  - record timestamps for key milestones
  - output to logs in debug mode

## Profiling plan (on real Mac hardware)

- Instruments:
  - Time Profiler
  - Allocations
  - Energy Log
- Scenarios:
  - rapid short snippets
  - long dictation
  - Swedish vs English
  - background + app switching

## Model tuning knobs (Whisper-style)

- Model size: tiny/base/small/medium/large
- Beam size: accuracy vs speed
- Language hinting (avoid auto-detect if it adds noticeable delay)

## UX latency tricks

- Show “Listening…” instantly.
- Provide partial transcript in overlay (optional).
- Paste only final text (avoid flicker in target app).

