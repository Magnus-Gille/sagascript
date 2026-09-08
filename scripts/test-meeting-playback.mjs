import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/meeting-playback.ts", import.meta.url), "utf8");
const moduleSource = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { activeSegmentsAtTime, isSegmentActive } = await import(
  `data:text/javascript;base64,${Buffer.from(moduleSource).toString("base64")}`,
);

const segments = [
  { id: "early", start: 0, end: 2, text: "early", speaker: "a" },
  { id: "overlap", start: 1, end: 3, text: "overlap", speaker: "b" },
  { id: "late", start: 3, end: 4, text: "late", speaker: "a" },
];

test("playback uses half-open boundaries and preserves overlap order", () => {
  assert.deepEqual(activeSegmentsAtTime(segments, 0).map((segment) => segment.id), ["early"]);
  assert.deepEqual(activeSegmentsAtTime(segments, 1).map((segment) => segment.id), ["early", "overlap"]);
  assert.deepEqual(activeSegmentsAtTime(segments, 2).map((segment) => segment.id), ["overlap"]);
  assert.deepEqual(activeSegmentsAtTime(segments, 3).map((segment) => segment.id), ["late"]);
});

test("playback leaves gaps and EOF inactive", () => {
  assert.deepEqual(activeSegmentsAtTime(segments, 4), []);
  assert.deepEqual(activeSegmentsAtTime(segments, 2.5), [segments[1]]);
  assert.deepEqual(activeSegmentsAtTime(segments, Number.NaN), []);
  assert.equal(isSegmentActive(segments[0], 2), false);
});
