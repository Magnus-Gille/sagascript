import { expect, test } from "vitest";
import { dictateButtonAction, retainTestRecordingOwnership } from "./dictation-ui-state";

test("an external hotkey recording cannot be stopped by the Dictate test button", () => {
  expect(dictateButtonAction("recording", false)).toBe("blocked");
  expect(retainTestRecordingOwnership("recording", false)).toBe(false);
});

test("a recording started by the Dictate test button remains stoppable there", () => {
  expect(dictateButtonAction("recording", true)).toBe("stop");
  expect(retainTestRecordingOwnership("recording", true)).toBe(true);
});

test("only idle can start and leaving recording clears test ownership", () => {
  expect(dictateButtonAction("idle", false)).toBe("start");
  expect(dictateButtonAction("transcribing", false)).toBe("blocked");
  expect(dictateButtonAction("loading_model", false)).toBe("blocked");
  expect(retainTestRecordingOwnership("transcribing", true)).toBe(false);
  expect(retainTestRecordingOwnership("idle", true)).toBe(false);
});
