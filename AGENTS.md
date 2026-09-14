# Sagascript — project instructions

Sagascript is a low-latency, privacy-first macOS dictation app built with Tauri v2,
Rust, Svelte 5, and local Whisper inference.

## Product invariants

- **CLI first:** every feature needs a CLI equivalent. Commands in
  `src-tauri/crates/sagascript-cli/src/` define the source-of-truth capability surface;
  discover the current inventory with `sagascript --help` rather than duplicating it here.
- **Privacy first:** local transcription is the default; remote/cloud behavior is opt-in.
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
  `app_handle.run_on_main_thread()`.
- Signed builds need the audio-input entitlement. Set the signing identity through
  `APPLE_SIGNING_IDENTITY`, never in `tauri.conf.json`.
- `cargo tauri dev` is unsigned and cannot validate TCC permission behavior. Use
  `cargo tauri build --debug` for microphone or Accessibility permission testing.

## Code layout

- `src/`: Svelte frontend.
- `src-tauri/src/`: Tauri shell and desktop integrations.
- `src-tauri/crates/sagascript-core/src/`: transcription, audio, settings, and diarization.
- `src-tauri/crates/sagascript-cli/src/`: canonical CLI commands.
- The app crate and CLI crate both emit a binary named `sagascript`; `target/release/sagascript`
  is whichever one was built most recently.

## Verification

Run Rust commands from `src-tauri/`:

- `cargo check --workspace`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build -p sagascript-cli --no-default-features` for the lean batch CLI

For frontend changes, run `npx svelte-check --tsconfig ./tsconfig.json` from the repo root.
