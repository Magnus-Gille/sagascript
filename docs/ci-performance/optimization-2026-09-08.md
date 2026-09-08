# CI optimization experiment — September 8, 2026

Tracking: [issue #178](https://github.com/Magnus-Gille/sagascript/issues/178),
[PR #226](https://github.com/Magnus-Gille/sagascript/pull/226).

## Baseline and changes

The archived [baseline](baseline-2026-09-08.json) contains 39 successful CI runs
(median 618 seconds) and 20 successful Windows candidate runs (median 1,864
seconds). These mixed cohorts include different cache conditions and revisions;
they are context, not a matched control for attributing a percentage improvement.

The nearest main revision is `b661415f6db99e64bad95cfdd766e7c359733b1a`.
[CI run 34216174578](https://github.com/Magnus-Gille/sagascript/actions/runs/34216174578)
took 351 seconds on macOS and 658 seconds on Windows. Native build steps took
135 and 308 seconds respectively (Cargo itself reported 132 and 301 seconds).
The macOS native step rebuilt the three workspace crates. Windows additionally
rebuilt `ort-sys` and `ort`; a cache hit does not imply no compilation.

The proposed changes preserve the existing native, inference, architecture,
unsigned-installer, and source-identity gates:

- Retain source-line backtraces while reducing development/test debug information
  to `line-tables-only`. Release optimization remains unchanged by this setting.
- Reuse the frontend built earlier in each job. Validate its title and content
  hash against build metadata before Tauri embeds it; fail on missing or changed
  output. Normal local frontend builds still regenerate output and metadata.
- Remove three redundant macOS CLI diarization commands. CLI default features
  already include diarization; default CLI tests/all-target Clippy and the
  separate core feature matrix remain. Those commands cost 14 seconds in the
  nearest main run (approximately 15 seconds in the broader baseline).
- Cache Windows candidate model files, retaining manifest verification and both
  inference gates. Earlier downloads took only 3–5 seconds, so this is a small
  improvement rather than the main performance opportunity.
- Run macOS and Windows validation alongside native release builds. Separate
  caches keep development/test dependencies apart from release dependencies.
  Final jobs retain the existing `check-macos` and `check-windows` status names
  and require both lanes to succeed, including failure/cancellation/skip cases.
  This trades duplicated runner setup for a shorter critical path; measure total
  runner time as well as elapsed time.
- Add offline cohort reporting and Cargo fingerprint diagnostics for measuring
  subsequent changes and explaining rebuilds.

Local frontend timing was approximately 0.58 seconds for Vite versus 0.03 seconds
for reuse validation. This measures one local invocation, not total CI latency.

## Verification and limitations

The initial unsplit native CI run at `15113c01b8a626c2f59c06c939a4f17a27459a19`
passed all three platforms. New debug-profile cache fingerprints required cold
builds: macOS took 1,185 seconds, Windows 1,643 seconds, and Linux 503 seconds.
These are not comparable to the warm baseline and are not claimed as gains.
The parallel split introduces separate cache namespaces and also requires an
initial population before warm measurements. Its first attempt at
`cebb0b13c4288294afae80b18cae941cc9d7547e` passed both native build jobs, but
validation failed because a Python workflow fixture still expected the old job
names. The final aggregation jobs correctly failed too. The fixture now checks
the three validation jobs and retains the Python-version and ordering assertions.
Local verification after this correction passed all 127 Python tests and 185
frontend tests (one additional test is skipped), plus workflow actionlint.
The failed workflow is excluded from successful latency comparisons.

The first Windows candidate attempt at `15113c01b8a626c2f59c06c939a4f17a27459a19`
failed before native compilation: a newly added test expected LF but the Windows
workflow checkout used CRLF. The correction parameterizes the test for both line
endings without weakening its feature assertions. Failed attempts remain part
of the experiment record; their short durations are not speed improvements.

A separate code-generation parallelism experiment compares release builds with
one and sixteen codegen units while retaining optimization level 3 and thin LTO.
An initial offline attempt failed to link ONNX Runtime and is excluded from all
performance comparisons. Successful local CLI builds took 75.76 seconds with one
codegen unit and 90.16 seconds with sixteen. Sixteen units were 19% slower for this CLI build. A controlled app experiment,
with workspace crates cleaned and dependency artifacts retained between builds,
used order 1 → 16 → 16 → 1: 95.03, 73.56, 75.86, and 108.22 seconds. The means
were 101.63 versus 74.71 seconds, a 26.5% local improvement, but the app binary
grew from 43,777,296 to 49,891,488 bytes (14%). These trials used the same older
source revision, not the final CI patch; runtime parity was not benchmarked.
The mixed CLI/app results and binary growth do not justify changing the global
release setting, which remains one codegen unit.

The larger acceptance cohorts in #178 remain necessary before claiming stable
median/tail targets or closing the issue. Signed macOS and production release
workflows have not been dispatched for this experiment. Native run results and
subsequent attempt comparisons are recorded in PR #226. Compare workflow elapsed
time across the split, not old versus new `check-macos`/`check-windows` job
durations: those names become short aggregation jobs. Compare native build steps
against the new `build-macos`/`build-windows` lanes explicitly.

The initial independent review used Claude Sonnet 5 in read-only mode. Its only
medium concern was missing context for the signed macOS frontend-build ordering:
source inspection confirmed `Build fresh frontend` precedes the Tauri reuse step.
The review found no confirmed defect. A separate Claude Sonnet 5 review of the parallel-job change found no blockers
after deterministic checks passed. It confirmed gate parity and fail-closed
aggregation, and highlighted duplicated setup and loss of sequential fail-fast
as runner-cost tradeoffs. The conductor also added explicit assertions that
macOS checks remain blocking and only the two existing Windows smoke exceptions
remain nonblocking.

## Cached comparison and merge review

The nearest main control, [34216174578 attempt 1](https://github.com/Magnus-Gille/sagascript/actions/runs/34216174578/attempts/1),
used `b661415f6db99e64bad95cfdd766e7c359733b1a`. The corrected cached PR repeat,
[34224809046 attempt 2](https://github.com/Magnus-Gille/sagascript/actions/runs/34224809046/attempts/2),
used `fcd1599f840f4005c97416c076538d8ad71eaaf5` and passed all seven jobs.

| Metric | Main control | Cached PR repeat |
|---|---:|---:|
| First job start to last job end | 10m58s | 8m01s |
| Summed runner time | 18m55s | 23m24s |
| macOS native build step | 2m15s | 3m12s |
| Windows native build step | 5m08s | 5m39s |

This is **26.9% less waiting** and **23.7% more runner time**. Native compilation
itself did not get faster. Summed runner time is not billed cost. This single
cached comparison is not a stable percentile result: triggers differ (main push
versus PR rerun), although application Rust source and these three runner images
were unchanged. The corrected first attempt passed in 15m31s with cold validation
caches. Issue #178 remains open for faster compilation, packaging, lower runner
time, and the larger acceptance cohorts.

The [unsigned candidate repeat](https://github.com/Magnus-Gille/sagascript/actions/runs/34224809050)
passed x64 in 18m51s, versus 16m48s at a different older application revision;
this does not establish a packaging gain. ARM64 failed after the image changed
from `20260830.155.1` (Clang 20.1.6) to `20260906.161.1` (Clang 22.1.8).
Both ARM attempts missed their image-scoped Rust cache. The new native probe
selected SVE, which makes pinned `whisper-rs-sys` 0.14.1 include Linux-only
`sys/prctl.h` on Windows. The preceding cold candidate
[34221630681](https://github.com/Magnus-Gille/sagascript/actions/runs/34221630681)
passed both architectures. The image compatibility failure is not a timing sample.

The merge-review correction adds a Windows ARM64-only CMake hook that forces
only the pinned SVE/SME positive probe caches off. It preserves native tuning and
dotprod/i8mm probes, and lets upstream test the corresponding disabling flags.
The hook has Windows/ARM64/pointer-size guards and participates in the native
cache key. Regression tests exercise the actual CMake source-run check module
and reject wrong targets. Dependency updates must re-audit these internal probe
names. The native candidate run must confirm `+nosve+nosme` in generated flags,
both architecture packages, and the existing transcription gates before merge;
its result is recorded in PR #226. This compatibility correction is not claimed
as a compilation or inference speed improvement.

Merging this PR activates the main-push CI workflow. Windows candidates remain
PR/manual-triggered, signed macOS test builds remain manual, and application
releases remain tag-triggered. No application release is needed for these CI
changes. Reverting the PR restores the prior workflow and tooling behavior.

## Delegated work

The conductor integrates changes, checks results, and owns publication and the
final performance assessment. Bounded tasks used native Luna agents:

| Task | Model / effort | Checks | Usefulness |
|---|---|---|---|
| Historical timing and log analysis | gpt-5.6-luna / high | Raw run/step arithmetic cross-checked | pass |
| Frontend reuse implementation | gpt-5.6-luna / high | Focused tests, full frontend suite, build and hash validation | partial: schema corrected and metadata test strengthened during integration |
| Model cache implementation | gpt-5.6-luna / high | Cache contract and Windows workflow tests, actionlint | pass |
| CLI duplicate-gate removal | gpt-5.6-luna / xhigh | Feature coverage test, actionlint | partial: Windows line-ending failure corrected after native CI |
| LF/CRLF regression correction | gpt-5.6-luna / high | Both line-ending fixtures pass; conductor reran | pass |
| Parallel validation/native lanes | gpt-5.6-luna / high | Gate parity, actionlint, frontend suite; conductor expanded all 16 aggregate cases and retained smoke failure policy | partial: missed Python workflow fixture corrected after CI |
| Python workflow fixture correction | gpt-5.6-luna / high | Red/green regression, 127 Python tests and frontend suite rerun by conductor | pass |
| Release code-generation experiment | gpt-5.6-luna / xhigh | Paired CLI builds and four controlled app builds; logs and sizes inspected by conductor | pass: measured tradeoff, setting retained |
| Follow-up rebuild diagnosis | gpt-5.6-luna / xhigh | Historical Cargo logs and source inspection | partial: stale worktree context corrected; exact invalidation cause remains unproven |
| Merge review: gate parity and frontend reuse | gpt-5.6-luna / high | Focused tests plus conductor inspection; fresh independent Claude Sonnet 5 review | pass |
| ARM64 image diagnosis | gpt-5.6-luna / xhigh | Pinned CMake/header source and old/new compiler logs checked | pass |
| ARM64 regression tests | gpt-5.6-luna / high | Red/green CMake tests; conductor corrected native flag name and strengthened diagnostics/module coverage | partial |
| Cleanup inventory and evidence archive | gpt-5.6-luna / high | Clean status, patch-ID equivalence, source/copy SHA-256 checks repeated by conductor | pass |
