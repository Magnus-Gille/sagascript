# Sagascript — Claude Code Project Context

## What this is

Sagascript is a low-latency, privacy-first macOS dictation app built with Tauri v2 (Rust backend + Svelte 5 frontend). It provides push-to-talk transcription using local Whisper models via whisper-rs (Metal + Core ML).

## Golden rules

- **CLI-first design**: Every feature must have a CLI equivalent. The GUI is a convenience layer on top of CLI commands. The CLI commands in `src-tauri/crates/sagascript-cli/src/` are the source of truth for what the app can do.
- **Privacy-first**: Default to local transcription. Remote/cloud features are opt-in, never default.
- Optimize for **latency** and **perceived speed**.
- Keep UI minimal (menu bar + settings + indicator).
- **Always work in worktrees**: When making changes, always create a separate branch in a git worktree (`isolation: "worktree"` for Task agents, or `EnterWorktree` for the main session). Never modify the main working tree directly — unstaged changes leak across branches and interfere with parallel work.

## Code style

- Rust: prefer clarity over cleverness, keep modules small and testable
- Svelte 5: use runes (`$state`, `$effect`), not legacy stores
- No hardcoded secrets
- macOS threading: `enigo` and other TIS/HIToolbox APIs MUST run on the main thread — use `app_handle.run_on_main_thread()` from async contexts

## macOS Signing & Entitlements

- **Signing identity** is NOT in `tauri.conf.json` — set via `APPLE_SIGNING_IDENTITY` env var (see `.env`, gitignored)
- **Entitlements.plist** grants `com.apple.security.device.audio-input` — required for mic access in signed builds
- `cargo tauri dev` produces an unsigned binary that can't do TCC permission checks — use `cargo tauri build --debug` for testing permissions
- Microphone permission API uses `AVCaptureDevice` via objc FFI (`macos_mic` module in `commands.rs`)

## Local commands

- `cargo tauri dev` — build and run the app in dev mode
- `cargo check --workspace` — type-check Rust (from `src-tauri/`; bare `cargo check` covers only the app crate)
- `cargo test --workspace` — run Rust unit tests across all three crates (from `src-tauri/`)
- `cargo clippy --workspace --all-targets -- -D warnings` — lint Rust (from `src-tauri/`)
- `cargo build -p sagascript-cli --no-default-features` — lean batch-transcribe CLI build (no cpal/ALSA, no diarization and its ONNX Runtime; ~8 MB vs ~31 MB default)
- `npx svelte-check --tsconfig ./tsconfig.json` — type-check Svelte/TS
- `tail -f ~/Library/Logs/Sagascript/sagascript.log` — watch logs

## CLI subcommands

- `sagascript transcribe <file>` — transcribe audio/video file
- `sagascript record` — record from mic and transcribe
- `sagascript list-models` — list available whisper models
- `sagascript download-model <id>` — download a model
- `sagascript config list|get|set|reset|path` — manage settings
- `sagascript formats` — list supported audio formats
- `sagascript completions <shell>` — generate shell completions
- `sagascript manpages [--dir DIR]` — generate man pages

## Architecture

```
src/                            # Svelte 5 frontend (menu bar UI)
src-tauri/                      # Rust workspace (root package = the Tauri app)
  src/                          # App crate: GUI shell + desktop integrations
    hotkey/                     # Global hotkey service
    paste/                      # Paste-into-active-app service
    platform/                   # Platform-specific code (macOS, Windows stubs)
    logging/                    # Structured logging
  crates/sagascript-core/src/   # Lib crate: transcription engine
    audio/                      # Audio capture (`record` feature), decoding, resampling
    transcription/              # Whisper backend, model management
    settings/                   # Settings store (shared between CLI and GUI)
    diarization/                # Speaker diarization (`diarization` feature)
  crates/sagascript-cli/src/    # Lib + bin crate: CLI subcommands (clap)
```

Both the app crate and `sagascript-cli` produce a bin named `sagascript`;
`target/release/sagascript` is whichever was built most recently.
