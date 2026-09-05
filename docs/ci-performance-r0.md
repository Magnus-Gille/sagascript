# CI performance R0 baseline (#178)

Measured 2026-09-05 from the read-only GitHub Actions job JSON for the
successful runs below. Job elapsed time is `completedAt - startedAt` using the
whole-second timestamps returned by GitHub. CI wall time is the earliest job
`startedAt` to the latest job `completedAt`; it includes post-job cleanup that
is represented in `completedAt`, and excludes queue time before the first job
starts. No workflow was dispatched and no cache was cleared.

## Current PR #197 observation

The current CI and Windows candidate runs are on the exact revision
`90300ac5a654b2011a82b9b935422e3c1b629124`.

| Lane | Run/job | Elapsed | Critical path evidence |
| --- | --- | ---: | --- |
| PR CI wall | [33982933041](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933041) | 8m 12s | Linux 1m 26s, macOS 4m 18s, Windows 8m 12s |
| PR CI — Windows | [job 101351279721](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933041/job/101351279721) | 8m 12s | native release 4m 16s; model-backed smokes 27s total |
| PR CI — macOS | [job 101351279937](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933041/job/101351279937) | 4m 18s | native release 2m 00s; model-backed smokes 15s total |
| PR CI — Linux | [job 101351280041](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933041/job/101351280041) | 1m 26s | compile/test/lint checks only |
| Windows package — x64 | [job 101351279993](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933148/job/101351279993) | 20m 20s | Rust workspace 6m 30s; real-transcription gate 4m 08s; unsigned installers 8m 03s |
| Windows package — arm64 | [job 101351280210](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933148/job/101351280210) | 17m 00s* | Rust workspace 5m 27s; real-transcription gate 2m 42s; unsigned installers 6m 39s |
| Signed macOS test | [job 101354351246](https://github.com/Magnus-Gille/sagascript/actions/runs/33984088875/job/101354351246) | 4m 36s | sign/notarize/staple app 3m 19s |

Run links: [Windows package 33982933148](https://github.com/Magnus-Gille/sagascript/actions/runs/33982933148),
[signed test 33984088875](https://github.com/Magnus-Gille/sagascript/actions/runs/33984088875).

The signed-test run is a separate exact revision,
`62dadba7c3ba780867d8fede441990bb35abc407`, and is included as the available
signing-lane baseline rather than as a same-SHA PR comparison.

\* The supplied run summary called this 16m 59s. The API timestamps are
`18:05:00Z` to `18:22:00Z`, i.e. 1,020 seconds (17m 00s); the one-second
difference is retained here rather than silently inventing sub-second timing.

The current Windows x64 package critical path is therefore installer creation
(483s), followed by the Rust workspace gate (390s) and real-transcription gate
(248s). The named steps total 1,121s; setup, frontend, and inter-step overhead
account for the remaining 99s of the 1,220s job span.

The timing reporter returns `wallSeconds=492` and `runnerSeconds=836` for the
current CI run. The earlier 8m 15s figure used the top-level run start and
therefore included three seconds before the first job; it is not the table's
job-span metric.

## Prior comparison observations

These are directional comparisons, not a controlled A/B test: the runs have
different head SHAs and may have different runner/cache state.

| Lane | Exact revision | Run/job | Elapsed |
| --- | --- | --- | ---: |
| PR CI wall | `28e87c20fb2feeb39e1ce77f2ef7a6982b06d887` | [33980787222](https://github.com/Magnus-Gille/sagascript/actions/runs/33980787222) | 8m 22s |
| Prior Windows package — x64 | `28e87c20fb2feeb39e1ce77f2ef7a6982b06d887` | [job 101345501742](https://github.com/Magnus-Gille/sagascript/actions/runs/33980787220/job/101345501742) | 22m 05s |
| Prior Windows package — arm64 | `28e87c20fb2feeb39e1ce77f2ef7a6982b06d887` | [job 101345501654](https://github.com/Magnus-Gille/sagascript/actions/runs/33980787220/job/101345501654) | 18m 03s |
| Prior signed macOS test | `2ce5e923dfd09fa01ae1fd66005fb22900e06b9a` | [job 101348797780](https://github.com/Magnus-Gille/sagascript/actions/runs/33982013412/job/101348797780) | 4m 44s |

Supporting run links: [Windows package 33980787220](https://github.com/Magnus-Gille/sagascript/actions/runs/33980787220),
[signed test 33982013412](https://github.com/Magnus-Gille/sagascript/actions/runs/33982013412).

Against those observations, current x64 is 105s (7.9%) faster than prior x64,
current arm64 is 63s (5.8%) faster than prior arm64, and the signed test is 8s
(2.8%) faster overall. These changes are not attributable to caching without
same-SHA, same-runner, cache-hit evidence. On the same reporter metric, CI wall
time is 10s (2.0%) faster, with runner-seconds falling from 1,035 to 836
(19.2%); these are also directional because the revisions differ.

## Observed cache configuration and limits

The job JSON exposes cache step names and durations, but not the cache-hit
outputs. The following is the workflow configuration that produced each
baseline run; it is distinct from the uncommitted workflow changes in the
working tree:

- The baseline CI run used floating `Swatinem/rust-cache@v2`, plus the macOS
  model cache. Its actual hit/miss state is not represented in this JSON, so
  the run is not a fully cold or fully warm measurement.
- The baseline Windows package run used `actions/cache` for the Cargo registry
  only. The native `target` directory was not cached by that workflow, so the
  native target was uncached by design; registry hit/miss state remains
  unknown. It must not be described as a fully cold run because a registry
  cache could still have been restored.
- The baseline signed macOS test used raw `actions/cache` over `src-tauri/target`
  keyed by `Cargo.lock`. A target hit or miss is not represented in this JSON,
  so its 4m 36s result is not a cold signing baseline.
- No true fully-cold datapoint is available in the specified runs. The current
  timings are “observed run; actual cache hit/miss unknown,” with the Windows
  native-target scope known from the workflow configuration.

The proposed workflow changes must record a first-miss run before a warm rerun
on the same revision, preserving all required checks and recording the actual
cache hit/miss result. Future timing reports can be reproduced with:

```sh
gh run view <RUN_ID> --repo Magnus-Gille/sagascript --json databaseId,headSha,jobs | node scripts/ci-run-timings.mjs
```

Stable-toolchain comparisons assume the runner labels and workflow toolchain
inputs used here: `macos-14`, `windows-latest`, `windows-11-arm`,
`dtolnay/rust-toolchain@stable`, and Node.js 20. The Rust-cache action revision
must be recorded for each future run; the proposed pinned revision is not a
property of these baseline measurements. Runner image/toolchain drift must be
noted with the measurement.

## R0 acceptance targets

- Keep all current required CI, Windows x64/arm64 candidate, transcription
  gate, and signed-test checks green.
- Warm Windows x64 final package: target **≤12m 00s**. The fallback target of
  at least 30% reduction from 20m 20s is **≤14m 14s** (`1,220 × 0.70 = 854s`).
- Warm signed macOS test: target **≤6m 00s**, with no material regression from
  the observed 4m 36s baseline.
- Record the first cache miss before the warm rerun; report both cache states,
  exact SHA, runner image, and run/job links.

## Cache isolation decision

The Windows native cache includes architecture, `ImageOS`, `ImageVersion`, and
the workflow content hash in `shared-key`, not merely in a lockfile suffix.
Missing image identity fails the job before restoration. This preserves the
reason the old workflow disabled target caching: C/C++ toolchain changes must
not reuse incompatible native outputs. The pinned action also hashes the Rust
toolchain and C/C++ environment. Its only fallback retains the shared namespace
and environment hash; see the reviewed [key construction](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/config.ts)
and [restore implementation](https://github.com/Swatinem/rust-cache/blob/6323deb102c322ba6fcbdcafc7e3dddab59af2b6/src/restore.ts).

No gate is removed. PR package runs share a PR-number concurrency group; manual
runs use their unique run ID and do not cancel unrelated builds. The macOS test
cache remains separate from Windows and the stable release lane, and only
dependency artifacts are retained by the action's default policy.
