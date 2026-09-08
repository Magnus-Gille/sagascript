# Comparing CI performance

Issue [#178](https://github.com/Magnus-Gille/sagascript/issues/178) targets a large
reduction in CI and build waiting. Measurement can proceed alongside application
implementation: the reporting commands below read saved JSON and do not start
builds, download models, change caches, or modify workflows.

## Capture and report

Use Node.js and the existing authenticated GitHub CLI. Save explicit run attempts
so later reruns cannot silently change the evidence. For example, these commands
read two existing runs; they do not dispatch or retry them:

```sh
gh run view 34204997034 --attempt 1 --repo Magnus-Gille/sagascript \
  --json databaseId,headSha,workflowName,event,attempt,status,conclusion,createdAt,jobs \
  > /tmp/sagascript-ci-run-a.json
gh run view 34201979049 --attempt 1 --repo Magnus-Gille/sagascript \
  --json databaseId,headSha,workflowName,event,attempt,status,conclusion,createdAt,jobs \
  > /tmp/sagascript-ci-run-b.json
jq -s '.' /tmp/sagascript-ci-run-a.json /tmp/sagascript-ci-run-b.json \
  > /tmp/sagascript-ci-sample.json
node scripts/ci-cohort-timings.mjs < /tmp/sagascript-ci-sample.json \
  > /tmp/sagascript-ci-report.json
```

Select the full comparison window before collecting runs. Include failures,
cancellations, and in-progress runs, not just the fastest green runs. Archive the
input JSON together with the report. Repeat the same selection procedure after
each optimization, retaining the commit SHA and run/attempt identity.

The existing single-run command remains available:

```sh
gh run view 34204997034 --attempt 1 --repo Magnus-Gille/sagascript \
  --json databaseId,headSha,jobs | node scripts/ci-run-timings.mjs
```

## Meaning of the measurements

- **Elapsed:** run creation to the final job completion, including initial wait,
  gaps between dependent jobs, and post-job cleanup.
- **Initial wait:** run creation to the first job start. This does not measure
  every job's scheduling delay.
- **Wall:** first job start to final job completion; the existing single-run
  reporter uses this definition.
- **Runner:** sum of job durations, including jobs that execute in parallel.
  This is raw time, not billed cost.
- **p50 / p90:** median / nearest-rank 90th percentile, in seconds. Reports retain
  sample size and range. Small samples are descriptive, not evidence of stable
  tail performance.

Cohorts separate workflow, event, and attempt. Only completed successful runs
with complete required timing data contribute latency samples. Missing data is
excluded explicitly, not converted to zero. Failed/cancelled outcomes remain
counted. Job statistics use successful jobs; step statistics use successful steps
within those jobs and retain the job name, step number, and step name so matrix
architectures are not pooled. Job and step medians are independent distributions:
adding them does not reconstruct the median workflow's critical path.

Run creation precedes later retries, so elapsed time for attempt 2+ includes the
intervening delay. Keep those cohorts separate; do not describe their elapsed
metric as the retry's execution duration. Use wall/runner time for that question.

The reporter cannot establish compiler cache hits, runner image changes, toolchain
versions, or a split between compilation, linking, downloads, and notarization
inside one step. Record that evidence separately. Do not rename a combined
build/sign/notarize step as pure compilation or treat a short cache step as proof
of a hit. Compare matched runner/architecture, workflow revision, feature/profile,
and warm/cold conditions before attributing a speedup to a change.

## September 8 reference

[The baseline JSON](baseline-2026-09-08.json) preserves the earlier issue snapshot's
workflow statistics, successful run IDs/SHAs, per-run job durations, and outcome
counts. It is an archival summary, not the full raw input to the new CLI. Its
mixed-event/mixed-attempt values should not be equated with individual cohorts.
Full job/step metadata can be read from the linked GitHub run IDs while retained;
the raw capture used for local validation lives outside the repository.

| Workflow | Successes | Median | p90 | Median target |
|---|---:|---:|---:|---:|
| CI | 39 | 10m18s | 12m09s | 5m |
| Windows Package Candidate | 20 | 31m04s | 36m11s | 15m |
| Signed macOS Test Build | 9 | 8m19s | 10m16s | 5m |
| Release | 1 | 17m47s observed | insufficient sample | 10m provisional |

These are observations from September 6–8, 2026, after earlier caching work.
They do not prove a warm-cache baseline. The full acceptance criteria remain in
#178. Implementation of this reporter alone does not improve build speed or
satisfy those criteria.

## Local checks

```sh
node --test scripts/test-ci-run-timings.mjs scripts/test-ci-cohort-timings.mjs
```

The existing `scripts/run-frontend-tests.mjs` discovers the new test file
automatically. No package manifest, dependency, CI trigger, signing, publication,
or application changes are required for the reporting tool.
