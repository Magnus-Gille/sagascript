# Informational code-health collection

`.github/workflows/code-health.yml` writes sanitized code-health v1 reports as GitHub Actions
artifacts. The workflow has no publishing, deployment, signing, release, or quality-gate step.
Existing CI and release workflows remain authoritative for application validation.

The collection modes are intentionally separate:

- Pull requests to `main` run the dependency-free synthetic conformance suite only. Static
  measurements are not collected per commit.
- A weekly Monday schedule runs Rust complexity, Rust and Swift coverage, and unqualified Knip
  candidate collection. Unsupported tools and scopes remain visible in each snapshot.
- A daily schedule reads the previous 28 days of first-attempt `CI` push runs on `main`. It skips
  static analyzers; static metric slots stay `unknown` with `not-collected`, not copied from a
  previous artifact or represented as zero.
- Manual dispatch runs static collection and is limited to `main`.

Every collector run requires a clean checkout, including no untracked files, and checks the
checked-out SHA, workspace path, repository identity, run ID, and attempt against GitHub's run
environment before collecting. Static GitHub reports use that run ID and attempt even though they
do not call the GitHub API; local reports use a local collection reference. Knip's
`--no-exit-code` keeps findings as report data while preserving nonzero exits for runtime errors.
The CI producer classifies provider `startup_failure` and `timed_out` conclusions as failures unless
separate evidence establishes an infrastructure cause; the original provider conclusion remains
in the timing evidence.

Each run uploads the `code-health-v1` artifact with 30-day retention, including setup or collection
failures. A setup failure retains a small `workflow-status.json` marker. A completed collection
contains the closed six-key manifest, evidence index, objective snapshots, scope registry, and
sanitized relative-path evidence. Raw analyzer reports and compiler output stay in runner
temporary storage. The static job installs the project graph from the checked-in npm lockfile with
lifecycle scripts disabled, then installs analyzers under `$RUNNER_TEMP`. Knip's isolated manifest
and lockfile live under `tooling/code-health/`. Exact versions are checked against
`docs/code-health-producer-v1.json` before measurements are accepted. Swift's compiler version is
recorded from the runner because Xcode supplies Swift.

The CI-only job receives only the run-scoped, read-only `actions` and `contents` permissions needed
to enumerate workflow runs and inspect commit history. The static PR path does not expose
`GH_TOKEN`; no production Heimdall or Munin credentials are configured in either job.

Disable collection by removing this standalone workflow or its triggers. This leaves the produced
artifacts available until their retention expires and does not change application checks or
release behavior. A skipped or missing run is a collection gap, not a health result.
