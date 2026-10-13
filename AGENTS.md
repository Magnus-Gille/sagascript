# Sagascript — project instructions

Sagascript is a low-latency, privacy-first macOS dictation app built with Tauri v2,
Rust, Svelte 5, and local Whisper inference.

## Product invariants

- **CLI first:** every feature needs a CLI equivalent. Commands in
  `src-tauri/crates/sagascript-cli/src/` define the source-of-truth capability surface;
  discover the current inventory with `sagascript --help` rather than duplicating it here
  ([ADR 0001](docs/adr/0001-cli-first.md)).
- **Privacy first:** local transcription is the default; remote/cloud behavior is opt-in
  ([ADR 0002](docs/adr/0002-privacy-first.md)).
- Optimize latency and perceived speed; keep the UI to the menu bar, settings, and indicator.
- Every build must identify itself with its release version and source revision/build
  identity in the tray menu, prominently in Settings, and in CLI `--version` output.
  Use generated build metadata rather than manually maintained display strings.
- Make changes only on a task branch in a separate worktree; never edit the primary checkout.
- Never read, print, modify, or commit `.env`, `.env.*`, or `secrets/**` without explicit
  user authorization. Never hardcode secrets.

## Technical invariants

- Use Svelte 5 runes such as `$state` and `$effect`, not legacy stores.
- `enigo` and TIS/HIToolbox APIs must run on the macOS main thread; from async code use
  `app_handle.run_on_main_thread()` ([ADR 0003](docs/adr/0003-main-thread-native-apis.md)).
- Lock order is in `docs/ARCHITECTURE.md` §3. Never hold `SharedController` across `.await`.
- Locks recover from poison via `lock_or_recover`/`lock_controller`; never `lock().unwrap()`
  in non-test code ([ADR 0005](docs/adr/0005-mutex-poison-policy.md)).
- Tests: regression test for every bug, no regex tests over `.svelte` source, fixtures never
  touch real user settings (`SAGASCRIPT_SETTINGS_PATH`) — [ADR 0004](docs/adr/0004-test-strategy.md).
- Signed builds need the audio-input entitlement. Set the signing identity through
  `APPLE_SIGNING_IDENTITY`, never in `tauri.conf.json`.
- `cargo tauri dev` is unsigned and cannot validate TCC permission behavior. Use
  `cargo tauri build --debug` for microphone or Accessibility permission testing.

## Code layout

Map, data flows, threads/locks, feature flags and test layout: `docs/ARCHITECTURE.md`.
Step-by-step changes (command, setting, CLI subcommand, event, engine): `docs/recipes/`.
Decisions and their reasons: `docs/adr/`.

- `src/`: Svelte frontend; `invoke` wrappers live in `src/lib/api.ts`.
- `src-tauri/src/`: Tauri shell and desktop integrations; also dispatches CLI subcommands.
- `src-tauri/crates/sagascript-core/src/`: transcription, audio, settings, diarization, engine-host client.
- `src-tauri/crates/sagascript-cli/src/`: canonical CLI commands.
- `src-tauri/crates/sagascript-engine-protocol/`: engine-host JSON Lines wire types.
- `src-tauri/engine-host/`: `coreml/` Swift host, `ort/` ONNX host (separate Cargo workspace), shared `test-vectors/`.
- The app crate and CLI crate both emit a binary named `sagascript`; `target/release/sagascript`
  is whichever one was built most recently.

## Verification

In a fresh worktree first run `npm ci && npm run build`: `dist/` is needed by every cargo
command that compiles the app crate (`tauri::generate_context!`).

`npm run verify` runs every default check for this OS (opt-in ones, such as browser and
MSRV, only with `--only`) and stops at the first failure (`--keep-going` reports all); `npm run verify:fast` is the inner loop (rustfmt, app clippy,
svelte-check, Prettier, ESLint, unit and UI node tests); `--only <check|area>` focuses; `--list` prints the checks.
`scripts/verify.mjs` defines them, CONTRIBUTING.md "Running tests" has the control matrix. Its
preflight lists every missing prerequisite (including `dist/`) in one error and installs
nothing.

Reference commands, from `src-tauri/`:

- `cargo fmt --all -- --check` and `cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings` (lean batch CLI)
- `cargo test --workspace` (a bare `cargo test` covers only the app crate)
- `cargo test -p sagascript-cli --no-default-features`

From the repo root: `npx svelte-check --tsconfig ./tsconfig.json` and `npm run test:frontend`
(every Node test group; add new test files to `scripts/node-test-groups.mjs`).
Frontend tests are Vitest unit and component tests next to the code (`npm run test:unit`; `npm run format` and `npm run lint` before committing); follow docs/testing/frontend.md and never assert on `.svelte` source text.
