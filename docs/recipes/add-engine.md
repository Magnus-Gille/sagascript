# Recipe: add an engine or engine host

Two kinds of change:

- **A new host for the existing protocol** (another runtime or platform for a model the app
  already knows, as `engine-host/ort/` did for Pianissimo on Windows on ARM). Steps 1–5.
- **A new speech engine/model in the app** (a new model family next to Whisper and
  Pianissimo). Steps 1–5 if it runs out of process, plus step 6.

Read [engine-host-protocol.md](../engine-host-protocol.md) (normative) and
[ARCHITECTURE.md §2.4](../ARCHITECTURE.md#24-engine-host-process-lifecycle) first. Hosts never
open network connections or touch the microphone ([ADR 0002](../adr/0002-privacy-first.md)).

## 1. Protocol changes (only if the wire format must change)

The source of truth is the set of golden transcripts in
`src-tauri/engine-host/test-vectors/protocol/` (`*.jsonl` plus the scripted
`fake-engine.json`; format in
[engine-host-protocol.md, Golden transcripts](../engine-host-protocol.md#golden-transcripts)).
Change the wire format in one PR, in this order:

1. Rust types: `src-tauri/crates/sagascript-engine-protocol/src/lib.rs` (`RequestOp`,
   `Capabilities`, `HelloResult`, `LoadResult`, `ErrorCode`, limits such as
   `MAX_LINE_BYTES`). Fields are required unless the spec marks them optional; only optional
   ones get `#[serde(default)]`. The core client, the ORT host and the fake host all use this
   crate.
2. Golden transcripts: add or change the steps that show the new behaviour (and its errors) in
   the `.jsonl` files and, if the engine's answers change, in `fake-engine.json`. A new optional
   result field also goes into `OPTIONAL` in `src/vector_tests.rs`.
3. Core ML host: `src-tauri/engine-host/coreml/Sources/EngineHostCore/WireProtocol.swift` owns
   every wire field name, request parser and response builder of the Swift host, and
   `WireErrorCode` (`EngineTypes.swift`) every error code; `ProtocolServer.swift` (handshake
   state, dispatch, scheduling) goes through them.
4. ONNX host: `src-tauri/engine-host/ort/src/server.rs` (it decodes requests with the protocol
   crate, so usually only new behaviour needs code).
5. Spec: update `docs/engine-host-protocol.md`.
6. Compatibility: new fields are optional and omitted when unused (unknown fields are ignored
   by both sides); a breaking change bumps `PROTOCOL_VERSION` and the client rejects other
   versions in `hello`.

What catches drift: `cargo test -p sagascript-engine-protocol` (`src/vector_tests.rs`) decodes
every expected response with the client's types, checks that every field not listed as optional
is required and that the transcripts cover every op and error code; the Core ML host replays
the transcripts in `ProtocolServerTests.swift` (`swift test`) and the ONNX host in
`src/server/golden_tests.rs` (`cargo test` in `engine-host/ort`). A field renamed in any one of
them fails at least one of these tests.

## 2. The host process

- Speak JSON Lines on stdin/stdout when started with `--protocol 1`; diagnostics only on
  stderr; exit on stdin EOF or when orphaned; answer `ping`, `status` and `cancel` immediately.
- Report build identity in `hello` (`version`, 40-hex `git_sha`, `build_dirty`); the client
  rejects a missing SHA. One definition serves every host: `scripts/engine-host-identity.sh`
  prints version, SHA and dirty, and a tree is dirty when `git status` lists anything besides
  the generated files in `scripts/build-identity-generated-paths.txt`. The Core ML host gets it
  from `BuildInfo.swift`, which `scripts/build-engine-host.sh` writes from that script; the ORT
  host's `build.rs` (`build_identity.rs`) and the Windows packaging gate
  (`scripts/ci-build-identity.mjs`) read the same path list. Release checks
  (`scripts/check-engine-host-identity.sh --release`) reject a dirty host.
- Advertise `max_in_flight` ≥ 2 so the client can reserve a slot for dictation.
- Layout options: a Swift package like `src-tauri/engine-host/coreml/`, or a standalone Cargo
  workspace like `src-tauri/engine-host/ort/` (own `[workspace]` and `Cargo.lock`, depends on
  `sagascript-engine-protocol` by path). Keep `version` in step with the app release.

## 3. Shared test vectors

Logic that must match across hosts gets vectors in `src-tauri/engine-host/test-vectors/`.
`protocol/` holds the golden transcripts (see step 1), which every host replays through its
protocol server against the scripted `fake-engine.json`; a new host does the same in its unit
tests. `context-biasing.json` is loaded by `ContextBiasingTests.swift` and by
`engine-host/ort/src/boost.rs` (`include_str!`); a new host that supports context biasing loads
it too. New shared behaviour gets a new vector file used by every host.

## 4. Client integration: `src-tauri/crates/sagascript-core/src/transcription/`

- `pianissimo_backend.rs`: platform gate (`platform_supported`, `platform_gate`,
  `runtime_supported_on_this_os`), host location (`resolve_host`, the `BUNDLED_HOST` /
  `WINDOWS_HOST_*` constants), compute units (`default_compute_units`), config
  (`config_for_host`), and the process-global client (`shared_client`).
- `pianissimo_model.rs`: model download manifest, directory and verification.
- `engine_host/` (`client.rs`, `process.rs`, `pipeline.rs`) is engine-agnostic; change it only
  for protocol or lifecycle behaviour.
- Development builds can point at a local host with `SAGASCRIPT_ENGINE_HOST` (debug or
  `dev-overrides`); release builds ignore it.

## 5. Tests and packaging

| What | Where / command |
|---|---|
| Client behaviour (restarts, timeouts, ordering, cancel) | `crates/sagascript-core/tests/engine_host.rs` against `sagascript-fake-engine-host` (`tests/support/fake_engine_host.rs`; `FAKE_*` variables select faults). Add a fake mode for new behaviour. |
| Protocol crate | `cargo test -p sagascript-engine-protocol`, including the golden transcripts (CI: test-linux). |
| Swift host | `scripts/build-engine-host.sh`, then `swift test` in `src-tauri/engine-host/coreml`, including the golden-transcript replay (CI: test-macos). |
| ORT host | In `src-tauri/engine-host/ort/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` (CI: test-linux runs all three, test-windows the tests; locally `npm run verify -- --only ort-host,fmt`). |
| Conformance | `scripts/engine-host-conformance.py <host binary> --model-dir <dir>`; every host must pass it. `--no-model` runs the model-free subset: CI test-macos runs it against the staged Core ML host (`npm run verify -- --only engine-host-conformance`). The full run needs a downloaded model and is in the release gate (`scripts/verify-release-draft.sh`); run it by hand for a new host. |
| End to end | `sagascript engine status --json`, `sagascript engine doctor`. |
| macOS packaging | `scripts/stage-engine-host.sh`, `scripts/tauri-engine-host-bundle.json`, `scripts/check-engine-host-identity.sh`. |
| Windows packaging | `scripts/tauri-windows-engine-host.json`, `.github/workflows/windows-package.yml`. |

## 6. A new engine in the app

There is no engine trait today: Whisper and Pianissimo are chosen by hand in
`live_dictation.rs` (`LiveDictationBackend::new(whisper, pianissimo)`), `plain_file_jobs.rs`
and the CLI `transcribe.rs`, and the model choice is the enums `FileModel` (model catalog,
`models/file_model.rs`) and `FileModelPreference` (settings, `settings/manager.rs`). Adding one therefore touches those call sites,
the model catalogue (`list-models`, `get_model_info`), settings (see
[add-setting.md](add-setting.md) for the enum rules), the CLI `--model` parsing and the UI
model pickers. Plan it with an issue first. Planned:
[#378](https://github.com/Magnus-Gille/sagascript/issues/378) introduces a `SpeechEngine`
trait and one cancellation model. Host supervision already recovers from `Failed`
([#366](https://github.com/Magnus-Gille/sagascript/issues/366); see
[ARCHITECTURE.md §2.4](../ARCHITECTURE.md#24-engine-host-process-lifecycle)).

Finish with `npm run verify` ([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)).
