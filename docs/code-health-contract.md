# Code health and agent changeability — v1

Status: shared contract for [Grimnir #211](https://github.com/Magnus-Gille/grimnir/issues/211).
This freezes the **1.0 pilot contract**, not a deployed collector or dashboard. It is advisory.
It does not change LearningTaskContract, Hugin Quality Receipts, model capability evidence,
operational health, or mutation authority. Those records may be correlated by reference only.

## Purpose and ownership

Use a small set of trends to choose and evaluate simplification/stability sprints. A trend
suggests where to investigate; it does not establish causation, defects, or an agent's ability.
There is no cross-language average, overall quality score, automatic cleanup, or policy gate.

| Fact / responsibility | Owner | Consumers and limits |
| --- | --- | --- |
| Contract, definitions, pilot policy and compatibility | Grimnir | Owning repos adopt through their tickets; this merge does not certify conformance there. |
| Objective repository snapshots, tool configuration, source inventory, exclusions and source evidence | Each product repository | Heimdall stores bounded snapshots and derives history; it cannot invent producer facts. |
| Observations during work and terminal changeability assessments | Agent skills / their harness adapter | The worker reports experienced evidence; a parent or Close preserves the worker and source IDs. |
| Native task identity, attempt lifecycle and known participation for Hugin tasks | Hugin | Adds its own facts only. It does not invent identity, participation, model or outcome for external sessions. |
| Subjective evidence persistence, idempotency, corrections and access control | Munin | Existing APIs where adequate; implementation must establish lifecycle/conformance before collection. Storage is not a scoring authority. |
| Derived trends, freshness, collection gaps and web presentation | Heimdall | Read-only views, separate objective and subjective panels; authoritative source links remain accessible to authorized users only. |
| Regression confirmation and candidate-code validation | Maintainer of the owning product repo | Explicit issue evidence; neither text matching nor an agent's guess is confirmation. |

No new generic telemetry service or transcript store. Product-owned artifacts/runs, Munin and
Heimdall remain the intended stores. A source may remain unavailable to a viewer; do not copy
its contents or broaden access to make a dashboard link work.

## Frozen shapes and validation boundary

- [`code-health-objective-v1.schema.json`](code-health-objective-v1.schema.json): one objective snapshot.
- [`code-health-agent-v1.schema.json`](code-health-agent-v1.schema.json): an observation or assessment.
- `scripts/lib/code-health-objective.mjs` and `scripts/lib/code-health-agent.mjs`: deterministic
  semantic conformance, including population arithmetic and reference/replay rules beyond JSON Schema.
- `scripts/lib/code-health-schema.mjs`: dependency-free implementation of the explicitly used
  JSON Schema subset. Unknown keywords fail validation; it is not a general JSON Schema library.
- Synthetic positive/adversarial fixtures under `tests/fixtures/code-health/`; run
  `make test-code-health` and the repository's `make test`.

The agent API is `validateRecord` for local shape checks, `validateRecords` for batch/reference
admission, and `aggregateRecords` with optional `contextRecords` and full-identity
`expectedAttempts`. The objective API is `validateObjective` and `validateObjectiveSeries`
(the latter validates a batch and correction context; its name does not authorize joining
incompatible time series). These are conformance/reference helpers, not store APIs. Incremental
context supplies authenticated source records, not invented placeholders. Consumers deduplicate
persistently by record ID and recompute views over the retained set; never sum overlapping batch
aggregates or count a transport retry as another observation. `replay_count` is batch-local;
`contextRecords` is reference-validation context, not a persistent admission ledger.

Passing the shape alone is insufficient: adopters must implement the semantic rules and pass the
same fixtures. No network, live stores or real task material is required to run conformance.
UTC timestamps have whole-second precision and must be real calendar instants. All objects are
closed; integers are nonnegative safe integers where applicable. Opaque references resolve in an
owner-controlled registry; their syntactic validity alone proves neither existence nor authority.

An objective snapshot is a fixed five-slot bundle bound to a repository and immutable source
commit. Each metric slot has a stable `slot_ref` registered by the producer/consumer with its
expected metric, scope, language/platform and freshness policy. Missing slots are derived from
that registry, never merely from the received set. Each slot also has its own observation time, definition/version, tool/version/config
digest, scope version/digest and inventory references, source run and attempt, language, platform
and features, plus an explicit unit. The envelope time is collection time and cannot precede any
slot. A nonmeasured slot may have null provenance or population if unavailable; measured slots
require both. Never invent a tool run for an unsupported or uncollected slot. Scope must say what was included and excluded:
generated code, tests, inline tests, native targets, feature-disabled modules and unimported source.
An eligible inventory cannot silently become the subset a tool happened to emit.

States are `measured`, `unknown`, `unsupported`, `failed`, and `stale`. Only `measured` carries
current numeric data. Other states have null data and a bounded reason; they may reference the
last historical snapshot. Zero is a measured result with a known population, never a missing value.
`scripts/lib/code-health-policy.mjs` supplies comparable series keys and freshness projection.
All current objective slots have a 10-day cap except CI's 48-hour cap; a clock before the source
observation yields unknown. The consumer computes staleness from its clock and pilot freshness caps, even if a source still
says measured. An expired historical point may be plotted with its original time but must not
appear as the current value. Missing expected slots must be rendered unknown, not omitted.

## Five bounded metric definitions

### 1. Complex functions

Count functions **strictly above** a declared threshold, alongside the eligible function population.
Bind the exact algorithm (for example cyclomatic or cognitive), parser/tool/configuration and
function inventory. Compare like with like. File totals, Svelte template branch counts, callback
nesting, and a route-registration aggregate are not interchangeable with function complexity.
Functions that cannot be parsed are explicit scope gaps, not easy functions.

Optional recent churn is a separate prioritization context: count commits touching a declared
module over the preceding 30 days, with source window and path/rename policy. It is not a sixth
quality score or proof that a complex function causes bugs. The v1 core does not require a churn
payload; a source-linked drill-down may supply it without changing the five metrics.

### 2. Unused candidates

Report tool candidates partitioned into `validated`, `false_positive`, and `uncertain`.
Every validation links to owning-repo evidence and declared entrypoints. `validated` means the
maintainer checked reachability within this exact graph/scope, not permission to delete code.
An unqualified graph can show raw candidates for investigation; it cannot headline a dead-code
total or an improvement trend. Graph changes start a new series.

The first pilot holds this headline metric: gille-inference's ten examined candidates yielded
eight false positives and two uncertain cases. Omitted operator entrypoints, tests and dynamic
consumers make raw totals misleading. No automatic deletion follows a candidate finding.

### 3. Scoped coverage

Show covered / eligible units and identify `lines`, `branches`, or `functions`. A positive
denominator and a complete declared eligible inventory are required. Include eligible unimported
source as zero where supported; otherwise explicitly name an **exported-only** scope and its gap
against the broader inventory. Never label an exported-only percentage repository coverage.
Missing feature targets are unsupported/unknown slots, not zero coverage.

Clean profile isolation is mandatory for a measured baseline. Failed-attempt profiles mixed into
a corrected run invalidate that baseline. Rust inline-test-inclusive denominators must remain
labelled; Swift source-only and test-bundle denominators are separate; AST parsing is not execution
coverage. Do not combine measures, languages, feature sets, platforms, or source/test populations.

### 4. First-attempt main CI reliability

Pilot cohort: `push` events on `main`, one pinned workflow definition and expected job inventory,
over a trailing 28-day window `[start, end)`. PR, scheduled and manually dispatched runs are distinct
cohorts. A job matrix/configuration change starts a new series or an explicitly reviewed bridge.
Record unique workflow run IDs and commit, actual **attempt 1**, latest attempt and all expected jobs.
If latest attempt is greater than one, retrieve the provider's attempt-1 record; latest green is
not evidence for the first attempt. Missing/inaccessible first-attempt evidence stays unknown.

The payload binds `run_inventory_ref` and `expected_run_count`; every expected run appears once,
with explicit unknown outcomes when attempt evidence is unavailable. The count must equal the
provided run inventory. If enumeration itself is incomplete, the entire CI slot is unknown/failed,
not a measured subset. More than 1,000 runs exceeds this bounded v1 payload and requires an
explicit contract revision; never truncate silently. Every expected job is represented exactly once. Missing evidence needs an explicit unknown job;
extra or duplicate jobs invalidate the declared cohort. Run outcome is derived in this priority:
failure, infrastructure failure, unknown, pending, cancelled, skipped, success. A known failure
therefore remains a failure even when fail-fast cancels dependent jobs. Preserve all job outcomes
so unknown/cancelled evidence remains visible alongside that failure. Success means all expected
jobs succeeded. Legitimately conditional jobs belong in a separately versioned
eligibility inventory; do not silently discard a skipped expected job.

Reliability numerator = successful first attempts. Denominator = success + failure + infrastructure
failure. All three are terminal attempts; infrastructure classification remains visible and requires
source evidence. Cancelled, skipped, pending and unknown are separate counts outside that fraction.
Always show those excluded counts and total expected run inventory so a selective green fraction
cannot hide missing runs. Zero denominator yields no percentage. Fewer than 20 eligible attempts
is **sparse**: show counts and the fraction's sample size, withhold improvement/regression claims.
Rerun success alone never establishes test flakiness or root cause.

### 5. Confirmed release-linked regressions

The product maintainer owns a triage record with these exact labels/fields: `code-health:regression`
and one of `severity:critical`, `severity:major`, `severity:minor`; a published release reference;
the introduction commit; a known-good release; a reproducer or corroborating evidence reference;
and a maintainer confirmation reference. Labels are a **proposed adoption convention**: their
creation and enforcement remain in the producer tickets. Labels without the structured evidence
do not count. A bug, a commit containing “fix”, and a suspected regression do not qualify.

Critical = release prevents the principal supported workflow or causes data loss; major = a
supported workflow fails with substantial impact but a workaround exists; minor = limited impact
with the principal workflow usable. These are product-impact categories, not security severity.
The maintainer resolves ambiguous classification and records corrections.

Pilot observation window per release: the first 14 days after publication, `[published, +14 days)`.
Only fully observed release windows with a reviewed complete issue survey can supply a measured
count. A complete cohort with no published releases is a measured zero with population zero
and a sparse label; incomplete release enumeration remains unknown. The release cohort selects publications in `[cohort_start, cohort_end)`, independently of each
release's 14-day observation window. Record both windows, release inventory, per-issue severity
and first-observed time; every observation window must have ended by the snapshot's as-of time.
Count a confirmed issue once in its introducing release, not once per affected release, fix or
label change. A later discovery is linked to the source issue and reported outside this fixed
window; do not silently lengthen old windows. Incomplete surveillance yields unknown, not zero.
Show counts by severity and eligible releases. Below five fully observed releases, label sparse;
v1 does not derive a regression rate or compare repos. Both exploratory histories remain unknown.

## Agent observations and final assessment

The rubric is `changeability-1.0`, derived from the corrected eight-scenario exploration.
Each dimension is assessed separately:

| Dimension | Easy | Manageable | Difficult |
| --- | --- | --- | --- |
| Locate | One obvious owning file/symbol | Two or three related files, or one indirection | Distributed/ambiguous ownership or hidden consumer |
| Understand | Local behavior and explicit contract | A few known callers, data paths or compatibility details | Hidden consumers, cross-layer contracts or materially uncertain state |
| Verify | An available deterministic direct check | Small setup, several checks or contract inspection | Validation spans layers or remains materially uncertain despite available checks |

`not-assessable` is a distinct state, never numeric zero or easy. Pure Q&A is not applicable and
all three ratings are not-assessable. An unavailable verification environment makes Verify
not-assessable while Locate and Understand can remain assessable. Preserve `code`, `environment`
and evidenced `mixed` causes separately, and positive/negative polarity. An environmental failure
must not automatically downgrade the code. Each grounded rating cites bounded source evidence.

Keep at most three concise observations per assessment, with supporting and counter-evidence
references. A useful observation explains the concrete obstacle or helpful property; no copied
code or conversational narrative is needed. Terminal `completed`, `partial`, `failed`, `aborted`
outcomes remain separate from assessment completeness. Partial/failed/aborted attempts cannot
claim a complete assessment in this pilot. Missing assessment is not an easy task.

Bind repository, before/after revisions where known, touched module references, task class, stable
task and attempt IDs, parent/child lineage, record and occurrence IDs, reporter and actual worker
identity. Requested model is separate from observed model/effort; unknown stays null. A parent
model must not be substituted for the worker. Locally stable external IDs are allowed; do not
invent Hugin ownership for non-Hugin work. A task/attempt join is trusted only through its owner.

### Replay, retries, conflicts and Close

An exact record replay is idempotent. Reusing its ID with different data is a collision and fails.
Deduplicate observation occurrences by repository + task + occurrence ID while retaining every
record, reporter, worker, attempt, parent and source reference. A parent or Close summary refers
to the existing source occurrence: it does not add a finding. A distinct retry retains its own
attempt and, for a newly experienced occurrence, a new occurrence ID. Do not merge by wording.
Conflicting polarity, dimension or cause remains one conflicted occurrence requiring investigation;
newest-wins is forbidden. Dangling or cyclic source-observation, correction and
assessment-observation references cannot be admitted as complete. Native `parent_attempt_id`
and `child_attempt_ids` instead resolve through the task owner: a known-but-unassessed child
need not have an agent record in the batch/context. Validation rejects contradictions among
supplied lineage records; absent child assessments neither add participation nor prove that
the child completed. A complete parent assessment describes that parent attempt only.

Known expected attempt identities (repository, task and attempt together) provide the participation
denominator; a reused short attempt ID in another task or repo is a different identity. Count assessed applicable
attempts once and keep completeness/outcome visible. Unknown expected population yields an unknown
denominator and no participation percentage. Report adoption for the registered surfaces only;
never claim all coding work or all agents participated. Model comparisons and automated routing
are outside this contract. Close is an explicit finalization path, not an automatic new assessment
agent after every edit; the continuous skill captures evidence as it occurs.

## Pilot policy: reviewed choices, not observed production results

The six-week clock starts only after both pilot product repositories have enabled conforming
collection and registered their expected scopes/workflows and participating task surfaces.
Record that start explicitly. This contract merge and historical samples do not start the clock.

| Signal | Collection choice | Freshness / minimum useful sample |
| --- | --- | --- |
| Complexity and scoped coverage | Weekly, plus before/after an agreed simplification sprint on immutable revisions | Stale after 10 days. At least 4 of 6 weekly snapshots per enabled scope, with the same comparable series. Both sprint endpoints required for a before/after claim. |
| First-attempt CI | Daily metadata collection, trailing 28 days | Collector stale after 48 hours; at least 20 eligible attempts, all run outcomes visible. |
| Agent evidence | During work; assessment once at terminal handoff, normally within 24 hours | Weekly summaries; at least 10 applicable attempts per repo/task class and assessments for 80% of the known participating eligible attempt population before interpreting a trend. Unknown denominator cannot pass. |
| Unused candidates / confirmed regressions | Hold headline trends until graph qualification / complete release-window evidence respectively | Report current limitations and unknown slots. Release evidence minimum 5 fully observed windows for a trend discussion. |

The budget is a **pilot cap**, informed by one-machine spike measurements, not a cost prediction:

- Cheap static analysis + CI metadata: at most 30 seconds incremental wall time per repo snapshot.
- Coverage: at most 180 seconds per selected warm target and 600 seconds total per repo snapshot;
  a cold/setup attempt has a separate 900-second cap and is reported separately. No per-commit
  coverage rollout; weekly/pre-post-sprint only. Exceeding a cap records failed/partial collection,
  never truncates an inventory into an apparently complete measurement.
- Subjective capture/finalization: target at most 30 seconds and 1,000 additional tokens per
  attempt, using the existing work context. No extra evaluator model call by default. These are
  provisional caps because the trial supplied **no observed time/token telemetry**.
- Record actual elapsed time, measured added CI time and tokens with sample N where available;
  otherwise unknown. Separate setup, analysis and ordinary tests. Review after two collected
  weeks; if median incremental work exceeds the caps or useful coverage is unmet, reduce scope
  or stop the pilot explicitly. Do not reduce honesty or exclude difficult attempts.

Measured anchors: gille-inference ESLint corrected run 1.92 s, qualified Knip invocation 4.40 s
(graph still unqualified), scoped Vitest coverage 7.20 s local wall time; SagaScript Rust core
coverage 81.21 s including compile and Swift coverage 16.63 s, with prerequisite build 7.88 s.
The rejected CLI coverage timing is not a clean baseline. Hosted incremental overhead remains
unknown. These measurements motivate a bounded trial; they do not establish portable budgets.

## Series compatibility, correction and privacy

Series identity includes repo, registered slot, metric/rubric and contract version, tool algorithm/version/config,
scope policy/inventory-construction version and digest, language/platform/features, measure/unit, threshold, workflow
and eligibility policy where relevant. Commit, timestamp, evaluated inventory membership and population size can vary within a series;
show population changes explicitly. Scope digests identify the inclusion/exclusion and inventory
construction policy, not the changing source bytes or list of files. Preserve the actual per-commit
inventory by reference. Compare only equal policy identities. Any changed semantics starts a new series. A reviewed bridge requires paired
measurements on the same immutable source and explicit old/new identities, scope and evidence;
v1 does not auto-bridge or rewrite history. Unknown schema versions fail closed pending adoption.

Producers admit only bounded metadata and opaque references. The optional short agent summary
must be sanitized by its producer before persistence: no transcripts, source snippets, prompts,
credentials, personal/client content, raw errors or private locators. Shape validation is not a
secret scanner and cannot prove a summary safe. Omit it if safe minimization is uncertain.
Reference resolution enforces the source's access controls. The pilot's default classification is
internal; a public synthetic fixture is not permission to publish real repository/task evidence.

The [data lifecycle map](data-lifecycle.md) remains the retention authority. Code-health snapshots,
agent evidence and derived history use the six-month evidence window from collection, with no
automatic promotion to permanent personal memory. Raw local collection/trial artifacts use 30 days
unless deliberately promoted as owned research evidence. Synthetic schemas/fixtures are durable
contract artifacts. Existing source issues/releases retain their own lifecycle.

Objective corrections pair `supersedes_ref` with `correction_ref`; agent corrections pair
`supersedes_record_id` with `correction_ref`. Both links are null for an original record.
Corrections append a successor bound to the original ID and correction evidence; preserve original
provenance and flag conflicting reports. An assessment correction must not increment participation.
The owning store authenticates the writer and serializes correction/idempotency admission; schema
validation does not supply authorization. Heimdall invalidates affected cached/derived points and
recomputes or marks unknown, preserving the correction boundary in display.

Deletion begins at the authoritative store and propagates by record/reference lineage to Munin,
Heimdall caches/history and known exports/backups. Keep only a content-free tombstone/audit ID when
permitted. Do not retain erased summaries in a correction ledger. Verify readback and backup expiry;
blocked stores remain explicit, and restore must not resurrect expired/erased evidence. Collection
must not begin on a store without an accepted retention/erasure mechanism. No pruning/deletion
job is created or authorized by this contract.

## Adoption, verification and reversal

Downstream conformance remains tracked in [gille-inference #402](https://github.com/Magnus-Gille/gille-inference/issues/402),
[SagaScript #332](https://github.com/Magnus-Gille/sagascript/issues/332),
[claude-skills #12](https://github.com/Magnus-Gille/claude-skills/issues/12),
[claude-config #45](https://github.com/Magnus-Gille/claude-config/issues/45),
[Munin #358](https://github.com/Magnus-Gille/munin-memory/issues/358),
[Hugin #405](https://github.com/Magnus-Gille/hugin/issues/405), and
[Heimdall #84](https://github.com/Magnus-Gille/heimdall/issues/84) / [#85](https://github.com/Magnus-Gille/heimdall/issues/85).
The program remains [Grimnir #210](https://github.com/Magnus-Gille/grimnir/issues/210).
Producer/consumer owners must pin the accepted contract revision and pass its synthetic fixtures
before live adoption; merged shared schemas are not proof that those components conform.

Every later enablement records the exact revisions/configuration, collection scopes, authority,
verification, audit event and reversal recipe in its owning ticket. Reversal disables collection
and derived presentation, restores the previous configuration, and verifies no new writes; it does
not silently delete historical evidence. A schema correction uses a reviewed successor/revert PR.
No advisory finding authorizes a code change, deployment, merge gate, model-routing change or
autonomous mutation. Existing owner approval boundaries continue to govern those actions.

## Source evidence

- [gille-inference report at accepted merge](https://github.com/Magnus-Gille/gille-inference/blob/87f93c84b5d1292b198dfe5aa0cb29c56baa4df4/research/code-health-401/report.md)
- [SagaScript report at accepted merge](https://github.com/Magnus-Gille/sagascript/blob/14506ae50a58aafb5432de1285875d62f82d3394/docs/research/code-health-2026-10-09/README.md)
- [Corrected agent trial](https://github.com/Magnus-Gille/claude-skills/blob/37da7fe63e2c54ba4c29309639e401fbf26ffb9e/research/code-friction-11-v2/analysis/report-v2.md)

The corrected trial's 21/24 agreement is pilot evidence, not external validity or a model ranking.
The rejected initial trial remains documented: agreement alone did not detect invalid tasks.
