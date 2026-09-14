# Transcribe polish review

Independent reviewer: **Claude Opus 5** (`claude-opus-5`), requested effort High.
Structured CLI metadata confirms Anthropic first-party model usage. Reviews
received a frozen diff only, with no tools, hooks, MCP, browser or repository
access, in non-persistent plan mode.

## Outcome

- Round 1: request changes. Identified shared abort ownership, cancellation gaps
  across model loading and resampling, a compressed-size timeout that penalized
  long files, unscoped progress, synthetic percentages, and UI consistency issues.
- Round 2: main fixes accepted; two boundary races remained: Stop arriving before
  run registration, and frontend cancellation overriding a backend success.
- Final focused follow-up: **APPROVE**, no High/Medium issue in those fixes and no
  new blocking issue in the supplied diff.

## Implemented corrections

- Plain imports lease a separate warm backend. Live dictation/training cannot be
  aborted by plain Stop. Only one plain run holds the lease at a time.
- Cancellation is sticky, checked between decoder/resampler calls and after
  model loading, and reasserted after warm-state acquisition. Native work is
  joined before releasing the slot; no detached-worker claim of idle.
- Removed the decode timeout based on compressed bytes. Inference retains a
  duration-based deadline. Native model loading itself cannot be interrupted.
- Run-ID-scoped events prevent stale/training events advancing another import.
- Bounded pending-stop intents cover IPC reordering before registration.
- Backend success is authoritative if completion wins a Stop race, including
  any already-dispatched auto-paste; the UI preserves the successful result.
- Native help dialog supports keyboard focus/Escape; Save/Copy feedback is
  generation-guarded; missing-file detection is decoder-specific.

## Deliberate limits

- Re-run uses current model settings plus the saved file-specific profile,
  prompt and diarization choice. It does not promise exact historical engine
  replay after settings changes; #235's original broader wording needs follow-up.
- File imports retain an additional warm runtime and can compete with dictation
  for CPU/GPU resources. No latency or peak-memory improvement is claimed.
- Model loading cancellation waits for the native loader to return; a spinner
  or elapsed timer does not prove native progress.
- Native user acceptance of layout/progress preceded the cancellation hardening.
  Browser checks of focus, run-ID filtering and controls used synthetic Tauri
  responses; they are not native WebKit end-to-end acceptance.
- CLI JSON progress acceptance is documented in `transcription-progress.md`.
  Earlier incomplete Ctrl-C/PTTY experiments are explicitly not accepted as
  evidence of clean cancellation.

Follow-ups suggested by Opus: finer classification of errors arriving during
Stop, further tombstone-expiry/race tests, and idle unload/resource budgeting.
