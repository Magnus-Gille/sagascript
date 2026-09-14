import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/transcribe-stages.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { initialStages, startStages, updateStages, finishStages, stageRows, acceptRunProgress } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`,
);

test("progress from another run, training, or an idle view cannot advance the current import", () => {
  const state = startStages();
  for (const payload of [null, 80, {}, {runId:'old',phase:'transcribing',percent:100}]) {
    assert.deepEqual(acceptRunProgress(state, 'current', payload), state);
  }
  assert.deepEqual(acceptRunProgress(state, null, {runId:'current',phase:'transcribing',percent:80}), state);
  assert.equal(acceptRunProgress(state, 'current', {runId:'current',phase:'resampling',percent:40}).index, 1);
});

test("three empty steps become active in order, only successful completion checks all three", () => {
  assert.deepEqual(stageRows(initialStages()).map(r => r.status), ["pending", "pending", "pending"]);
  let state = startStages();
  state = updateStages(state, "decoding", 99);
  assert.deepEqual(stageRows(state).map(r => r.status), ["active", "pending", "pending"]);
  state = updateStages(state, "resampling", 0);
  assert.deepEqual(stageRows(state).map(r => r.status), ["completed", "active", "pending"]);
  for (const pct of [25, 50, 99]) {
    state = updateStages(state, "resampling", pct);
    assert.equal(stageRows(state)[1].percent, pct);
  }
  state = updateStages(state, "resampling", 100);
  assert.equal(stageRows(state)[1].status, "active", "conversion is not the whole preparation step");
  state = updateStages(state, "loading");
  assert.equal(stageRows(state)[1].percent, null, "no invented model-load percentage");
  state = updateStages(state, "encoding");
  state = updateStages(state, "transcribing", 0);
  assert.equal(stageRows(state)[1].status, "active", "zero progress is not decoded words");
  state = updateStages(state, "transcribing", 35);
  assert.deepEqual(stageRows(state).map(r => r.status), ["completed", "completed", "active"]);
  state = updateStages(state, "transcribing", 100);
  assert.equal(stageRows(state)[2].status, "active", "wait for the command result");
  state = finishStages(state, "completed");
  assert.deepEqual(stageRows(state).map(r => r.status), ["completed", "completed", "completed"]);
  assert.deepEqual(updateStages(state, "decoding", 0), state, "late events cannot change completed rows");
});

test("error and cancellation preserve completed work without checking unfinished steps", () => {
  for (const outcome of ["failed", "cancelled"]) {
    for (const [phase, index] of [["decoding", 0], ["resampling", 1], ["transcribing", 2]]) {
      const state = finishStages(updateStages(startStages(), phase, 40), outcome);
      const rows = stageRows(state);
      assert.equal(rows[index].status, outcome);
      assert.ok(rows.slice(0, index).every(r => r.status === "completed"));
      assert.ok(rows.slice(index + 1).every(r => r.status === "pending"));
      assert.deepEqual(updateStages(state, "transcribing", 100), state);
    }
  }
});

test("warm/skipped phases work, retry resets, and malformed or older events cannot regress", () => {
  let state = updateStages(startStages(), "preparing");
  state = updateStages(state, "transcribing", 60);
  assert.deepEqual(stageRows(state).map(r => r.status), ["completed", "completed", "active"]);
  for (const [phase, pct] of [["decoding", 99], ["resampling", 40], ["transcribing", 20], ["transcribing", NaN], ["loading"], ["nonsense", 100]]) {
    state = updateStages(state, phase, pct);
    assert.equal(stageRows(state)[2].percent, 60);
  }
  assert.deepEqual(stageRows(startStages()).map(r => r.status), ["active", "pending", "pending"]);
  assert.equal(stageRows(startStages())[0].percent, 0);
});
