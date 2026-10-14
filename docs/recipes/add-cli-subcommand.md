# Recipe: add a CLI subcommand

Subcommands live in `src-tauri/crates/sagascript-cli/`. The headless binary and the desktop
binary share them: the app's `main()` calls `sagascript_cli::try_parse()` and
`sagascript_cli::run()`, so a new subcommand needs no app change. Completions and man pages
are generated from the clap definitions. Example: `sagascript example [--json]`.

## 1. Module and arguments

Create `src-tauri/crates/sagascript-cli/src/example.rs`:

```rust
use clap::Args;
use sagascript_core::error::DictationError;

#[derive(Args)]
pub struct ExampleArgs {
    /// Machine-readable JSON on stdout
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ExampleArgs) -> Result<(), DictationError> {
    let report = build_report()?;          // pure, unit-tested
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("Example: {}", report["status"]);
    }
    Ok(())
}
```

Nested actions use a `#[derive(Subcommand)]` enum inside the args (`engine.rs`, `config.rs`).
Put the domain logic in `sagascript-core` when the GUI needs it too.

## 2. Wire it into `src/lib.rs`

1. `pub mod example;` — gate it with `#[cfg(feature = "record")]` or
   `#[cfg(feature = "diarization")]` if it needs that stack (see `record`).
2. Add a variant to `pub enum Command` with a one-line doc comment (short help), a
   `long_about` and an `after_long_help` with `EXAMPLES:` (copy the style of the
   neighbours). Gate the variant with the same `cfg`.
3. Add the arm in `pub fn run(cli: Cli)`. Wrap inference work in
   `run_inference_command("example", move || example::run(args))` (Windows needs the larger
   stack); async work uses `rt.block_on(...)` like `DownloadModel`.

## 3. Output conventions (current)

- stdout carries the result only: human text, or with `--json` exactly one JSON document
  (`serde_json::to_string_pretty`), or with `--jsonl` one compact object per line for
  per-item streams (`transcribe`). Keep JSON keys snake_case and stable.
- stderr carries everything else: confirmations (`config set` prints `Set key = value`),
  warnings, progress, and `tracing` logs (the subscriber writes to stderr at `warn` by default).
  `scripts/verify-json-cli-streams.py` checks this contract for `transcribe --json` in CI.
- Never write transcripts, audio or personal paths to logs
  ([ADR 0002](../adr/0002-privacy-first.md)).

## 4. Exit codes (current)

| Code | When |
|---|---|
| 0 | `run` returned `Ok`. |
| 1 | Any `Err` (printed as `Error: …` on stderr by `run`), including failed checks such as `engine doctor`. |
| 2 | clap usage errors; the headless binary without a subcommand. |
| 130 / 143 | Cancelled by SIGINT / SIGTERM (`cancel.rs`: `cancel::is_cancelled()`, `cancel::exit_code()`). |

Planned: [#364](https://github.com/Magnus-Gille/sagascript/issues/364) adds an `Out` writer, a
typed `CliError`/`ExitCode` (including a distinct check-failed code), structured JSON errors
and broken-pipe handling. Until then follow the table and do not call `std::process::exit`
in subcommands.

## 5. Tests

1. Unit tests in `example.rs` for the pure parts. Inject environment-dependent inputs
   (see `Probe` in `engine.rs`) instead of reading the machine.
2. An integration test `src-tauri/crates/sagascript-cli/tests/example.rs` that spawns
   `env!("CARGO_BIN_EXE_sagascript")` with `SAGASCRIPT_SETTINGS_PATH` set to a file in a
   `tempfile` directory (a dev-dependency), parses stdout as JSON and checks the exit status.
   Set environment on the child `Command`, not with `std::env::set_var` in the test process.
   Model: `tests/diarization_progress.rs` (`tempdir()`, `env_clear()` plus an allow-list,
   `SAGASCRIPT_SETTINGS_PATH`); a test that needs a feature starts with
   `#![cfg(feature = "...")]` like that file.
3. Run both feature modes, because CI does:

```bash
cd src-tauri
cargo test -p sagascript-cli
cargo test -p sagascript-cli --no-default-features
cargo clippy -p sagascript-cli --all-targets -- -D warnings
cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings
cargo run -q -p sagascript-cli -- example --json
```

## 6. CI smoke (model-free commands)

If the command is cheap and needs no model, add it to the "Tier 1: CLI smoke tests" steps
of `build-macos` (bash) and the matching steps of `build-windows` (PowerShell) in
`.github/workflows/ci.yml`. Some `scripts/test-ci-*.mjs` files assert on `ci.yml` text, so
run `npm run test:frontend` after editing it.

## 7. Docs

`sagascript --help` is the inventory; do not list commands in AGENTS.md. Add a README "CLI
usage" example if users need it, and a GUI path if the feature is user-facing
([ADR 0001](../adr/0001-cli-first.md)). Then run the full list in
[CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests).
