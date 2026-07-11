# Contributing to Sagascript

Thanks for your interest in contributing! This guide will help you get set up and understand the project conventions.

## Development setup

### Prerequisites

**All platforms:**

- Rust 1.75+ with the `stable` toolchain
- Node.js 20+
- Tauri CLI: `cargo install tauri-cli`

**macOS:**

- macOS 13.0+ on Apple Silicon for the default diarization-enabled build

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
# Rust unit tests — all workspace crates (app + sagascript-core + sagascript-cli).
# NOTE: a bare `cargo test` covers only the app crate; use --workspace or -p.
cd src-tauri && cargo test --workspace

# Svelte/TypeScript type checking
npx svelte-check --tsconfig ./tsconfig.json

# Rust lints (per crate; the member crates gate optional features)
cd src-tauri && cargo clippy --workspace --all-targets -- -D warnings
cd src-tauri && cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings
```

## Code style

- **Rust:** Prefer clarity over cleverness. Keep modules small and testable. Follow standard `rustfmt` formatting.
- **Svelte 5:** Use runes (`$state`, `$effect`), not legacy stores.
- **No hardcoded secrets** -- ever.

### CLI-first design rule

Every user-facing feature must have a CLI equivalent. The GUI is a convenience layer on top of CLI commands. When adding a new feature, implement the CLI subcommand first (or alongside the GUI). The CLI commands in `src-tauri/crates/sagascript-cli/src/` are the source of truth for what the app can do.

### Privacy-first rule

Default to local transcription. Remote/cloud features are always opt-in, never default.

## Architecture overview

```
src/                            # Svelte 5 frontend (menu bar UI)
src-tauri/                      # Rust workspace (root package = the Tauri app)
  src/                          # App crate: GUI shell + desktop integrations
    hotkey/                     # Global hotkey service
    paste/                      # Paste-into-active-app service
    platform/                   # Platform-specific code (macOS, Windows stubs)
    logging/                    # Structured logging
  crates/sagascript-core/       # Lib crate: the transcription engine
    src/audio/                  # Audio capture (`record` feature), decode, resample
    src/transcription/          # Whisper backend, model management
    src/settings/               # Settings store (shared between CLI and GUI)
    src/diarization/            # Speaker diarization (`diarization` feature)
  crates/sagascript-cli/        # Lib + bin crate: CLI subcommands (clap)
```

The workspace root package is the Tauri app, so bare `cargo build`/`test`/
`clippy` in `src-tauri/` cover the app only — use `--workspace` or `-p` for the
member crates. Both the app crate and `sagascript-cli` produce a binary named
`sagascript`; `target/release/sagascript` is whichever one was built last, so
build with `-p sagascript-cli` (headless) or `cargo tauri build` (app)
immediately before using that path.

## Platform-specific notes

### macOS: threading caveat

`enigo` (used for auto-paste) and other TIS/HIToolbox APIs **must run on the main thread**. From async contexts, use `app_handle.run_on_main_thread()`. Calling these APIs from a tokio worker thread will cause a SIGTRAP crash. This restriction does not apply on Windows.

### macOS: TCC permission reset for dev builds

After rebuilding, macOS may invalidate previously granted permissions
(Microphone and Accessibility) because an ad-hoc code signature
changes. A build in `target/` and an installed copy in `/Applications` are also
different app identities from TCC's perspective, even if their icons and names
match.

For a clean permission test, quit all Sagascript copies and run:

```bash
./scripts/reset-macos-permissions.sh
```

Remove stale Sagascript entries from **System Settings > Privacy & Security >
Microphone and Accessibility**. If the old Accessibility row
will not update, remove it with the minus button, click plus, and explicitly add
`/Applications/Sagascript.app`. Install one fresh build there, launch that exact
copy, and re-grant permissions. Do not test a `target/debug` copy at the same
time.

Pre-launch builds used the old `com.sagascript.app` identifier. The helper resets
both that identity and the production `ai.gille.sagascript` identity. User
settings migrate automatically; macOS deliberately requires permissions to be
approved again. Official releases use a stable Developer ID signature so this
should happen only for the identity transition, not on every update.

### Windows: debugging notes

- **No TCC equivalent.** Windows does not require accessibility or input monitoring permissions. Microphone access is the only permission needed and is managed via Windows Settings.
- **`whisper-rs` compilation** requires the MSVC C++ toolchain (installed via Visual Studio Build Tools). If you get linker errors, verify the "Desktop development with C++" workload is installed.
- **Hotkey conflicts.** If the global hotkey doesn't register, check for conflicts with other applications or Windows keyboard shortcuts.

## Submitting changes

1. Fork the repo and create a feature branch from `main`.
2. Make your changes, following the code style guidelines above.
3. Ensure tests pass (`cargo test --workspace`, `npx svelte-check`, `cargo clippy --workspace --all-targets -- -D warnings`).
4. Open a pull request with a clear description of what you changed and why.

Keep PRs focused -- one feature or fix per PR. If you're planning a large change, open an issue first to discuss the approach.
