import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  assembleReportBundle,
  assertCompletePageCount,
  buildCiFirstAttemptPayload,
  createObjective,
  parseWorkflowJobIds,
  normalizeCiCohort,
  parseExportedCoverage,
  parseKnipCandidates,
  parseRustComplexity,
  wholeSecondUtc,
} from "./lib/code-health-producer.mjs";
import { validateObjective, validateObjectiveSeries } from "./lib/code-health-objective.mjs";
import { seriesKey } from "./lib/code-health-policy.mjs";
import { summarizeCohorts } from "./ci-cohort-timings.mjs";

const readJson = path => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const config = readJson("../docs/code-health-producer-v1.json");
const schema = readJson("../docs/code-health-objective-v1.schema.json");
const fixture = path => readJson("../tests/fixtures/code-health/" + path);
const commit = "a".repeat(40);
const runRef = "ref:local-aaaaaaaaaaaa-attempt-1";
const observedAt = "2026-10-09T10:00:00Z";
const digest = "b".repeat(64);
const at = (offsetSeconds = 0) => new Date(Date.parse("2026-09-15T00:00:00Z") + offsetSeconds * 1000).toISOString().replace(".000Z", "Z");
const expectedJobs = ["scope", "test-macos"];

test("Rust complexity counts function-body complexity once and uses a strict threshold", () => {
  const result = parseRustComplexity([{
    path: "src-tauri/crates/sagascript-cli/src/transcribe.rs",
    ast: fixture("rust-complexity.json"),
  }], 10);
  assert.equal(result.eligible_functions, 4);
  assert.equal(result.above_threshold_functions, 2);
  assert.deepEqual(result.functions.map(item => [item.name, item.cyclomatic]), [
    ["outer", 11],
    ["nested", 4],
    ["exact_threshold", 10],
    ["<anonymous>", 11],
  ]);
  assert.throws(() => parseRustComplexity([], 10), /inventory is empty/);
  assert.throws(() => parseRustComplexity([{ path: "broken.rs", ast: { spaces: [{ kind: "function", spaces: [] }] } }], 10), /score is missing/);
});

test("Rust coverage labels inline tests and keeps inactive or non-emitted files out of the denominator", () => {
  const parsed = parseExportedCoverage(fixture("rust-coverage.json"), {
    repoRoot: "/workspace/sagascript",
    inventoryPaths: [
      "src-tauri/crates/sagascript-core/src/covered.rs",
      "src-tauri/crates/sagascript-core/src/zero.rs",
      "src-tauri/crates/sagascript-core/src/feature-disabled.rs",
      "src-tauri/crates/sagascript-core/tests/it.rs",
    ],
    sourceRoots: [
      "src-tauri/crates/sagascript-core/src/",
      "src-tauri/crates/sagascript-core/tests/",
    ],
    excludedPaths: ["src-tauri/crates/sagascript-core/src/generated/BuildInfo.rs"],
    includesInlineTests: true,
  });
  assert.equal(parsed.measure, "lines");
  assert.equal(parsed.covered, 7);
  assert.equal(parsed.eligible, 16);
  assert.equal(parsed.emitted_source_files, 2);
  assert.equal(parsed.eligible_source_files, 2);
  assert.equal(parsed.includes_inline_tests, true);
  assert.equal(parsed.source_inventory, "exported-only");
  assert.deepEqual(parsed.not_emitted_paths, [
    "src-tauri/crates/sagascript-core/src/feature-disabled.rs",
    "src-tauri/crates/sagascript-core/tests/it.rs",
  ]);
  assert.ok(parsed.excluded_report_paths.every(item => item.path === null || !item.path.startsWith("/")));
  assert.throws(() => parseExportedCoverage({ data: [] }, {
    repoRoot: "/workspace/sagascript", inventoryPaths: [], sourceRoots: ["src/"],
  }), /emitted no files/);
  assert.throws(() => parseExportedCoverage({ data: [{ files: [{ filename: "src/a.rs", summary: { lines: { count: 1, covered: 2 } } }] }] }, {
    repoRoot: "/workspace/sagascript", inventoryPaths: ["src/a.rs"], sourceRoots: ["src/"],
  }), /counts are invalid/);
});

test("Swift source-only coverage excludes generated identity and test bundle files", () => {
  const parsed = parseExportedCoverage(fixture("swift-coverage.json"), {
    repoRoot: "/workspace/sagascript",
    inventoryPaths: [
      "src-tauri/engine-host/coreml/Sources/EngineHostCore/ProtocolServer.swift",
      "src-tauri/engine-host/coreml/Sources/EngineHostCore/CoreMLEngine.swift",
    ],
    sourceRoots: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/"],
    excludedPaths: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/BuildInfo.swift"],
  });
  assert.equal(parsed.covered, 80);
  assert.equal(parsed.eligible, 100);
  assert.equal(parsed.emitted_source_files, 1);
  assert.deepEqual(parsed.not_emitted_paths, ["src-tauri/engine-host/coreml/Sources/EngineHostCore/CoreMLEngine.swift"]);
  assert.ok(!parsed.included_paths.some(path => path.includes("BuildInfo") || path.includes("Tests/")));
});

test("Knip candidates are projected by source language and generated/test candidates are excluded", () => {
  const report = fixture("knip.json");
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "typescript" }), [
    { category: "exports", path: "src/lib/Settings.ts", name: "oldSetting", line: 18 },
    { category: "files", path: "src/lib/Unused.ts", name: null, line: 1 },
  ]);
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "svelte" }), [
    { category: "svelte", path: "src/lib/Unused.svelte", name: "Unused.svelte", line: 1 },
  ]);
  assert.throws(() => parseKnipCandidates("not json", { repoRoot: "/workspace/sagascript", language: "typescript" }), /not valid JSON/);
});

test("native Knip JSON retains grouped file candidates and nested member candidates", () => {
  const report = fixture("knip-native.json");
  const typescript = parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "typescript" });
  assert.deepEqual(typescript.map(({ category, path, name, line }) => ({ category, path, name, line })), [
    { category: "classMembers", path: "src/lib/Settings.ts", name: "Settings.unusedMethod", line: 41 },
    { category: "duplicates", path: "src/lib/Settings.ts", name: "duplicateValue", line: 48 },
    { category: "enumMembers", path: "src/lib/Settings.ts", name: "Mode.Legacy", line: 35 },
    { category: "exports", path: "src/lib/Settings.ts", name: "oldSetting", line: 18 },
    { category: "nsExports", path: "src/lib/Settings.ts", name: "unusedNamespaceExport", line: 22 },
    { category: "nsTypes", path: "src/lib/Settings.ts", name: "UnusedNamespaceType", line: 31 },
    { category: "types", path: "src/lib/Settings.ts", name: "OldSettings", line: 27 },
    { category: "files", path: "src/lib/Unused.ts", name: null, line: 1 },
  ]);
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "svelte" }), [
    { category: "files", path: "src/lib/Unused.svelte", name: null, line: 1 },
    { category: "exports", path: "src/lib/Widget.svelte", name: "unusedComponentExport", line: 7 },
  ]);
  assert.throws(() => parseKnipCandidates({ files: [null], issues: [] }, { repoRoot: "/workspace/sagascript", language: "typescript" }), /file candidate is malformed/);
});

test("CI cohort uses attempt 1 despite a successful retry and preserves job outcomes", () => {
  const normalized = normalizeCiCohort({
    runs: [{
      id: 42,
      head_sha: "c".repeat(40),
      run_attempt: 2,
      event: "push",
      head_branch: "main",
      created_at: at(0),
      workflow_config_digest: digest,
      attempt1_jobs: [
        { id: 1001, name: "scope", status: "completed", conclusion: "failure", started_at: at(5), completed_at: at(20), steps: [] },
        { id: 1002, name: "test-macos", status: "completed", conclusion: "cancelled", started_at: at(6), completed_at: at(10), steps: [] },
      ],
      latest_conclusion: "success",
    }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.equal(normalized.runs.length, 1);
  assert.equal(normalized.runs[0].attempt, 1);
  assert.equal(normalized.runs[0].latest_attempt, 2);
  assert.equal(normalized.runs[0].overall_conclusion, "failure");
  assert.deepEqual(normalized.runs[0].jobs.map(job => job.conclusion), ["failure", "cancelled"]);
  const payload = buildCiFirstAttemptPayload({
    normalizedRuns: normalized.runs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    expectedJobIds: expectedJobs,
    runInventoryRef: "ref:ci-main-push-28-day",
  });
  assert.equal(payload.expected_run_count, 1);
  assert.equal(payload.runs[0].latest_attempt, 2);
  const timings = summarizeCohorts(normalized.timing_runs);
  assert.equal(timings.cohorts[0].attempt, 1);
  assert.equal(timings.cohorts[0].counts.outcomes.failure, 1);
  assert.equal(timings.cohorts[0].ledger[0].attempt, 1);
});

test("CI gaps stay unknown and unproven startup or timeout conclusions stay failures with raw evidence", () => {
  const base = {
    id: 8,
    head_sha: "d".repeat(40),
    run_attempt: 1,
    event: "push",
    head_branch: "main",
    created_at: at(400),
    workflow_config_digest: digest,
    attempt1_jobs: [
      { id: 2001, name: "scope", status: "completed", conclusion: "startup_failure", steps: [] },
      { id: 2002, name: "test-macos", status: "completed", conclusion: "timed_out", steps: [] },
    ],
  };
  const normalized = normalizeCiCohort({
    runs: [base],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(normalized.runs[0].jobs.map(job => job.conclusion), ["failure", "failure"]);
  assert.equal(normalized.runs[0].overall_conclusion, "failure");
  assert.deepEqual(normalized.timing_runs[0].jobs.map(job => job.conclusion), ["startup_failure", "timed_out"]);
  const missingJob = normalizeCiCohort({
    runs: [{ ...base, attempt1_jobs: [{ id: 2001, name: "scope", status: "completed", conclusion: "startup_failure" }] }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(missingJob.runs[0].jobs.map(job => job.conclusion), ["failure", "unknown"]);
  assert.throws(() => normalizeCiCohort({
    runs: [{ ...base, created_at: "not-a-date" }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  }), /malformed identity or timestamp/);
  assert.throws(() => normalizeCiCohort({
    runs: [{ ...base, workflow_config_digest: "e".repeat(64) }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  }), /configuration changed/);
  assert.throws(() => normalizeCiCohort({
    runs: [{ ...base, attempt1_jobs: [...base.attempt1_jobs, { id: 2003, name: "new-job", status: "completed", conclusion: "success" }] }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  }), /unregistered job/);
  assert.throws(() => normalizeCiCohort({
    runs: [{ ...base, id: "9" }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  }), /malformed identity or timestamp/);
  assert.throws(() => normalizeCiCohort({
    runs: [{ ...base, attempt1_jobs: [{ id: "2001", name: "scope", status: "completed", conclusion: "success" }] }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  }), /malformed identity/);
});

test("failed workflow with mixed known and unknown jobs is retained but makes the CI slot incomplete", () => {
  const normalized = normalizeCiCohort({
    runs: [{
      id: 91,
      head_sha: "e".repeat(40),
      run_attempt: 1,
      event: "push",
      head_branch: "main",
      created_at: at(500),
      status: "completed",
      conclusion: "failure",
      workflow_config_digest: digest,
      attempt1_jobs: [{ id: 9101, name: "scope", status: "completed", conclusion: "success" }],
    }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(normalized.runs[0].jobs.map(job => job.conclusion), ["success", "unknown"]);
  assert.equal(normalized.runs[0].overall_conclusion, "unknown");
  assert.deepEqual(normalized.incomplete_runs, [{ id: 91, status: "completed", conclusion: "failure" }]);
  assert.equal(normalized.timing_runs[0].status, "completed");
  assert.equal(normalized.timing_runs[0].conclusion, "failure");
});

test("CI workflow job inventory and pagination reject incomplete or duplicate evidence", () => {
  const workflow = `name: CI\njobs:\n  # Stable IDs are the cohort slots.\n  scope:\n    runs-on: ubuntu-latest\n  test-macos:\n    needs: scope\non:\n  push:\n`;
  assert.deepEqual(parseWorkflowJobIds(workflow), ["scope", "test-macos"]);
  assert.throws(() => parseWorkflowJobIds("name: empty\non:\n  push:\n"), /no jobs section/);
  assert.throws(() => parseWorkflowJobIds("jobs:\n  scope:\n  scope:\n"), /duplicated/);
  assert.doesNotThrow(() => assertCompletePageCount(120, 120, 1000));
  assert.throws(() => assertCompletePageCount(100, 120, 1000), /does not match total_count/);
  assert.throws(() => assertCompletePageCount(1001, 1001, 1000), /exceeds the configured limit/);
});

test("contract scope registry creates six exact-schema snapshots with distinct slot identities", () => {
  const scopes = config.scopes;
  const snapshots = scopes.map(scope => ({
    contract_version: "1.0",
    snapshot_id: "ref:sagascript-" + scope.name + "-" + commit.slice(0, 12),
    supersedes_ref: null,
    correction_ref: null,
    repository: { owner: "Magnus-Gille", name: "sagascript" },
    commit,
    observed_at: observedAt,
    metrics: Object.fromEntries([
      ["complex_functions", "functions"],
      ["unused_candidates", "candidates"],
      ["coverage", "lines"],
      ["ci_first_attempt", "runs"],
      ["confirmed_regressions", "regressions"],
    ].map(([name, unit]) => {
      const slot = scope.slots[name];
      return [name, {
        observed_at: observedAt,
        source: null,
        population: null,
        unit,
        status: slot.status === "measured" ? "unknown" : slot.status,
        payload: null,
        reason: slot.status === "measured" ? "not-collected" : slot.reason,
        slot_ref: slot.slot_ref,
      }];
    })),
  }));
  assert.equal(new Set(scopes.flatMap(scope => Object.values(scope.slots).map(slot => slot.slot_ref))).size, 30);
  for (const snapshot of snapshots) assert.equal(validateObjective(schema, snapshot).valid, true);
});

test("snapshot identities distinguish collections and attempts while replaying the same artifact", () => {
  const scope = config.scopes.find(item => item.name === "rust-core");
  const context = (runRef, attempt) => ({
    commitSha: commit,
    observedAt,
    runRef,
    attempt,
  });
  const first = createObjective(scope, context("ref:github-run-7021-attempt-1", 1));
  const replay = createObjective(scope, context("ref:github-run-7021-attempt-1", 1));
  const rerun = createObjective(scope, context("ref:github-run-7021-attempt-2", 2));
  const otherRun = createObjective(scope, context("ref:github-run-7022-attempt-1", 1));
  const sameCommitPrefix = createObjective(scope, {
    ...context("ref:github-run-7021-attempt-1", 1),
    commitSha: "a".repeat(12) + "b".repeat(28),
  });

  assert.equal(first.snapshot_id, replay.snapshot_id);
  assert.notEqual(first.snapshot_id, rerun.snapshot_id);
  assert.notEqual(first.snapshot_id, otherRun.snapshot_id);
  assert.notEqual(first.snapshot_id, sameCommitPrefix.snapshot_id);
  const admission = validateObjectiveSeries(schema, [first, replay, rerun, otherRun, sameCommitPrefix]);
  assert.equal(admission.valid, true, admission.errors.join("; "));
  assert.equal(admission.replayCount, 1);
});

test("scope digest tracks registered policy rather than changing population membership", () => {
  const scope = config.scopes.find(item => item.name === "rust-core");
  const context = { commitSha: commit, observedAt, runRef, attempt: 1 };
  const scopePolicy = {
    inventory: "configured-rust-source-files-v1",
    included_paths: ["src-tauri/crates/sagascript-core/src/download.rs"],
    excluded_paths: ["generated/**"],
    parser: config.complexity.function_inventory,
  };
  const objectiveFor = (includedRefs, policy = scopePolicy) => createObjective(scope, context, {
    complex_functions: {
      status: "measured",
      payload: {
        algorithm: config.complexity.algorithm,
        threshold: config.complexity.threshold,
        eligible_functions: includedRefs.length,
        above_threshold_functions: 0,
      },
      tool: { name: "rust-code-analysis-cli", version: "0.0.25" },
      command: "rust-code-analysis-cli -p <selected-repository-path> -m -F -O json --pr",
      scope_policy: policy,
      included_refs: includedRefs,
      excluded_refs: [],
    },
  });
  const first = objectiveFor(["ref:function-a"]);
  const changedPopulation = objectiveFor(["ref:function-a", "ref:function-b"]);
  const changedPolicy = objectiveFor(["ref:function-a"], {
    ...scopePolicy,
    excluded_paths: ["generated/**", "vendor/**"],
  });
  assert.equal(seriesKey(schema, first, "complex_functions"), seriesKey(schema, changedPopulation, "complex_functions"));
  assert.notEqual(seriesKey(schema, first, "complex_functions"), seriesKey(schema, changedPolicy, "complex_functions"));
  assert.deepEqual(first.metrics.complex_functions.population.included_refs, ["ref:function-a"]);
  assert.deepEqual(changedPopulation.metrics.complex_functions.population.included_refs, ["ref:function-a", "ref:function-b"]);
});

test("report transport uses the root manifest shape and rejects unresolved population evidence", () => {
  const noMeasures = Object.fromEntries(config.scopes.map(scope => [scope.name, {}]));
  const bundle = assembleReportBundle({
    scopes: config.scopes,
    measurementsByScope: noMeasures,
    context: { commitSha: commit, observedAt, runRef, attempt: 1 },
    evidenceFiles: { "evidence/collection-overhead.json": { elapsed_seconds: 0.5, measured: true } },
    config,
  });
  assert.deepEqual(Object.keys(bundle.manifest), [
    "contract_version", "repo_owner", "repo_name", "commit_sha", "snapshots", "evidence_index",
  ]);
  assert.equal(bundle.manifest.snapshots.length, 6);
  assert.equal(bundle.evidenceIndex.version, "1.0");
  for (const snapshot of bundle.snapshots) assert.equal(validateObjective(schema, snapshot).valid, true);
  assert.throws(() => assembleReportBundle({
    scopes: [config.scopes[0]],
    measurementsByScope: {
      "rust-core": {
        complex_functions: {
          status: "measured",
          payload: { algorithm: "cyclomatic-complexity-v1", threshold: 10, eligible_functions: 2, above_threshold_functions: 1 },
          tool: { name: "rust-code-analysis-cli", version: "0.0.25" },
          command: "fixture",
          scope_policy: { inventory: "fixture-policy-v1" },
          included_refs: ["ref:missing-inventory"],
          excluded_refs: [],
        },
      },
    },
    context: { commitSha: commit, observedAt, runRef, attempt: 1 },
    config,
  }), /unresolved/);
});

test("timestamps are normalized to the contract's UTC whole-second precision", () => {
  assert.equal(wholeSecondUtc("2026-10-09T10:00:00.987Z"), observedAt);
  assert.throws(() => wholeSecondUtc("invalid"), /invalid timestamp/);
});
