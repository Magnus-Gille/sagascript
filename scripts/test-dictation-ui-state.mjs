import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const dictationStateSource = await readFile(
  new URL("../src/lib/dictation-ui-state.ts", import.meta.url),
  "utf8",
);
const dictationStateModule = ts.transpileModule(dictationStateSource, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { dictateButtonAction, retainTestRecordingOwnership } = await import(
  `data:text/javascript;base64,${Buffer.from(dictationStateModule).toString("base64")}`
);

test("an external hotkey recording cannot be stopped by the Dictate test button", () => {
  assert.equal(dictateButtonAction("recording", false), "blocked");
  assert.equal(retainTestRecordingOwnership("recording", false), false);
});

test("a recording started by the Dictate test button remains stoppable there", () => {
  assert.equal(dictateButtonAction("recording", true), "stop");
  assert.equal(retainTestRecordingOwnership("recording", true), true);
});

test("only idle can start and leaving recording clears test ownership", () => {
  assert.equal(dictateButtonAction("idle", false), "start");
  assert.equal(dictateButtonAction("transcribing", false), "blocked");
  assert.equal(dictateButtonAction("loading_model", false), "blocked");
  assert.equal(retainTestRecordingOwnership("transcribing", true), false);
  assert.equal(retainTestRecordingOwnership("idle", true), false);
});
