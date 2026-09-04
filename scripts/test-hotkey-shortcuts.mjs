import assert from "node:assert/strict";
import test from "node:test";

import {
  canUseBareHotkey,
  supportedBareFunctionKeyRange,
  tauriKeyName,
} from "../src/lib/hotkey.js";

test("F13 through F24 may omit modifiers on macOS", () => {
  for (const key of ["F13", "F14", "F19", "F20", "F21", "F24"]) {
    assert.equal(canUseBareHotkey(key, "macos"), true, key);
  }

  for (const key of ["F1", "F12", "F25", "Space", "A"]) {
    assert.equal(canUseBareHotkey(key, "macos"), false, key);
  }
});

test("Windows may use bare F13 through F24 while Linux rejects unsupported mappings", () => {
  assert.equal(canUseBareHotkey("F13", "windows"), true);
  assert.equal(canUseBareHotkey("F24", "windows"), true);
  assert.equal(canUseBareHotkey("F25", "windows"), false);
  assert.equal(canUseBareHotkey("F13", "linux"), false);
  assert.equal(canUseBareHotkey("F24", "linux"), false);
});

test("the keyboard event mapper accepts exactly F1 through F24", () => {
  assert.equal(tauriKeyName("F1"), "F1");
  assert.equal(tauriKeyName("F13"), "F13");
  assert.equal(tauriKeyName("F24"), "F24");
  assert.equal(tauriKeyName("F0"), null);
  assert.equal(tauriKeyName("F25"), null);
  assert.equal(tauriKeyName("F99"), null);
  assert.equal(tauriKeyName("f13"), null);
  assert.equal(tauriKeyName("F١٣"), null);
});

test("the bare-key range shown in Settings matches registration support", () => {
  assert.equal(supportedBareFunctionKeyRange("macos"), "F13–F24");
  assert.equal(supportedBareFunctionKeyRange("windows"), "F13–F24");
  assert.equal(supportedBareFunctionKeyRange("linux"), null);
});

test("malformed extended function keys never qualify as bare shortcuts", () => {
  for (const key of ["F013", "F13a", "F25", "F-13", "13", " F13 "]) {
    assert.equal(canUseBareHotkey(key, "macos"), false, key);
  }
});
