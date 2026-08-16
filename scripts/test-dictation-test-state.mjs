import assert from "node:assert/strict";
import test from "node:test";

import { dictationTestStateForEvent } from "../src/lib/dictation-test-state.js";

const busyState = { recording: false, transcribing: false, error: "Already busy" };

test("global recording makes the Dictate test control a stop action", () => {
  assert.deepEqual(dictationTestStateForEvent(busyState, "recording"), {
    recording: true,
    transcribing: false,
    error: "",
  });
});

test("transcription and model loading stop recording without losing an error", () => {
  for (const event of ["transcribing", "loading_model"]) {
    assert.deepEqual(dictationTestStateForEvent(busyState, event), {
      recording: false,
      transcribing: true,
      error: "Already busy",
    });
  }
});

test("idle clears the Dictate test activity after every terminal outcome", () => {
  assert.deepEqual(
    dictationTestStateForEvent({ recording: true, transcribing: false, error: "" }, "idle"),
    { recording: false, transcribing: false, error: "" },
  );
});

test("unrelated lifecycle events leave the Dictate test controls untouched", () => {
  assert.equal(dictationTestStateForEvent(busyState, "settings_reloaded"), busyState);
  assert.equal(dictationTestStateForEvent(busyState, "unknown"), busyState);
});
