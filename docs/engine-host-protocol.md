# Engine-host protocol v1

Sagascript runs local speech engines (first: Pianissimo on CoreML/ANE) in a separate
`sagascript-engine-host` process. The Rust side (app and CLI) owns audio decoding, long-audio
chunking, token merging, progress, cancellation and scheduling; the host only loads a model and
transcribes **one window** of audio at a time. This keeps every engine small, lets dictation run
between the windows of a long file job, and gives all engines one tested merge
(`sagascript-core/src/transcription/chunk_merge.rs`).

## Transport

- The client spawns the host with `--protocol 1`. stdin carries requests, stdout carries responses
  and events, both as **JSON Lines** (one UTF-8 JSON object per `\n`-terminated line, ≤ 16 MiB).
  stdout must contain nothing else; the host writes diagnostics to **stderr** only.
- The host exits promptly when stdin reaches EOF (parent gone) and when it notices its parent
  process changed (orphaned). It never opens network connections and never touches the
  microphone. It reads only `model_dir` and `pcm_path` files named by the client.

## Envelope

| Kind | Shape |
|---|---|
| Request | `{"v":1,"id":<u64 ≥ 1>,"op":"<op>", ...params}` |
| Success | `{"id":<same>,"ok":true, ...result}` |
| Failure | `{"id":<same>,"ok":false,"error":{"code":"<code>","message":"<text>","retryable":<bool>}}` |
| Event | `{"id":<request id>,"event":"<name>", ...fields}` (zero or more before the terminal response) |

Every request gets exactly one terminal response. Responses to concurrent requests may arrive in
any order; correlate by `id`. Unknown fields are ignored (forward compatibility); an unknown `op`
fails with `unsupported`. Breaking changes bump `protocol`.

Error codes: `protocol` (e.g. op before `hello`, bad envelope), `bad_request`, `unsupported`,
`not_loaded`, `model_missing`, `model_load_failed`, `busy` (retryable), `cancelled`, `engine`,
`internal`.

All `*_ms` and `*_bytes` fields are non-negative JSON integers (clients also accept and round
non-negative floats for robustness).

## Operations

### `hello` (must be first)
Request: `{"client":{"name","version","git_sha"}}`.
Result:
```json
{"protocol":1,
 "host":{"name":"sagascript-engine-host","version":"1.4.0","git_sha":"<40 hex>","engine":"coreml","engine_version":"<os/framework info>"},
 "capabilities":{"sample_rate":16000,"max_window_s":30.0,"preferred_window_s":30.0,"preferred_overlap_s":6.0,
                 "max_in_flight":4,"token_timestamps":true,"languages":["sv"],
                 "compute_units":["ane","gpu","cpu","all"],"min_macos":"14.0"}}
```
The client rejects a host whose `protocol` differs or whose `git_sha` is missing, and logs a
warning when the host build differs from its own (build-identity invariant).

### `load`
Request: `{"model_dir":"<abs path>","model_id":"pianissimo-sv-coreml-<rev>","compute_units":"ane"}`.
Result: `{"model_id","load_ms","compiled":<bool: first-time device compile happened>,
"window_s","frame_s","vocab_size","blank_id"}`. May emit `{"event":"load_progress","phase":"compiling"}`.
`compiled` is true only when this load compiled at least one `.mlpackage`; a load served from the
compiled-model cache (`~/Library/Caches/Sagascript/EngineHost/<key>/<Name>.mlmodelc`, valid once its
`.complete` marker exists) reports false. The host loads from that stable path so the OS Neural
Engine compile cache is reused, and accepts float16 or float32 tensors on the model interface.
Loading the already-loaded model is a no-op success; loading another model replaces it.
`window_s` is the model's fixed input length and **the effective window**: the `hello`
capabilities are announced before any model is loaded and are only defaults. After `load` the
client uses `max_window_s = min(hello.max_window_s, LoadResult.window_s)` and
`preferred_window_s ≤ max_window_s` (overlap is reduced if it no longer fits); it must never send
a window longer than `LoadResult.window_s` (the host answers `bad_request`). The client
recomputes this after every `load` (restart, reload after idle unload).

### `transcribe_window`
Request:
```json
{"pcm_path":"<abs path>","offset_samples":0,"num_samples":480000,"sample_rate":16000,
 "format":"f32le","priority":"batch"}
```
- `pcm_path` is a raw little-endian float32 mono file written by the client (0600, private temp
  dir, deleted by the client). The host reads `num_samples` starting at `offset_samples`.
- `num_samples ≤ max_window_s × sample_rate`, else `bad_request`. Shorter windows are padded
  internally; tokens beyond the real audio are dropped.
- `priority` is `interactive` (dictation) or `batch` (file chunks). When more requests are queued
  than `max_in_flight`, interactive ones run first. A host that is not loaded answers `not_loaded`;
  the client then reloads and resends the window once.

Result:
```json
{"tokens":[{"id":123,"text":"▁hej","start":0.24,"duration":0.16,"confidence":0.98}],
 "audio_s":30.0,"timings":{"preprocess_ms":4,"encode_ms":55,"decode_ms":20}}
```
`start`/`duration` are seconds relative to the window start. `text` is the raw SentencePiece piece
(word boundary `▁`), so the client can merge and detokenize identically for every engine. Tokens are
in time order. `confidence` is optional.

### `cancel`
Request: `{"target":<request id>}` → `{"cancelled":<bool>}`. If the target has not finished, it
terminates with error `cancelled`. Cancellation is best effort; a window may still complete.

### `status`, `ping`, `unload`, `shutdown`
- `status` → `{"state":"idle|loading|ready|busy","model_id":null|"…","in_flight":0,"rss_bytes":…,"uptime_s":…}`
- `ping` → `{}`; always answered, even while busy.
- `unload` → `{}`; frees model memory, process stays.
- `shutdown` → `{}`; the host flushes and exits 0.

`status`, `ping` and `cancel` are always served immediately and never count toward `max_in_flight`.

## Client behavior (normative for Sagascript's Rust client)

- Timeouts: `hello` 10 s, `load` 180 s (first-time ANE compile), `ping` 2 s, `transcribe_window`
  max(15 s, 3 × window seconds); `shutdown` grace 2 s, then kill.
- Long audio: decode to 16 kHz mono f32 once, write one PCM file, plan windows with
  `chunk_merge::plan_windows(preferred_window_s, preferred_overlap_s)` with the effective window
  (see `load`), keep at most `max_in_flight − 1` batch windows in flight (`max_in_flight` when it
  is 1), merge strictly in window order with `chunk_merge::merge_window`, report
  progress as merged windows / total, cancel by not sending further windows and cancelling the
  ones in flight.
- Dictation: one window (utterances longer than `max_window_s` use the long-audio path) with
  `priority:"interactive"`; the client never queues it behind file windows. One host slot is
  therefore reserved for interactive work (batch is capped at `max_in_flight − 1`), so a dictation
  request starts immediately even while a file job saturates the batch lane. The Core ML host
  advertises `max_in_flight` 4 for this reason (3 batch windows is where throughput plateaus).
  Hosts SHOULD advertise `max_in_flight` ≥ 2 so a client can reserve one slot for interactive
  work. A host advertising 1 gets no interactive reservation: the client logs a warning once per
  host, and dictation may wait for at most one running batch window (it is still sent ahead of any
  queued batch window).
- Cancelling a wait for `load` (for example the user aborts dictation during the first-use compile)
  stops the client's wait only: the host is not killed and keeps loading, and the next caller
  resumes waiting for that same `load`.
- Stdout lines are read with a hard 16 MiB bound. A longer line (or a flood without a newline)
  is a protocol violation: the client kills the host and fails in-flight requests with a protocol
  error; the restart budget applies.
- `SAGASCRIPT_ENGINE_HOST` and `SAGASCRIPT_PIANISSIMO_MODEL_DIR` are honored in development builds
  only (debug, or the `dev-overrides` cargo feature); release and signed builds ignore them (with
  one warning) and use the bundled host and the manifest-verified model.
- Lifecycle: lazy start, optional pre-warm, idle `unload` then `shutdown`, crash detection with
  bounded restart backoff. A request that was in flight when the host crashed fails with the
  stderr tail; it is not silently retried more than once.

## Engine hosts

| Host | Location | Engine | Platforms |
|---|---|---|---|
| Core ML | `src-tauri/engine-host/coreml/` (`sagascript-engine-host`) | `coreml` | macOS 14+ Apple silicon |
| ONNX Runtime | `src-tauri/engine-host/ort/` (`sagascript-engine-host-ort`) | `onnx` | any (CPU); shipped for Windows on ARM |

The ONNX host runs KlangAI's official `int8` ONNX export (`encoder-model.int8.onnx`,
`decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`, `config.json` in `model_dir`) on
the ONNX Runtime CPU execution provider. Differences visible on the wire: `hello.host.engine`
is `onnx`; `engine_version` is `unloaded` until the first `load` has loaded ONNX Runtime;
`capabilities.compute_units` is `["cpu"]` and `min_macos` is absent (clients must treat it as
optional); `load` accepts any `compute_units` value and runs on the CPU; `load_ms`/`compiled`
follow the same rules (`compiled` is always false). It advertises `max_in_flight` 2 so the
client reserves an interactive slot, but executes one window at a time and starts queued
interactive windows first. ONNX Runtime is loaded at run time from next to the executable
(`onnxruntime.dll` on Windows, `libonnxruntime.dylib` on macOS); `ORT_DYLIB_PATH` is honoured only in
debug builds or with the `dev-overrides` feature. On Windows the DLL search path is restricted to the
application directory and System32. Tuning
variables for measurements only: `SAGASCRIPT_ORT_THREADS`, `SAGASCRIPT_ORT_DEC_THREADS`,
`SAGASCRIPT_ORT_PRE_THREADS`, `SAGASCRIPT_ORT_ARENA=1`, `SAGASCRIPT_ORT_SLOTS`.
This host does not pad short windows: the models take dynamic input lengths, so work scales with the
real audio. `SAGASCRIPT_ORT_TRACE=1` prints one stderr line per window (frames, session-run time,
joint steps, tokens); `scripts/benchmark-windows-engine-host.ps1` sweeps these settings on a device.

## Conformance

`scripts/engine-host-conformance.py <host binary> --model-dir <dir>` exercises `hello`, `load`,
`transcribe_window` (including out-of-order concurrent responses and an over-long window),
`cancel`, `status`, `ping`, `unload`, EOF exit and `shutdown`. Every engine host must pass it.
The script is engine-agnostic (it sends `compute_units:"ane"`, which non-Core ML hosts map to
their only device); set `ORT_DYLIB_PATH` when running it against the ONNX host.
