# Contributing to Sagascript

Thanks for your interest in contributing! This guide will help you get set up and understand the project conventions.

## Development setup

### Prerequisites

**All platforms:**

- Rust 1.88+ (the minimum supported version, `rust-version` in `src-tauri/Cargo.toml`). Use rustup: it installs the development toolchain pinned in `rust-toolchain.toml` (with clippy and rustfmt) on the first `cargo` call
- Node.js 22+ (`.nvmrc`)
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

Two things a fresh clone needs before the checks below work:

- `npm run build` creates `dist/`. Every cargo command that compiles the app crate (`cargo
  check`, `test`, `clippy` in `src-tauri/`, including `--workspace`) fails in
  `tauri::generate_context!` until it exists.
- `swift test` in `src-tauri/engine-host/coreml` needs the gitignored `BuildInfo.swift`;
  run `./scripts/build-engine-host.sh` once first (macOS).

For how the code fits together, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md); for
common changes, follow the recipes in [docs/recipes/](docs/recipes/README.md).

### Running tests

This is the canonical verification list; AGENTS.md and the PR template mirror it. All checks
must pass on macOS, Linux and Windows (CI runs all three).

Run before every PR (Rust commands from `src-tauri/`, after `npm run build`):

```bash
# Formatting (CI gate): the main workspace and the standalone ORT engine host
cargo fmt --all -- --check
cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check

# Lints: the whole workspace, plus the lean batch CLI without default features
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings

# Tests. NOTE: a bare `cargo test` covers only the app crate; use --workspace or -p.
cargo test --workspace
cargo test -p sagascript-cli --no-default-features

# Frontend (from the repo root)
npx svelte-check --tsconfig ./tsconfig.json
npm run test:frontend
```

Also run these when your change touches the area:

| Area | Command |
|---|---|
| UI flows (Transcribe tab, update recovery) | `npx playwright install chromium` once, then `npm run test:browser` |
| Feature-gated core code (`record`, `diarization`) | `cargo test -p sagascript-core`, `cargo test -p sagascript-core --features record`, `cargo test -p sagascript-core --features diarization`, `cargo clippy -p sagascript-core --all-targets -- -D warnings` |
| Swift Core ML engine host | `./scripts/build-engine-host.sh`, then `swift test` in `src-tauri/engine-host/coreml` |
| ORT engine host | `cargo test` and `cargo clippy --all-targets -- -D warnings` in `src-tauri/engine-host/ort/` (CI: test-linux) |
| Python evaluation tooling | `python -m unittest discover -s scripts/dictation_eval -p 'test_*.py'`; for `scripts/diarization_sv`, see `.github/workflows/ci.yml` |
| Code-health tooling | `npm run test:code-health` |
| Release metadata, licenses | `npm run release:check`, `npm run licenses:check` |

CI (`.github/workflows/ci.yml`) runs the same cargo checks split per crate and per feature
set, so that the no-feature builds of `sagascript-core` and `sagascript-cli` stay honest; the
workspace commands above are the closest single-machine equivalent. Where every suite lives
and which CI lane runs it is described in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#5-where-tests-live-and-how-ci-runs-them).
Planned: `npm run verify` as one entry point for this list
([#350](https://github.com/Magnus-Gille/sagascript/issues/350)).

## Code style

- **Rust:** Prefer clarity over cleverness. Keep modules small and testable. Run `cargo fmt --all` (plus `cargo fmt --manifest-path engine-host/ort/Cargo.toml --all` for the separate ORT host workspace) before committing; CI rejects unformatted code (`src-tauri/rustfmt.toml`).
- **Svelte 5:** Use runes (`$state`, `$effect`), not legacy stores.
- **No hardcoded secrets** -- ever.

### CLI-first design rule

Every user-facing feature must have a CLI equivalent. The GUI is a convenience layer on top of CLI commands. When adding a new feature, implement the CLI subcommand first (or alongside the GUI). The CLI commands in `src-tauri/crates/sagascript-cli/src/` are the source of truth for what the app can do.

### Privacy-first rule

Default to local transcription. Remote/cloud features are always opt-in, never default.

## Architecture overview

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) is the map: crates and packages, the dictation,
settings, updater and engine-host flows, threads and locks, feature flags and test layout.
Decisions behind the rules are in [docs/adr/](docs/adr/README.md).

The workspace root package is the Tauri app, so bare `cargo build`/`test`/
`clippy` in `src-tauri/` cover the app only — use `--workspace` or `-p` for the
member crates. Both the app crate and `sagascript-cli` produce a binary named
`sagascript`; `target/release/sagascript` is whichever one was built last, so
build with `-p sagascript-cli` (headless) or `cargo tauri build` (app)
immediately before using that path.

## Platform-specific notes

### macOS: threading caveat

`enigo` (used for auto-paste) and other TIS/HIToolbox APIs **must run on the main thread**. From async contexts, use `app_handle.run_on_main_thread()`. Calling these APIs from a tokio worker thread will cause a SIGTRAP crash. This restriction does not apply on Windows. See [ADR 0003](docs/adr/0003-main-thread-native-apis.md).

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
3. Run the checks in [Running tests](#running-tests). A bug fix includes a regression test that fails before the fix ([ADR 0004](docs/adr/0004-test-strategy.md)).
4. Open a pull request with a clear description of what you changed and why.

Keep PRs focused -- one feature or fix per PR. If you're planning a large change, open an issue first to discuss the approach.
