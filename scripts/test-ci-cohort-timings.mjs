import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { summarizeCohorts } from "./ci-cohort-timings.mjs";

const cli = fileURLToPath(new URL("./ci-cohort-timings.mjs", import.meta.url));
const at = (seconds) => new Date(Date.parse("2026-09-05T00:00:00Z") + seconds * 1000).toISOString();

function job(databaseId, start, end, name = `job-${databaseId}`, steps = []) {
  return {
    databaseId,
    name,
    status: "completed",
    conclusion: "success",
    startedAt: start === null ? null : at(start),
    completedAt: end === null ? null : at(end),
    steps,
  };
}

function run(databaseId, start, end, overrides = {}) {
  const jobStart = start === null || end === null ? start : Math.min(start + 2, end);
  return {
    databaseId,
    headSha: `sha-${databaseId}`,
    workflowName: "CI",
    event: "push",
    attempt: 1,
    status: "completed",
    conclusion: "success",
    createdAt: start === null ? null : at(start),
    jobs: [job(databaseId * 10, jobStart, end)],
    ...overrides,
  };
}

test("groups by workflow, event, and attempt and computes parallel timing", () => {
  const result = summarizeCohorts([
    run(1, 0, 10, {
      jobs: [
        job(11, 2, 10, "build", [{ number: 1, name: "compile", status: "completed", conclusion: "success", startedAt: at(3), completedAt: at(8) }]),
        job(12, 4, 7, "test", [{ number: 1, name: "unit", status: "completed", conclusion: "skipped", startedAt: at(4), completedAt: at(6) }]),
      ],
    }),
    run(2, 0, 20),
  ]);
  assert.equal(result.cohorts.length, 1);
  const cohort = result.cohorts[0];
  assert.deepEqual(cohort.counts, { total: 2, eligible: 2, incomplete: 0, success: 2, inProgress: 0, noConclusion: 0, outcomes: { success: 2 } });
  assert.deepEqual(cohort.elapsedSeconds, { n: 2, p50: 15, p90: 20, min: 10, max: 20 });
  assert.deepEqual(cohort.initialWaitSeconds, { n: 2, p50: 2, p90: 2, min: 2, max: 2 });
  assert.deepEqual(cohort.wallSeconds, { n: 2, p50: 13, p90: 18, min: 8, max: 18 });
  assert.deepEqual(cohort.runnerSeconds, { n: 2, p50: 14.5, p90: 18, min: 11, max: 18 });
  assert.deepEqual(cohort.jobs.find((entry) => entry.name === "build"), { name: "build", n: 1, p50: 8, p90: 8, min: 8, max: 8 });
  assert.deepEqual(cohort.steps.find((entry) => entry.name === "compile"), { jobName: "build", name: "compile", number: 1, n: 1, p50: 5, p90: 5, min: 5, max: 5 });
  assert.equal(cohort.steps.find((entry) => entry.name === "unit"), undefined);
});

test("uses median for p50 and nearest rank for p90", () => {
  const result = summarizeCohorts([run(1, 0, 1), run(2, 0, 3), run(3, 0, 9)]).cohorts[0];
  assert.deepEqual(result.elapsedSeconds, { n: 3, p50: 3, p90: 9, min: 1, max: 9 });
});

test("keeps mixed outcomes and incomplete successes in the ledger", () => {
  const result = summarizeCohorts([
    run(1, 0, 4),
    run(2, 0, 3, { conclusion: "failure" }),
    run(3, 0, null, { status: "in_progress", conclusion: null }),
    run(4, null, null),
  ]).cohorts[0];
  assert.deepEqual(result.counts, { total: 4, eligible: 1, incomplete: 1, success: 2, inProgress: 1, noConclusion: 1, outcomes: { failure: 1, none: 1, success: 2 } });
  assert.equal(result.elapsedSeconds.n, 1);
  assert.deepEqual(result.ledger.map(({ id, eligible, incomplete, reason }) => ({ id, eligible, incomplete, reason })), [
    { id: 1, eligible: true, incomplete: false, reason: null },
    { id: 2, eligible: false, incomplete: false, reason: "unsuccessful" },
    { id: 3, eligible: false, incomplete: false, reason: "unsuccessful" },
    { id: 4, eligible: false, incomplete: true, reason: "incomplete-timing" },
  ]);
});

test("separates attempts and rejects duplicate run/attempt identities", () => {
  const result = summarizeCohorts([run(1, 0, 1), run(2, 0, 2, { attempt: 2 })]);
  assert.equal(result.cohorts.length, 2);
  assert.throws(() => summarizeCohorts([run(1, 0, 1), run(1, 0, 2)]), /duplicate run databaseId\/attempt 1\/1/);
  assert.equal(summarizeCohorts([run(1, 0, 1), run(1, 0, 2, { attempt: 2 })]).cohorts.length, 2);
});

test("keeps architecture-specific jobs and steps separate and splits workflow/event cohorts", () => {
  const architectureRuns = [
    run(10, 0, 8, {
      jobs: [
        job(101, 1, 8, "build-x64", [{ number: 1, name: "compile", status: "completed", conclusion: "success", startedAt: at(2), completedAt: at(7) }]),
        job(102, 1, 6, "build-arm64", [{ number: 1, name: "compile", status: "completed", conclusion: "success", startedAt: at(2), completedAt: at(5) }]),
      ],
    }),
    run(11, 0, 9, {
      jobs: [
        job(111, 1, 9, "build-x64", [{ number: 1, name: "compile", status: "completed", conclusion: "success", startedAt: at(2), completedAt: at(8) }]),
        job(112, 1, 7, "build-arm64", [{ number: 1, name: "compile", status: "completed", conclusion: "success", startedAt: at(2), completedAt: at(6) }]),
      ],
    }),
  ];
  const sameCohort = summarizeCohorts(architectureRuns).cohorts[0];
  assert.deepEqual(sameCohort.jobs.map(({ name }) => name), ["build-arm64", "build-x64"]);
  assert.deepEqual(sameCohort.steps.map(({ jobName, name, number }) => ({ jobName, name, number })), [
    { jobName: "build-arm64", name: "compile", number: 1 },
    { jobName: "build-x64", name: "compile", number: 1 },
  ]);
  assert.deepEqual(sameCohort.steps.map(({ n }) => n), [2, 2]);

  const split = summarizeCohorts([
    ...architectureRuns,
    run(12, 0, 2, { workflowName: "Release" }),
    run(13, 0, 3, { event: "pull_request" }),
  ]);
  assert.equal(split.cohorts.length, 3);
  assert.deepEqual(split.cohorts.map(({ workflowName, event }) => `${workflowName}/${event}`), ["CI/pull_request", "CI/push", "Release/push"]);
});

test("validates required semantics, malformed timestamps, and negative intervals", () => {
  const missingEvent = run(1, 0, 1);
  delete missingEvent.event;
  assert.throws(() => summarizeCohorts([missingEvent]), /runs\[0\]\.event is required/);
  const missingHeadSha = run(1, 0, 1);
  delete missingHeadSha.headSha;
  assert.throws(() => summarizeCohorts([missingHeadSha]), /runs\[0\]\.headSha is required/);
  assert.throws(() => summarizeCohorts([run(1, 0, 1, { createdAt: "nope" })]), /runs\[0\]\.createdAt is not a valid timestamp/);
  assert.throws(() => summarizeCohorts([run(1, 5, 1)]), /createdAt→first job has a negative interval/);
  assert.throws(() => summarizeCohorts([run(1, 0, 1, { status: "completed", conclusion: null })]), /conclusion is required/);
});

test("counts arbitrary conclusions without colliding with object properties", () => {
  const result = summarizeCohorts([run(1, 0, 1, { conclusion: "constructor" }), run(2, 0, 1, { conclusion: "__proto__" })]).cohorts[0];
  assert.equal(result.counts.outcomes.constructor, 1);
  assert.equal(result.counts.outcomes["__proto__"], 1);
  assert.equal(result.counts.eligible, 0);
  assert.equal(result.elapsedSeconds.n, 0);
});

test("CLI supports help, JSON output, and failures", () => {
  const help = spawnSync(process.execPath, [cli, "--help"], { encoding: "utf8" });
  assert.equal(help.status, 0);
  assert.match(help.stdout, /Required run fields/);
  const valid = spawnSync(process.execPath, [cli], { input: JSON.stringify([run(7, 0, 2)]), encoding: "utf8" });
  assert.equal(valid.status, 0);
  assert.equal(JSON.parse(valid.stdout).cohorts[0].ledger[0].id, 7);
  for (const input of ["", "{bad", "{}", "[]"]) {
    const failed = spawnSync(process.execPath, [cli], { input, encoding: "utf8" });
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /ci-cohort-timings:/);
    assert.equal(failed.stdout, "");
  }
});
