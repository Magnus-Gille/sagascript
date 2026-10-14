import { expect, test } from "vitest";
import { acceptRunProgress, finishStages, initialStages, stageRows, startStages, updateStages } from "./transcribe-stages";

test("progress from another run, training, or an idle view cannot advance the current import", () => {
  const state = startStages();
  for (const payload of [null, 80, {}, { runId: "old", phase: "transcribing", percent: 100 }]) {
    expect(acceptRunProgress(state, "current", payload)).toStrictEqual(state);
  }
  expect(acceptRunProgress(state, null, { runId: "current", phase: "transcribing", percent: 80 })).toStrictEqual(state);
  expect(acceptRunProgress(state, "current", { runId: "current", phase: "resampling", percent: 40 }).index).toBe(1);
});

test("three empty steps become active in order, only successful completion checks all three", () => {
  expect(stageRows(initialStages()).map((r) => r.status)).toStrictEqual(["pending", "pending", "pending"]);
  let state = startStages();
  state = updateStages(state, "decoding", 99);
  expect(stageRows(state).map((r) => r.status)).toStrictEqual(["active", "pending", "pending"]);
  state = updateStages(state, "resampling", 0);
  expect(stageRows(state).map((r) => r.status)).toStrictEqual(["completed", "active", "pending"]);
  for (const pct of [25, 50, 99]) {
    state = updateStages(state, "resampling", pct);
    expect(stageRows(state)[1].percent).toBe(pct);
  }
  state = updateStages(state, "resampling", 100);
  expect(stageRows(state)[1].status, "conversion is not the whole preparation step").toBe("active");
  state = updateStages(state, "loading");
  expect(stageRows(state)[1].percent, "no invented model-load percentage").toBe(null);
  state = updateStages(state, "encoding");
  state = updateStages(state, "transcribing", 0);
  expect(stageRows(state)[1].status, "zero progress is not decoded words").toBe("active");
  state = updateStages(state, "transcribing", 35);
  expect(stageRows(state).map((r) => r.status)).toStrictEqual(["completed", "completed", "active"]);
  state = updateStages(state, "transcribing", 100);
  expect(stageRows(state)[2].status, "wait for the command result").toBe("active");
  state = finishStages(state, "completed");
  expect(stageRows(state).map((r) => r.status)).toStrictEqual(["completed", "completed", "completed"]);
  expect(updateStages(state, "decoding", 0), "late events cannot change completed rows").toStrictEqual(state);
});

test("error and cancellation preserve completed work without checking unfinished steps", () => {
  for (const outcome of ["failed", "cancelled"] as const) {
    for (const [phase, index] of [
      ["decoding", 0],
      ["resampling", 1],
      ["transcribing", 2],
    ] as const) {
      const state = finishStages(updateStages(startStages(), phase, 40), outcome);
      const rows = stageRows(state);
      expect(rows[index].status).toBe(outcome);
      expect(rows.slice(0, index).every((r) => r.status === "completed")).toBeTruthy();
      expect(rows.slice(index + 1).every((r) => r.status === "pending")).toBeTruthy();
      expect(updateStages(state, "transcribing", 100)).toStrictEqual(state);
    }
  }
});

test("warm/skipped phases work, retry resets, and malformed or older events cannot regress", () => {
  let state = updateStages(startStages(), "preparing");
  state = updateStages(state, "transcribing", 60);
  expect(stageRows(state).map((r) => r.status)).toStrictEqual(["completed", "completed", "active"]);
  const events: [phase: string, percent?: number][] = [
    ["decoding", 99],
    ["resampling", 40],
    ["transcribing", 20],
    ["transcribing", NaN],
    ["loading"],
    ["nonsense", 100],
  ];
  for (const [phase, pct] of events) {
    state = updateStages(state, phase, pct);
    expect(stageRows(state)[2].percent).toBe(60);
  }
  expect(stageRows(startStages()).map((r) => r.status)).toStrictEqual(["active", "pending", "pending"]);
  expect(stageRows(startStages())[0].percent).toBe(0);
});

test("incremental engine progress advances the Transcribe meter monotonically without jumping", () => {
  let state = updateStages(startStages(), "preparing");
  const seen: (number | null)[] = [];
  for (const pct of [3, 9.7, 9, 24, 24.4, 58, 91, 100]) {
    state = updateStages(state, "transcribing", pct);
    seen.push(stageRows(state)[2].percent);
  }
  expect(seen).toStrictEqual([3, 9, 9, 24, 24, 58, 91, 100]);
  expect(state.status, "a native 100% never completes the run").not.toBe("completed");
});
