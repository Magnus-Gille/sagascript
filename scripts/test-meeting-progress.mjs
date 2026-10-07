import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/meeting-progress.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const {
  formatDuration,
  phaseDetail,
  phaseLabel,
  statusCheckMessage,
  statusTitle,
} = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

test("meeting progress formats supplied durations without inventing precision", () => {
  assert.equal(formatDuration(0), "00:00");
  assert.equal(formatDuration(65_999), "01:05");
  assert.equal(formatDuration(3_661_000), "1:01:01");
  assert.equal(formatDuration(-1), "00:00");
});

test("meeting phases have human-readable labels and the long-running analysis explanation", () => {
  assert.equal(phaseLabel("loading_model"), "Loading speech model");
  assert.equal(phaseLabel("unknown"), "Processing meeting");
  assert.equal(
    phaseDetail("analyzing"),
    "Transcribing speech and identifying speakers. This can take several minutes for long recordings.",
  );
});

test("status titles cover running, cancellation, and terminal outcomes", () => {
  assert.equal(statusTitle("running"), "Processing meeting");
  assert.equal(statusTitle("cancelling"), "Cancelling meeting processing");
  assert.equal(statusTitle("completed"), "Meeting processing complete");
  assert.equal(statusTitle("cancelled"), "Meeting processing cancelled");
  assert.equal(statusTitle("failed"), "Meeting processing failed");
});

test("status feedback distinguishes recent checks, stale responses, and failed checks", () => {
  assert.equal(statusCheckMessage(0, false), "Status checked just now");
  assert.equal(statusCheckMessage(2_000, false), "Status checked 2 seconds ago");
  assert.equal(
    statusCheckMessage(10_001, false),
    "No status response for 10 seconds — processing may still be running.",
  );
  assert.equal(statusCheckMessage(0, true), "Status check failed — processing may still be running.");
});
