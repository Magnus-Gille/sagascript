# Code-health exploration (#331)

Date: 2026-10-09
Repository: `Magnus-Gille/sagascript`
Frozen base: `24cbf972ddd92ca488331d0d65ee1e313dd87d95`
Scope: Rust/Tauri, Svelte/TypeScript, Swift CoreML host, and a bounded CI
first-attempt reliability check.

This is a bounded research result. It changes no runtime code, CI gate,
version, release, signing, deployment, or external state. It uses no private
recordings, transcripts, model files, credentials, or `.env` contents.

## Decision

Do not make any analyzer a required check from this trial. The pinned tools are
useful as report-only analysis, especially around the largest orchestration
functions, recent frontend churn, and the CoreML host protocol. Coverage is
feature/platform-specific, Knip needs explicit entrypoint configuration, and
the Rust complexity tool carries a yanked transitive dependency.

The strongest measured simplification candidates are the 726-SLOC
`transcribe_file` function (cyclomatic 94, cognitive 85), the 31-commit
`Settings.svelte` module (543 AST branches and 65 template blocks), and the
low-coverage CoreML tensor/inference paths. These are research priorities, not
approved refactors.

## Trials and evidence

| Surface | Trial | Result | Interpretation |
| --- | --- | --- | --- |
| Rust coverage | `cargo llvm-cov 0.9.1 -p sagascript-core --no-default-features --lib --tests --json --summary-only` | 612 tests pass; 89.56% lines, 85.77% functions, 89.96% regions; 81.21s wall including compile | `diarization_evaluation.rs` 98.80% lines, `download.rs` 95.92%, `whisper_backend.rs` 66.36%. |
| Rust CLI coverage | `cargo llvm-cov 0.9.1 -p sagascript-cli --no-default-features --lib --tests --no-clean` | Correction pass: 235 tests pass; observed mixed-attempt profile: 71.46% lines, 70.60% functions, 72.29% regions; 8.09s wall; `diarization.rs` 85.48%, `transcribe.rs` 54.69% lines | Baseline failed four engine tests because the isolated target lacked the existing fake host. Prebuilding that fixture corrected execution, but `--no-clean` retained profiles from both attempts. This percentage is NOT qualified as a clean baseline. Core and CLI targets were separate. |
| Rust complexity | `rust-code-analysis-cli 0.0.25 -m -F -O json` | Function-level cyclomatic/cognitive metrics for five files; top finding is `transcribe_file` cyclomatic 94/cognitive 85 | Useful for prioritization. Its locked `crossbeam-channel 0.5.6` dependency is yanked; no lockfile mutation was made. |
| Rust selected tests | `CARGO_BUILD_JOBS=2 cargo test -p sagascript-core --no-default-features` | 612 passed, 0 failed, 1 ignored; 3 integration binaries 58 passed | The no-default core path passes. Default features, Tauri GUI, Windows/Linux paths, and native CoreML inference remain outside this lane. |
| Svelte/TypeScript AST | `svelte/compiler 5.56.4` and TypeScript 5.9.3 | Selected Svelte and TypeScript files parse successfully; `Settings.svelte` has 543 AST branches and 65 template blocks | AST counts are prioritization signals, not defect proof. |
| Unused candidates | Knip 5.46.0, JSON reporter | Reports `@tauri-apps/plugin-autostart`, `tslib`, and many exports/files | Several are entrypoint/configuration false positives. Labels and validation are in `findings.json`; no deletion is authorized. |
| Swift host build | `./scripts/build-engine-host.sh` | Pass; 7.88s wall, 6.29s SwiftPM build; generated ignored `BuildInfo.swift` | Existing prerequisite for the package’s build-identity source. |
| Swift host coverage | `swift test --enable-code-coverage --package-path src-tauri/engine-host/coreml --jobs 2` | 27 passed; source-only 48.61% lines, 58.14% functions, 48.39% regions; 16.63s wall (unfiltered test-bundle totals: 59.62% lines) | `ProtocolServer.swift` 88.33% lines and `ContextBiasing.swift` 91.48%; `CoreMLEngine.swift` 1.27% and `TdtDecoder.swift` 18.33% because no model-backed/private input was used. |

The local timings are wall-clock trial measurements, not CI runner promises.
Rust used two build jobs and disposable target directories. The first restricted
Rust run had 18 temporary-fixture permission failures; its correction outside
the restricted sandbox passed. The Swift package initially failed at the
missing generated `BuildInfo.swift` prerequisite, then passed after the
existing build script generated it.

The reproducible collector is [`scripts/code-health-exploration.mjs`](../../../scripts/code-health-exploration.mjs).
It excludes `.git`, `node_modules`, Cargo `target`, Swift `.build`, staged
build output, credentials, environment files, and private inputs. Run:

```sh
node scripts/code-health-exploration.mjs
```

It recomputes parser inventory and churn and assembles [`evidence.json`](evidence.json) with explicitly archived measurements in `measured-baseline.json`. It does NOT rerun Rust/Swift analyzers; commands below describe those trials. The script refuses changed tracked application source relative to the frozen revision.
Raw analyzer output stays outside the repository; only sanitized paths, counts,
metrics, and verdicts are retained.

## Coverage and platform limits

The selected Rust modules are `sagascript-cli/src/transcribe.rs`,
`sagascript-core/src/transcription/whisper_backend.rs`,
`sagascript-cli/src/diarization.rs`,
`sagascript-core/src/diarization_evaluation.rs`, and
`sagascript-core/src/download.rs`. The two no-default-feature coverage runs
measure executable paths but do not cover the full workspace feature matrix,
Tauri GUI, Windows/Linux platform branches, or native model inference.

The frontend AST pass covers `Settings.svelte`, `FileTranscription.svelte`,
`MeetingReview.svelte`, `MeetingReprocessing.svelte`, `Onboarding.svelte`,
and TypeScript modules. It does not claim Svelte type-checking or browser
coverage. Knip’s default graph treats many test/helper exports as unused;
those results require explicit entrypoint configuration before action.

The Swift target is the macOS 14+ CoreML host. Its tests exercise protocol,
tensor, cache, cancellation, and context-biasing logic with local fixtures.
They do not prove Neural Engine behavior, model accuracy, or cross-platform
support. The low CoreML inference coverage is an explicit unsupported branch,
not a quality claim.

## Findings and sprint candidates

[`findings.json`](findings.json) contains up to ten manually reviewed
candidates per surface, with `useful`, `false_positive`, or `unknown` labels.
The useful candidates are tied to measured metrics:

- Extract pure input/model normalization from `transcribe_file` and `run_inner`
  while preserving the CLI contract and using focused CLI regression tests; obtain a clean coverage run before adopting a numerical baseline.
- Split `Settings.svelte` state by profile/model/permission boundaries after an
  effect map, using its 31-commit churn and AST branch count to choose seams.
- Add deterministic fixture coverage for CoreML tensor conversion and load
  failure paths, then reassess `TdtDecoder.swift` and `CoreMLEngine.swift`.

No dead-code candidate was deleted or treated as proof of unused code.

The collector records a recent per-path churn window, 2026-09-09 through
2026-10-10, rather than a single commit total. The largest relevant paths are
`Settings.svelte` (31 commits, +1,912/-1,240), CLI `transcribe.rs` (14,
+1,513/-294), and `download.rs` (2, +1,341/-71). Churn prioritizes review; it
does not establish defect risk.

## CI and merge constraints

The current workflow remains authoritative: `pull_request` to `main` and
`push` to `main`, with the dependency-free `scope` classifier first. For a
non-docs change, the required aggregators are `check-macos` and
`check-windows`; `check-linux` is also guarded by the workflow. Docs-only
classification may skip native lanes, but the aggregators assert the expected
skipped state. Concurrency and timing behavior from #178 remain unchanged.

The retained read-only `receipts/ci-runs.json` sample covers the latest 20 main push runs of ci.yml at collection time: latest status is green for all 20, but run 37291285757 failed on attempt 1 before passing a retry. Thus first-attempt success is 19/20 for this bounded sample. Pending/cancelled/other outcomes must remain separate, retries are resolved via the run-attempt API, and PR/push cohorts are never pooled. This is a small starting sample, not an established reliability trend.

Both existing tools were actually reused: `ci-run-timings.mjs` on preserved `receipts/ci-timing-input.json`, and `ci-cohort-timings.mjs` on a one-element array containing the same record. Their sanitized outputs are retained. Failed runs remain in the cohort ledger and cannot count as successful-latency evidence. Runtime optimization stays in #178.

Confirmed release regressions require a published tag/release plus an issue explicitly confirmed as introduced in that release. Neither an ordinary bug label nor failed CI proves a post-release regression. The historical confirmed-regression count remains unknown; future manual links should include affected release, regression issue, confirmation date and resolution.

## Reproduction configuration

[`config.json`](config.json) records exact target paths, commands, versions,
parallelism, and exclusions. All generated data is sanitized JSON containing
repository paths and counts only.

## Denominators and decisions

`receipts/*-coverage.json` retains covered/count/percent per emitted file and metric, plus original-artifact hashes and byte sizes. Rust core counts 20,666/23,074 emitted lines across 41 files, including inline test code; it is NOT a production-only percentage. Feature-disabled/non-emitted modules are absent, not zero. CLI has 6,558/9,177 observed lines across 17 files, but retained failed-attempt profiles disqualify that run as a clean baseline. Swift source-only is 924/1,901 lines across 8 exported `Sources/EngineHostCore` files; the 2,454-line unfiltered denominator also contains tests/generated code. SHA256.swift is present with 0% lines. Neither LLVM export emitted branch records: branch coverage is unsupported in this trial, not 0%. Frontend execution coverage was not measured. Do not average line, region and branch coverage or combine languages.

Adopt the clean Rust-core and source-filtered Swift methods for report-only pilots, always displaying their source/platform/test scope. Defer CLI numerical adoption until a clean profile run; this bounded exploration does not conceal the failed setup by deleting its evidence. Adopt function-level Rust complexity for triage with maintenance review of its old dependency lock. Adjust Svelte AST counts to a parser-backed per-function definition before using a trend; these counts mix branch constructs and are not cyclomatic complexity. Defer dead-code totals until GUI/CLI/test/native entrypoints are validated. No Rust/Swift unused-code conclusion was established across platform/feature combinations.

Begin with weekly collection and a matched before/after snapshot around a simplification sprint. Measured times are local wall time, not extra hosted-runner measurements; no paid runner or release was dispatched for this exploration. Raw LLVM JSON sizes and retained per-file metrics are in the receipts; the complete committed research bundle can be measured with `du -sk docs/research/code-health-2026-10-09`.

Install task-only pinned Rust analyzers with `cargo install --locked --root <task-tools> cargo-llvm-cov --version 0.9.1 --jobs 2` and `cargo install --locked --root <task-tools> rust-code-analysis-cli --version 0.0.25 --jobs 2`; use the matching `llvm-tools-preview` component and `npm ci --ignore-scripts` from the source lock. Run Rust commands from `src-tauri/` with separate disposable `CARGO_TARGET_DIR`s. To reproduce the CLI setup correction, build `cargo build -p sagascript-core --no-default-features --bin sagascript-fake-engine-host` into its own target before the CLI test run; do not mix retained profiles with a clean-run claim. SwiftPM exports JSON under its `codecov` output directory after the recorded coverage command; use the report's source filter before aggregating. Knip 5.46.0 is exploratory only; it is installed in a task-only npm tree and invoked against this repo, not added to runtime dependencies.

Audit: frozen source SHA, tool/config manifest, retained per-file receipts, deterministic verification and PR checks. Reversal: a normal reviewed revert removing this research directory and its isolated assembler script. No runtime, installed skill or CI gate consumes them.

Verify archived arithmetic with `python3 docs/research/code-health-2026-10-09/verify.py`. Reproduce the CI run example with `node scripts/ci-run-timings.mjs < docs/research/code-health-2026-10-09/receipts/ci-timing-input.json`; wrap that same object in a JSON array and pass it to `node scripts/ci-cohort-timings.mjs` for the cohort example. Use the artifact-bearing PR revision (where the research scripts exist), while requiring the tracked application paths to match the frozen source revision; checking out the older source SHA alone does not include these artifacts.
