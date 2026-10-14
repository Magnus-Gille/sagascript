# Contributing to Sagascript

Thanks for your interest in contributing! This guide will help you get set up and understand the project conventions.

## Development setup

### Prerequisites

**All platforms:**

- Rust 1.88+ (the minimum supported version, `rust-version` in `src-tauri/Cargo.toml`). Use rustup with the development toolchain pinned in `rust-toolchain.toml`, including clippy and rustfmt; if it is missing, `npm run verify` prints the exact `rustup toolchain install` command
- Node.js 22+ (`.nvmrc`), then `npm ci`
- Python 3.10+ (CI uses 3.12) for the evaluation tooling tests
- Tauri CLI: `cargo install tauri-cli`

**macOS:**

- macOS 13.0+ on Apple Silicon for the default diarization-enabled build
- Xcode 16+ (Swift 6, for the Swift engine host tests)

**Linux:**

- `sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libssl-dev build-essential pkg-config file` (Debian/Ubuntu package names; other distributions need the equivalent development packages; runtime notes such as `xdotool` are in [docs/linux-notes.md](docs/linux-notes.md))

**Windows:**

- Windows 10 (version 1803+) or Windows 11
- [Visual Studio Build Tools 2022](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the **"Desktop development with C++"** workload (required by `whisper-rs` to compile `whisper.cpp`)

### Getting started

```bash
git clone https://github.com/Magnus-Gille/sagascript.git
cd sagascript
npm ci && npm run build   # dependencies, then dist/ (needed before cargo, see below)
npm run verify:fast       # checks the setup; its preflight names anything missing
cargo tauri dev
```

Two things a fresh clone or worktree needs before the checks below work:

- `npm run build` creates `dist/`. Every cargo command that compiles the app crate (`cargo
  check`, `test`, `clippy` in `src-tauri/`, including `--workspace`) fails in
  `tauri::generate_context!` until it exists; `npm run verify` reports it as a missing
  prerequisite of exactly those checks.
- `swift test` in `src-tauri/engine-host/coreml` needs the gitignored `BuildInfo.swift`;
  run `./scripts/build-engine-host.sh` once first (macOS). The `swift` check of
  `npm run verify` does both.

For how the code fits together, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md); for
common changes, follow the recipes in [docs/recipes/](docs/recipes/README.md).

### Running tests

`npm run verify` is the one entry point; [scripts/verify.mjs](scripts/verify.mjs) defines every
check (command, working directory, platforms, prerequisites, time budget, CI lanes).

```bash
npm run verify:fast                      # inner loop: rustfmt, app clippy, svelte-check, Prettier, unit and UI node tests
npm run verify                           # before a PR: every default check for this OS
npm run verify -- --keep-going           # report every failure instead of stopping at the first
npm run verify -- --only clippy,node-ui  # checks or whole areas, including opt-in checks
npm run verify -- --skip release-guard   # leave checks or areas out
npm run verify -- --list                 # every check with its commands, budget and CI lanes
```

- **Preflight.** Before anything runs, verify looks for what the selected checks need
  (`node_modules`, a built `dist/` for the checks that compile the app crate, the pinned Rust
  toolchain with clippy and rustfmt, the Linux system libraries, Python, Xcode on macOS, …)
  and prints one error listing everything missing with the command that provides it. It never installs anything, and it runs cargo with
  `RUSTUP_AUTO_INSTALL=0` so `rust-toolchain.toml` cannot pull a toolchain in either.
- **Time budgets.** Every check has one. On timeout, Ctrl-C, SIGTERM or SIGHUP the check's
  whole process tree is stopped, and the summary says why. Checks run in their own process
  group, so a SIGKILL of verify itself (which cannot be caught) leaves the running check
  behind; stop verify with Ctrl-C or SIGTERM.
- **Summary.** A table of pass/fail/skip with the reason and duration; the exit status is
  non-zero when a check failed. Checks for another OS (Swift on Linux, for example) are
  listed as skipped.

#### Control matrix

| Area (`--only`) | Checks | What runs (working directory) | Needs | CI lane |
|---|---|---|---|---|
| `fmt` | `fmt`, `fmt-ort` | `cargo fmt --all -- --check` and `cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check` (`src-tauri`) | Rust | test-linux |
| `clippy` | `clippy-workspace`, `clippy-cli-lean`, `clippy-policy` (Linux); `clippy-app` (fast set) | `cargo clippy --workspace --all-targets -- -D warnings`; `cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings` (`src-tauri`); `node scripts/clippy-policy.mjs` (policy counts, see below) | Rust; Linux libraries; `dist/` (workspace, app, policy) | test-macos, test-linux, test-windows (per crate); policy: test-linux |
| `rust-deps` | `cli-lean-deps` | `cargo tree -p sagascript-cli --no-default-features -e normal` contains no cpal/ALSA | Rust | test-linux |
| `rust-test` | `test-workspace`, `test-cli-lean` | `cargo test --workspace`; `cargo test -p sagascript-cli --no-default-features` | Rust; Linux libraries; `dist/` (workspace) | workspace: test-macos, test-windows (`cargo test` and per crate); lean CLI: test-macos, test-linux |
| `core-features` | `core-clippy`, `core-clippy-diarization`, `core-test`, `core-test-record`, `core-test-diarization` | `cargo clippy`/`cargo test -p sagascript-core` without features, with `record`, with `diarization` | Rust | test-macos, test-linux, test-windows |
| `engine-protocol` | `engine-protocol` | `cargo test -p sagascript-engine-protocol` | Rust | test-linux |
| `ort-host` | `ort-host-clippy`, `ort-host-test` | `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (`src-tauri/engine-host/ort`) | Rust | test-linux |
| `deps` | `deny`, `npm-audit` | `cargo deny --locked --workspace check` and the same for the ORT host (`src-tauri`; policy and what to do about a new advisory: `src-tauri/deny.toml`); `npm audit --audit-level=high` (root lockfile). Both fetch advisory data over the network | `cargo install --locked cargo-deny --version 0.20.2` | deny: test-linux; audit: test-macos |
| `release-guard` | `release-guard` | `cargo test --release -p sagascript-core --lib dev_overrides` | Rust | test-linux |
| `svelte-check` | `svelte-check` | `npx svelte-check --tsconfig ./tsconfig.json` | `npm ci` | test-macos |
| `format` | `format` | `npm run format:check` (Prettier over src/, scripts/, tests/; scope in `.prettierignore`; `npm run format` fixes) | `npm ci` | test-macos, test-windows |
| `unit` | `unit` | `npm run test:unit` (Vitest, `src/**/*.test.ts`; see [docs/testing/frontend.md](docs/testing/frontend.md)) | `npm ci` | test-macos, test-windows |
| `node` | `node-ui`, `node-ci`, `node-release` | `npm run test:ui`, `npm run test:ci`, `npm run test:release` | `npm ci` (ui) | test-macos, test-windows (`npm run test:frontend`) |
| `code-health` | `code-health` | `npm run test:code-health` | Node | `code-health.yml`; also in `test:frontend` |
| `python` | `python-dictation-eval`, `python-diarization-reference`; `python-diarization` (opt-in) | `python -m unittest discover -s scripts/dictation_eval -p 'test_*.py'`; `scripts/diarization_sv` reference tests; opt-in: the full suite against a freshly built lean CLI | Python 3.10+; opt-in: `pip install -r scripts/diarization_sv/requirements-evaluation.txt` | all three; diarization: test-macos, test-linux |
| `swift` | `swift` (macOS) | `./scripts/build-engine-host.sh` (generates `BuildInfo.swift`), `./scripts/check-engine-host-identity.sh` on the built host against `git rev-parse HEAD`, then `swift test` (`src-tauri/engine-host/coreml`); CI also pins `DEVELOPER_DIR` to the newest Xcode, verify uses the selected one | Xcode 16+ | test-macos |
| `msrv` | `msrv` (opt-in; run it for dependency bumps or newer language features) | `cargo check --workspace --locked`, the lean CLI and the ORT host on the `rust-version` toolchain | `rustup toolchain install 1.88 --profile minimal`; `dist/` | msrv-linux |
| `browser` | `browser` (opt-in) | `npm run test:browser` (Playwright against the mocked IPC) | `npx playwright install chromium` | test-macos |

CI splits the cargo commands per crate and feature set; `--list` shows which CI commands each
check stands for, and `scripts/test-verify-ci-parity.mjs` fails when ci.yml's test lanes and
this list drift apart. Not part of verify: release builds and CLI smoke tests (build-macos,
build-windows), engine-host staging, the DER/JER cross-check against a downloaded scorer, and
the release checks `npm run release:check` and `npm run licenses:check`. Where every suite
lives is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#5-where-tests-live-and-how-ci-runs-them).

**Frontend tests.** Frontend modules are tested with Vitest next to the code
(`src/lib/<module>.test.ts`, IPC through the typed mock in `src/test/ipc.ts`):
[docs/testing/frontend.md](docs/testing/frontend.md) has the patterns and the rules. Do not
add `scripts/test-*.mjs` files that read `.svelte` source.

**Node test groups.** `scripts/node-test-groups.mjs` puts every `test-*.mjs` and `test_*.mjs`
file anywhere under `scripts/` (except `fixtures/`) in one group: `ui` (the remaining
component source-text tests, being replaced by #361), `ci`, `release` or `codeHealth`.
`npm run test:frontend` runs all of them (as CI does); a new test file that is not in the
manifest makes the runner fail. Tests in both `ci` and `release` read workflow files, so after
editing `.github/workflows/` run `npm run test:frontend`, not one group.

#### Reference: the individual commands

From `src-tauri/`, after `npm run build` (a bare `cargo test` covers only the app crate; use
`--workspace` or `-p`):

```bash
cargo fmt --all -- --check
cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings
cargo test --workspace
cargo test -p sagascript-cli --no-default-features
```

From the repo root: `npx svelte-check --tsconfig ./tsconfig.json`, `npm run test:unit`,
`npm run format:check` and `npm run test:frontend`.

#### Example: a typical feature

Say you add a user setting that changes how dictation pastes. It crosses the CLI/core/GUI
contract in three places; [docs/recipes/add-setting.md](docs/recipes/add-setting.md) has the
exact files and steps.

1. **Core** (`src-tauri/crates/sagascript-core/src/settings/manager.rs`): the field, its
   default and serde round trip. Focused, from `src-tauri/`: `cargo test -p sagascript-core settings`.
2. **CLI first** (`src-tauri/crates/sagascript-cli/src/config.rs`): the `config get/set/reset`
   key. Focused: `cargo test -p sagascript-cli config`, then
   `npm run verify -- --only test-cli-lean,clippy-cli-lean` for the batch-only build.
3. **GUI** (the setter in `src-tauri/src/commands.rs`, `src/lib/api.ts`, `Settings.svelte`, the
   browser mock): focused: `npm run verify:fast`; if the Settings flow changed visibly, also
   `npm run verify -- --only browser`. The recipe's CLI/GUI parity check is manual.

Before the PR run `npm run verify`: the `core-features` checks catch a field used behind a
feature gate, the lean-CLI checks code that compiles only with default features, and the
`release` Node group the release-surface tests that read the Settings UI.

## Code style

- **Rust:** Prefer clarity over cleverness. Keep modules small and testable. Run `cargo fmt --all` (plus `cargo fmt --manifest-path engine-host/ort/Cargo.toml --all` for the separate ORT host workspace) before committing; CI rejects unformatted code (`src-tauri/rustfmt.toml`).
- **Svelte 5:** Use runes (`$state`, `$effect`), not legacy stores.
- **TypeScript and Node scripts:** Run `npm run format` (Prettier, `prettier.config.js`) before committing; CI rejects unformatted files in `src/`, `scripts/` and `tests/` (`.svelte` files follow once #361 replaces the source-text tests).
- **No hardcoded secrets** -- ever.

### Clippy policy lints

The clippy gate above (`-D warnings`) uses clippy's default lints. On top of it, CI counts a
stricter set of policy lints in production code and fails when any count rises
([#356](https://github.com/Magnus-Gille/sagascript/issues/356)): `unwrap_used`, `expect_used`,
`panic`, `await_holding_lock`, `too_many_lines` (over 150 lines), `cognitive_complexity`
(over 30), `dbg_macro`, `print_stdout` and `print_stderr`. They are not in
`[workspace.lints]`, because the existing findings would fail the `-D warnings` gate.

- **Check:** `node scripts/clippy-policy.mjs` from the repo root, or
  `npm run verify -- --only clippy-policy` (CI: test-linux; verify runs it on Linux only). It runs
  clippy with only these lints on the whole workspace and on the batch-only CLI
  (`-p sagascript-cli --no-default-features`), prints a lint by crate table (baseline, now,
  delta) and exits non-zero if a count is above `src-tauri/clippy-policy-baseline.json`,
  listing where that lint currently fires in that crate. It also fails when clippy reports a
  policy lint as renamed or unknown (a toolchain bump), instead of reading that as zero.
- **What counts:** the lib and bin targets of the workspace crates; not tests, examples,
  benches or build scripts. Each source location counts once. Only the platform it runs on is
  measured, so code behind `cfg(target_os = "macos")` or `cfg(target_os = "windows")` is not
  counted. `src-tauri/clippy.toml` sets the size thresholds and lets tests unwrap, expect,
  panic and print.
- **New code:** return or handle the error (`?`, `map_err`, a fallback) instead of `unwrap`
  or `expect`, and split long functions. If an `expect` guards a real invariant, mark it
  `#[expect(clippy::expect_used, reason = "...")]`; reviewers treat the reason like code.
  The CLI's user-facing output will get a scoped allow for the print lints
  ([#364](https://github.com/Magnus-Gille/sagascript/issues/364)).
- **Lowering the baseline:** after removing findings, run
  `node scripts/clippy-policy.mjs --update` and commit the baseline in the same PR. `--update`
  refuses to raise a count; `--update --allow-increase` does, and the PR must say why.
- **Zero means deny:** when a crate reaches zero for a lint, add the lint to the
  `#![deny(...)]` list in its crate roots so the regular gate enforces it on every platform
  (`sagascript-engine-protocol` denies all of them). Zero is measured on Linux only, so the
  macOS and Windows clippy jobs confirm a new deny.

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
3. Run `npm run verify` (see [Running tests](#running-tests)). A bug fix includes a regression test that fails before the fix ([ADR 0004](docs/adr/0004-test-strategy.md)).
4. Open a pull request with a clear description of what you changed and why.

Keep PRs focused -- one feature or fix per PR. If you're planning a large change, open an issue first to discuss the approach.
