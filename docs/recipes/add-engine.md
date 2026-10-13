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

1. Rust types: `src-tauri/crates/sagascript-engine-protocol/src/lib.rs` (`RequestOp`,
   `Capabilities`, `HelloResult`, `LoadResult`, `ErrorCode`, limits such as
   `MAX_LINE_BYTES`). Tests in `src/tests.rs`. The core client, the ORT host and the fake host
   all use this crate.
2. Swift copy: `src-tauri/engine-host/coreml/Sources/EngineHostCore/EngineTypes.swift` and
   `ProtocolServer.swift` are hand-written duplicates; change them in the same PR.
3. Spec: update `docs/engine-host-protocol.md`.
4. Compatibility: new fields are optional and omitted when unused (unknown fields are ignored
   by both sides); a breaking change bumps `PROTOCOL_VERSION` and the client rejects other
   versions in `hello`.

Planned: [#367](https://github.com/Magnus-Gille/sagascript/issues/367) adds golden JSONL
transcripts shared by Rust and Swift tests and runs conformance in CI.

## 2. The host process

- Speak JSON Lines on stdin/stdout when started with `--protocol 1`; diagnostics only on
  stderr; exit on stdin EOF or when orphaned; answer `ping`, `status` and `cancel` immediately.
- Report build identity in `hello` (`version`, 40-hex `git_sha`); the client rejects a missing
  SHA. The Core ML host gets it from `BuildInfo.swift` written by
  `scripts/build-engine-host.sh`; the ORT host from its `build.rs`.
- Advertise `max_in_flight` ≥ 2 so the client can reserve a slot for dictation.
- Layout options: a Swift package like `src-tauri/engine-host/coreml/`, or a standalone Cargo
  workspace like `src-tauri/engine-host/ort/` (own `[workspace]` and `Cargo.lock`, depends on
  `sagascript-engine-protocol` by path). Keep `version` in step with the app release.

## 3. Shared test vectors

Logic that must match across hosts gets vectors in `src-tauri/engine-host/test-vectors/`.
Today `context-biasing.json` is loaded by `ContextBiasingTests.swift` and by
`engine-host/ort/src/boost.rs` (`include_str!`). A new host that supports context biasing
loads the same file in its unit tests; new shared behaviour gets a new vector file used by
every host.

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
| Protocol crate | `cargo test -p sagascript-engine-protocol` (CI: test-linux). |
| Swift host | `scripts/build-engine-host.sh`, then `swift test` in `src-tauri/engine-host/coreml` (CI: test-macos). |
| ORT host | In `src-tauri/engine-host/ort/`: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` (CI: test-linux runs all three; locally `npm run verify -- --only ort-host,fmt`). |
| Conformance | `scripts/engine-host-conformance.py <host binary> --model-dir <dir>`; every host must pass it (needs a model; not in CI). |
| End to end | `sagascript engine status --json`, `sagascript engine doctor`. |
| macOS packaging | `scripts/stage-engine-host.sh`, `scripts/tauri-engine-host-bundle.json`, `scripts/check-engine-host-identity.sh`. |
| Windows packaging | `scripts/tauri-windows-engine-host.json`, `.github/workflows/windows-package.yml`. |

## 6. A new engine in the app

There is no engine trait today: Whisper and Pianissimo are chosen by hand in
`live_dictation.rs` (`LiveDictationBackend::new(whisper, pianissimo)`), `plain_file_jobs.rs`
and the CLI `transcribe.rs`, and the model choice is the settings enums `FileModel` /
`FileModelPreference` in `settings/manager.rs`. Adding one therefore touches those call sites,
the model catalogue (`list-models`, `get_model_info`), settings (see
[add-setting.md](add-setting.md) for the enum rules), the CLI `--model` parsing and the UI
model pickers. Plan it with an issue first. Planned:
[#378](https://github.com/Magnus-Gille/sagascript/issues/378) introduces a `SpeechEngine`
trait and one cancellation model; [#366](https://github.com/Magnus-Gille/sagascript/issues/366)
makes host supervision recover from `Failed`.

Finish with `npm run verify` ([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)).
