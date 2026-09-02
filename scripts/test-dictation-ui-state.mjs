import assert from "node:assert/strict";
import test from "node:test";

import {
  dictateButtonAction,
  retainTestRecordingOwnership,
} from "../src/lib/dictation-ui-state.ts";

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
