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

use crate::output::{errln, outln, Out};

#[derive(Args)]
pub struct ExampleArgs {
    /// Machine-readable JSON on stdout
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: ExampleArgs, out: &Out) -> Result<(), DictationError> {
    let report = build_report()?;          // pure, unit-tested
    if args.json {
        out.json(&report)?;
    } else {
        outln!(out, "Example: {}", report["status"])?;
    }
    errln!(out, "Example done.");
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
3. Add the arm in `dispatch` (called by `run_with`): `Command::Example(args) =>
   example::run(args, out)?`. Wrap inference work in `run_inference_command("example", move ||
   example::run(args, &out))` with a cloned `out` (Windows needs the larger stack); async work
   builds a runtime and calls `block_on(...)` like `DownloadModel`.
4. If the command has `--json`/`--jsonl`, add it to `json_output` in `lib.rs` (pretty or
   compact, matching what the command writes), so a failure before any output still writes the
   error object below. A command whose stdout is always JSON has no such flag and stays out
   (see "Output conventions").

## 3. Output conventions

All output goes through the `&Out` the command receives (`src/output.rs`, #364); the crate
denies `println!`/`eprintln!`. `outln!(out, …)?` writes a line to stdout, `errln!(out, …)` one
to stderr, `out.json(&value)?` a pretty document, `out.jsonl(&value)?` a compact line.

- stdout carries the result only: human text, or with `--json` exactly one JSON document
  (`out.json`), or with `--jsonl` one compact object per line for per-item streams
  (`transcribe`). Keep JSON keys snake_case and stable.
- stderr carries everything else: confirmations (`config set` prints `Set key = value`),
  warnings, progress, and `tracing` logs (the subscriber writes to stderr at `warn` by default).
  `scripts/verify-json-cli-streams.py` (`--json`, `--jsonl`, `--error`) checks this contract
  for `transcribe` in CI.
- Propagate stdout write errors with `?`. When the reader closes stdout (`… | head -1`) every
  later write fails fast and `run_with` ends the run quietly with exit 0. A stdout that is not
  open at all (`>&-`) also exits 0: the write fails with `EBADF`, which std swallows. Only a
  real I/O error (a full disk, `> /dev/full`) ends the run with
  `Error: could not write to stdout: …` and exit 1. Only a run whose own outcome is
  the closed write (or that succeeded) ends quietly: a command that failed for another reason
  keeps its failure and status. A check that owes a verdict (exit 3) writes its report through
  `output::ignore_closed(…)`, so a reader that left cannot turn a failed check into exit 0.
- A `--json`/`--jsonl` run that fails before writing anything to stdout gets one error object
  there, `{"status":"error","code":"unknown_profile","error":"Settings error: …"}`, pretty or
  compact like the command's own output (`exit::conclude`). `code` is `DictationError::code()`:
  the variant in snake_case, or the code of the typed error it wraps (a `SettingsError` keeps
  its own, `unknown_profile` above; `settings_error` is only the CLI's own wording of a bad
  flag), and `cancelled` / `check_failed` for those exits. A run that
  already wrote its report, or some `--jsonl` items, keeps stdout as it is. A single-file
  `transcribe --json` whose file fails writes that object with a `source` key.
- A command whose stdout is always JSON, with no `--json` flag (for example `latency-report`,
  `benchmark-dictation`, `diarization *`, `meeting proposal inspect`/`preview` and
  `transcribe --meeting-json`), stays out of `json_output` on purpose: when it fails before it
  has a report, stdout stays empty and only the `Error: …` line on stderr says why.
  `latency_budget_cli::missing_input_fails_without_echoing_private_path` requires this ("input
  errors must not emit JSON"). Only an explicit `--json`/`--jsonl` flag opts a run into the
  error object.
- Never write transcripts, audio or personal paths to logs
  ([ADR 0002](../adr/0002-privacy-first.md)).

## 4. Exit codes

Commands return `Result<(), DictationError>` or, to choose the status, `Result<(), CliError>`
(`src/exit.rs`); never call `std::process::exit`.

| Code | `exit::Exit` | When |
|---|---|---|
| 0 | `Success` | The command returned `Ok`. |
| 0 | `StdoutClosed` | The reader closed stdout (`… \| head -1`) and the command had no other failure; nothing is printed on stderr. |
| 1 | `Failure` | Any other `Err` (printed as `Error: …` on stderr), or stdout could not be written. `acceptance preflight` also uses exit 1 when its emitted report has blocked required checks. |
| 2 | `Usage` | clap usage errors; the headless binary without a subcommand. |
| 3 | `CheckFailed` | A check ran, wrote its report and did not pass (`CliError::CheckFailed`): `latency-report` budget, `engine doctor`, `benchmark-dictation` (`--max-warm-ms` or an `--expect-word`/empty-text check) and `diarization reference-qualify`. |
| 130 / 143 | `Cancelled` | Cancelled by SIGINT / SIGTERM (`cancel.rs`); `Cancelled` on stderr. |

Exit 3 means the report is already on stdout: a command that fails before it has a report
(bad input, a missing model, a `diarization evaluate` whose reference is not qualified) exits 1.
`acceptance preflight` is an intentional exit-1 exception: it emits its report, then exits 1
when required checks are blocked; malformed `--expect-revision` input is a usage error (2).
`run_with` has unit tests for these statuses (`lib.rs`, `exit.rs`); test a new command's
statuses the same way when it does not need a settings file, otherwise spawn the binary.

## 5. Tests

1. Unit tests in `example.rs` for the pure parts. Inject environment-dependent inputs
   (see `Probe` in `engine.rs`) instead of reading the machine. Pass `&Out::capture().0` where
   a function writes, or keep the `Captured` half to assert on stdout and stderr.
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

If the command is cheap and needs no model, add it to the `basic` tier of
`scripts/ci/smoke-cli.mjs`: one cross-platform script that `build-macos` and `build-windows`
in `.github/workflows/ci.yml` both run against the release binary (`node scripts/ci/smoke-cli.mjs
basic --binary <path>` runs it locally). `scripts/test-ci-smoke-cli.mjs` lists the commands each
tier runs, so extend it with the new one. The `scripts/test-ci-*.mjs` tests read the workflows as
parsed YAML; run `npm run test:frontend` after editing a workflow (tests in both the `ci` and
`release` groups read them).

## 7. Docs

`sagascript --help` is the inventory; do not list commands in AGENTS.md. Add a README "CLI
usage" example if users need it, and a GUI path if the feature is user-facing
([ADR 0001](../adr/0001-cli-first.md)). Then run `npm run verify`
([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)).
