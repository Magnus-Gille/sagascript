import assert from "node:assert/strict";
import test from "node:test";

import {
  canUseBareHotkey,
  hotkeyKeyLabel,
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
  assert.equal(tauriKeyName("f13"), "F13");
  assert.equal(tauriKeyName(" F24 "), "F24");
  assert.equal(tauriKeyName("F١٣"), null);
});

test("the bare-key range shown in Settings matches registration support", () => {
  assert.equal(supportedBareFunctionKeyRange("macos"), "F13–F24");
  assert.equal(supportedBareFunctionKeyRange("windows"), "F13–F24");
  assert.equal(supportedBareFunctionKeyRange("linux"), null);
});

test("malformed extended function keys never qualify as bare shortcuts", () => {
  for (const key of ["F013", "F0013", "F13a", "F25", "F-13", "13"]) {
    assert.equal(canUseBareHotkey(key, "macos"), false, key);
  }
  assert.equal(canUseBareHotkey(" F13 ", "macos"), true);
});

test("the ISO section key is identified by its physical code, not its printed character", () => {
  // Swedish/UK Apple ISO keyboards print "§" (Shift: "±"); German prints "^", French "@".
  for (const key of ["§", "±", "^", "@", "<", ">", "\\", "Dead"]) {
    assert.equal(tauriKeyName(key, "IntlBackslash", "macos"), "IntlBackslash", key);
  }
  assert.equal(tauriKeyName("§"), null);
  assert.equal(tauriKeyName("§", "Backquote"), null);
  assert.equal(tauriKeyName("a", "IntlBackslash", "windows"), "IntlBackslash");
  assert.equal(tauriKeyName("a", "KeyA"), "A");
  assert.equal(canUseBareHotkey("IntlBackslash", "macos"), false);
});

test("the section key is labelled with its Apple keycap on macOS", () => {
  assert.equal(hotkeyKeyLabel("IntlBackslash", "macos"), "§");
  assert.equal(hotkeyKeyLabel("IntlBackslash", "windows"), "IntlBackslash");
  assert.equal(hotkeyKeyLabel("Space", "macos"), "Space");
  assert.equal(hotkeyKeyLabel("F13", "linux"), "F13");
});

test("ISO physical keys fail closed on Linux and before platform detection", () => {
  for (const platform of ["linux", null, undefined, "unknown"]) {
    for (const key of ["§", "<", "a", "\\", " "]) {
      assert.equal(tauriKeyName(key, "IntlBackslash", platform), null,
        `${platform}: ${key}`);
    }
    assert.equal(tauriKeyName("a", "KeyA", platform), "A");
    assert.equal(tauriKeyName(" ", "Space", platform), "Space");
  }
});

test("ISO physical keys remain supported on macOS and Windows", () => {
  for (const platform of ["macos", "windows"]) {
    for (const key of ["§", "<", "a", "\\", "Dead", " "]) {
      assert.equal(tauriKeyName(key, "IntlBackslash", platform), "IntlBackslash");
    }
  }
});
