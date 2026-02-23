# Contributing to Sagascript

Thanks for your interest in contributing! This guide will help you get set up and understand the project conventions.

## Development setup

### Prerequisites

**All platforms:**

- Rust 1.75+ with the `stable` toolchain
- Node.js 20+
- Tauri CLI: `cargo install tauri-cli`

**macOS:**

- macOS 13.0+ (Apple Silicon recommended)

**Windows:**

- Windows 10 (version 1803+) or Windows 11
- [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **"Desktop development with C++"** workload (required by `whisper-rs` to compile `whisper.cpp`)

### Getting started

```bash
git clone https://github.com/Magnus-Gille/sagascript.git
cd sagascript
npm install
cargo tauri dev
```

### Running tests

All checks must pass on both macOS and Windows (CI runs on both platforms).

```bash
# Rust unit tests
cd src-tauri && cargo test

# Svelte/TypeScript type checking
npx svelte-check --tsconfig ./tsconfig.json

# Rust lints
cd src-tauri && cargo clippy -- -D warnings
```

## Code style

- **Rust:** Prefer clarity over cleverness. Keep modules small and testable. Follow standard `rustfmt` formatting.
- **Svelte 5:** Use runes (`$state`, `$effect`), not legacy stores.
- **No hardcoded secrets** -- ever.

### CLI-first design rule

Every user-facing feature must have a CLI equivalent. The GUI is a convenience layer on top of CLI commands. When adding a new feature, implement the CLI subcommand first (or alongside the GUI). The CLI commands in `src-tauri/src/cli/` are the source of truth for what the app can do.

### Privacy-first rule

Default to local transcription. Remote/cloud features are always opt-in, never default.

## Architecture overview

```
src/                  # Svelte 5 frontend (menu bar UI)
src-tauri/src/        # Rust backend
  cli/                # CLI subcommands (clap)
  audio/              # Audio capture, decoding, resampling
  transcription/      # Whisper backend, model management
  settings/           # Settings store (shared between CLI and GUI)
  hotkey/             # Global hotkey service
  paste/              # Paste-into-active-app service
  platform/           # Platform-specific code (macOS, Windows stubs)
  logging/            # Structured logging
  credentials/        # Keyring integration
```

## Platform-specific notes

### macOS: threading caveat

`enigo` (used for auto-paste) and other TIS/HIToolbox APIs **must run on the main thread**. From async contexts, use `app_handle.run_on_main_thread()`. Calling these APIs from a tokio worker thread will cause a SIGTRAP crash. This restriction does not apply on Windows.

### macOS: TCC permission reset for dev builds

After rebuilding, macOS may invalidate previously granted permissions (Microphone, Accessibility, Input Monitoring) because the ad-hoc code signature changes.

To fix:

```bash
tccutil reset Microphone com.sagascript.app
tccutil reset Accessibility com.sagascript.app
```

Then relaunch and re-grant permissions when prompted.

**Why this happens:** Ad-hoc signing generates a new signature each build. macOS ties TCC grants to the signature, not the bundle identifier alone. A stable Developer ID certificate would fix this permanently.

### Windows: debugging notes

- **No TCC equivalent.** Windows does not require accessibility or input monitoring permissions. Microphone access is the only permission needed and is managed via Windows Settings.
- **`whisper-rs` compilation** requires the MSVC C++ toolchain (installed via Visual Studio Build Tools). If you get linker errors, verify the "Desktop development with C++" workload is installed.
- **Hotkey conflicts.** If the global hotkey doesn't register, check for conflicts with other applications or Windows keyboard shortcuts.

## Submitting changes

1. Fork the repo and create a feature branch from `main`.
2. Make your changes, following the code style guidelines above.
3. Ensure tests pass (`cargo test`, `npx svelte-check`, `cargo clippy -- -D warnings`).
4. Open a pull request with a clear description of what you changed and why.

Keep PRs focused -- one feature or fix per PR. If you're planning a large change, open an issue first to discuss the approach.
