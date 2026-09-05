import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { summarizeRun } from "./ci-run-timings.mjs";

const cli = fileURLToPath(new URL("./ci-run-timings.mjs", import.meta.url));
const time = (minutes, seconds) => `2026-09-05T00:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}Z`;

function job(databaseId, startedAt, completedAt, steps = []) {
  return { databaseId, name: `job-${databaseId}`, status: "completed", conclusion: "success", startedAt, completedAt, steps };
}

function deepFreeze(value) {
  if (value && typeof value === "object" && !Object.isFrozen(value)) {
    for (const nested of Object.values(value)) deepFreeze(nested);
    Object.freeze(value);
  }
  return value;
}

test("summarizes jobs and steps with numeric ordering", () => {
  const result = summarizeRun({
    databaseId: 8,
    headSha: "abc1234",
    jobs: [
      job(10, time(0, 0), time(0, 3), [
        { number: 10, name: "ten", startedAt: time(0, 2), completedAt: time(0, 3) },
        { number: 2, name: "two", startedAt: time(0, 0), completedAt: time(0, 1) },
      ]),
      job(2, time(0, 1), time(0, 2)),
    ],
  });
  assert.deepEqual(result.jobs.map(({ id }) => id), [2, 10]);
  assert.deepEqual(result.jobs[1].steps.map(({ number }) => number), [2, 10]);
  assert.equal(result.wallSeconds, 3);
  assert.equal(result.runnerSeconds, 4);
});

test("does not mutate a nested unsorted input", () => {
  const input = {
    databaseId: 8,
    jobs: [
      job(10, time(0, 0), time(0, 3), [
        { number: 10, name: "ten" },
        { number: 2, name: "two" },
      ]),
      job(2, time(0, 1), time(0, 2)),
    ],
  };
  const original = structuredClone(input);
  summarizeRun(deepFreeze(input));
  assert.deepEqual(input, original);
});

test("uses the parallel wall span and summed runner time", () => {
  const result = summarizeRun({
    databaseId: 1,
    jobs: [job(1, time(0, 0), time(10, 0)), job(2, time(2, 0), time(5, 0))],
  });
  assert.equal(result.wallSeconds, 600);
  assert.equal(result.runnerSeconds, 780);
});

test("empty and incomplete jobs produce null totals without losing completed job data", () => {
  assert.deepEqual(summarizeRun({ databaseId: 1, jobs: [] }).jobs, []);
  const incomplete = summarizeRun({
    databaseId: 1,
    jobs: [job(1, time(0, 0), time(0, 3), [{ number: 1, name: "pending" }]), job(2, time(0, 0), null)],
  });
  assert.equal(incomplete.jobs[0].durationSeconds, 3);
  assert.equal(incomplete.jobs[0].steps[0].durationSeconds, null);
  assert.equal(incomplete.wallSeconds, null);
  assert.equal(incomplete.runnerSeconds, null);
});

test("completed job totals survive incomplete or skipped steps", () => {
  const result = summarizeRun({
    databaseId: 1,
    jobs: [job(1, time(0, 0), time(1, 0), [{ number: 1, name: "skipped", status: "completed", conclusion: "skipped" }])],
  });
  assert.equal(result.wallSeconds, 60);
  assert.equal(result.runnerSeconds, 60);
  assert.equal(result.jobs[0].steps[0].durationSeconds, null);
});

test("missing optional metadata becomes null", () => {
  const result = summarizeRun({
    databaseId: 1,
    jobs: [{ databaseId: 2, steps: [{ number: 1 }] }],
  });
  assert.equal(result.headSha, null);
  assert.equal(result.jobs[0].name, null);
  assert.equal(result.jobs[0].status, null);
  assert.equal(result.jobs[0].conclusion, null);
  assert.equal(result.jobs[0].durationSeconds, null);
});

test("rejects invalid dates, negative intervals, duplicate IDs, and invalid shapes", () => {
  assert.throws(() => summarizeRun({ databaseId: 1, jobs: [{ databaseId: 2, steps: [], startedAt: "nope" }] }), /startedAt is not a valid timestamp/);
  assert.throws(() => summarizeRun({ databaseId: 1, jobs: [job(2, time(1, 0), time(0, 0))] }), /negative interval/);
  assert.throws(() => summarizeRun({ databaseId: 1, jobs: [job(2, null, null), job(2, null, null)] }), /duplicate job databaseId/);
  assert.throws(() => summarizeRun({ databaseId: 1, jobs: [{ databaseId: 2, steps: null }] }), /steps must be an array/);
  assert.throws(() => summarizeRun({ databaseId: Infinity, jobs: [] }), /finite number/);
});

test("CLI prints formatted JSON for valid input", () => {
  const run = { databaseId: 7, headSha: "deadbee", jobs: [job(2, time(0, 0), time(0, 1))] };
  const result = spawnSync(process.execPath, [cli], { input: JSON.stringify(run), encoding: "utf8" });
  assert.equal(result.status, 0);
  assert.equal(JSON.parse(result.stdout).id, 7);
  assert.match(result.stdout, /\n  "headSha":/);
  assert.equal(result.stderr, "");
});

test("CLI reports malformed, invalid, and empty input on stderr without creating output files", () => {
  for (const input of ["{bad", "{\"databaseId\":1,\"jobs\":null}", ""]) {
    const result = spawnSync(process.execPath, [cli], { input, encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /ci-run-timings:/);
    if (input === "{bad") assert.match(result.stderr, /valid JSON/);
  }
});
