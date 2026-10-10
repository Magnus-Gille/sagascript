# Informational code-health collection

`.github/workflows/code-health.yml` writes sanitized code-health v1 reports as GitHub Actions
artifacts. The workflow has no publishing, deployment, signing, release, or quality-gate step.
Existing CI and release workflows remain authoritative for application validation.

The collection modes are intentionally separate:

- Pull requests to `main` run the dependency-free synthetic conformance suite only. Static
  measurements are not collected per commit.
- Schedules are opt-in through the repository Actions variable
  `CODE_HEALTH_COLLECTION_ENABLED=true`. When enabled, a weekly Monday schedule runs Rust
  complexity, Rust and Swift coverage, and unqualified Knip candidate collection. Unsupported tools
  and scopes remain visible in each snapshot.
- When enabled, a daily schedule reads the previous 28 days of first-attempt `CI` push runs on
  `main`. It skips static analyzers; static metric slots stay `unknown` with `not-collected`, not
  copied from a previous artifact or represented as zero.
- Manual dispatch runs static collection and is limited to `main`.

Every collector run requires a clean checkout, including no untracked files, and checks the
checked-out SHA, workspace path, repository identity, run ID, and attempt against GitHub's run
environment before collecting. Static GitHub reports use that run ID and attempt even though they
do not call the GitHub API; local reports use a local collection reference. Knip's
`--no-exit-code` keeps findings as report data while preserving nonzero exits for runtime errors.
The CI producer classifies provider `startup_failure` and `timed_out` conclusions as failures unless
separate evidence establishes an infrastructure cause; the original provider conclusion remains
in the timing evidence.

The daily CI metadata phase shares one monotonic 30-second deadline across run/job pagination and
historical workflow reads. Requests use only their remaining time, do not follow redirects, and fail
closed on 3xx responses. Raw workflow status and conclusion stay in the evidence inventory. A
completed failed run whose attempt-1 jobs leave the overall result unknown makes the CI metric
unavailable as `incomplete-input`; the producer does not infer a failed job or include that run in a
reliability denominator. Collector elapsed times and the workflow finalizer record phase and
pre-upload duration, compare metadata against 30 seconds and cold static collection against 900
seconds, and retain completed evidence when a duration exceeds its budget. Artifact upload has a
three-minute step limit; total duration through GitHub artifact receipt remains unknown.

Each run uploads the `code-health-v1` artifact with 30-day retention, including setup or collection
failures. A setup failure retains a small `workflow-status.json` marker. A completed collection
contains the closed six-key manifest, evidence index, objective snapshots, scope registry, and
sanitized relative-path evidence. Raw analyzer reports and compiler output stay in runner
temporary storage. The static job installs the project graph from the checked-in npm lockfile with
lifecycle scripts disabled, then installs analyzers under `$RUNNER_TEMP`. Knip's isolated manifest
and lockfile live under `tooling/code-health/`. Exact versions are checked against
`docs/code-health-producer-v1.json` before measurements are accepted. Swift's compiler version is
recorded from the runner because Xcode supplies Swift.

Swift coverage prepares a temporary copy of the Core ML package and the single tracked shared
`engine-host/test-vectors/context-biasing.json` fixture at the sibling path expected by its test
source. Coverage exports are filtered to that temporary package's `Sources/EngineHostCore`
directory before source-only parsing. Generated build identity, test bundle, intermediate, and
external toolchain files are excluded and recorded in `excluded_report_paths`; repository source
files that Swift does not emit remain explicit in `not_emitted_paths`. This keeps isolated
collector inputs complete without copying unrelated engine-host contents or counting generated
coverage.

The CI-only job receives only the run-scoped, read-only `actions` and `contents` permissions needed
to enumerate workflow runs and inspect commit history. The static PR path does not expose
`GH_TOKEN`; no production Heimdall or Munin credentials are configured in either job.

Disable scheduled collection by unsetting `CODE_HEALTH_COLLECTION_ENABLED` or setting it to
`false`; pull-request conformance and main-branch manual collection remain available. This leaves
the produced artifacts available until their retention expires and does not change application
checks or release behavior. A skipped or missing run is a collection gap, not a health result.
