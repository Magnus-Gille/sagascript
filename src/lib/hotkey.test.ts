import { expect, test } from "vitest";
import {
  brightnessKeyHotkeyWarning,
  canUseBareHotkey,
  formatHotkeyDisplay,
  hotkeyKeyLabel,
  supportedBareFunctionKeyRange,
  tauriKeyName,
} from "./hotkey";

test("shortcut display treats modifier aliases as whole tokens", () => {
  expect(formatHotkeyDisplay("CommandOrControl+IntlBackslash", "macos")).toBe("Cmd + §");
  expect(formatHotkeyDisplay("CommandOrControl+IntlBackslash", "windows")).toBe("Ctrl + IntlBackslash");
  expect(formatHotkeyDisplay("CmdOrCtrl+Meta+Option+Shift+A", "linux")).toBe("Ctrl + Win + Alt + Shift + A");
  expect(formatHotkeyDisplay("Control+Command+Space", "macos")).toBe("Control + Cmd + Space");
});

test("F13 through F24 may omit modifiers on macOS", () => {
  for (const key of ["F13", "F14", "F19", "F20", "F21", "F24"]) {
    expect(canUseBareHotkey(key, "macos"), key).toBe(true);
  }

  for (const key of ["F1", "F12", "F25", "Space", "A"]) {
    expect(canUseBareHotkey(key, "macos"), key).toBe(false);
  }
});

test("Windows may use bare F13 through F24 while Linux rejects unsupported mappings", () => {
  expect(canUseBareHotkey("F13", "windows")).toBe(true);
  expect(canUseBareHotkey("F24", "windows")).toBe(true);
  expect(canUseBareHotkey("F25", "windows")).toBe(false);
  expect(canUseBareHotkey("F13", "linux")).toBe(false);
  expect(canUseBareHotkey("F24", "linux")).toBe(false);
});

test("the keyboard event mapper accepts exactly F1 through F24", () => {
  expect(tauriKeyName("F1")).toBe("F1");
  expect(tauriKeyName("F13")).toBe("F13");
  expect(tauriKeyName("F24")).toBe("F24");
  expect(tauriKeyName("F0")).toBe(null);
  expect(tauriKeyName("F25")).toBe(null);
  expect(tauriKeyName("F99")).toBe(null);
  expect(tauriKeyName("f13")).toBe("F13");
  expect(tauriKeyName(" F24 ")).toBe("F24");
  expect(tauriKeyName("F١٣")).toBe(null);
});

test("the bare-key range shown in Settings matches registration support", () => {
  expect(supportedBareFunctionKeyRange("macos")).toBe("F13–F24");
  expect(supportedBareFunctionKeyRange("windows")).toBe("F13–F24");
  expect(supportedBareFunctionKeyRange("linux")).toBe(null);
});

test("malformed extended function keys never qualify as bare shortcuts", () => {
  for (const key of ["F013", "F0013", "F13a", "F25", "F-13", "13"]) {
    expect(canUseBareHotkey(key, "macos"), key).toBe(false);
  }
  expect(canUseBareHotkey(" F13 ", "macos")).toBe(true);
});

test("the ISO section key is identified by its physical code, not its printed character", () => {
  // Swedish/UK Apple ISO keyboards print "§" (Shift: "±"); German prints "^", French "@".
  for (const key of ["§", "±", "^", "@", "<", ">", "\\", "Dead"]) {
    expect(tauriKeyName(key, "IntlBackslash", "macos"), key).toBe("IntlBackslash");
  }
  expect(tauriKeyName("§")).toBe(null);
  expect(tauriKeyName("§", "Backquote")).toBe(null);
  expect(tauriKeyName("a", "IntlBackslash", "windows")).toBe("IntlBackslash");
  expect(tauriKeyName("a", "KeyA")).toBe("A");
  expect(canUseBareHotkey("IntlBackslash", "macos")).toBe(false);
});

test("the section key is labelled with its Apple keycap on macOS", () => {
  expect(hotkeyKeyLabel("IntlBackslash", "macos")).toBe("§");
  expect(hotkeyKeyLabel("IntlBackslash", "windows")).toBe("IntlBackslash");
  expect(hotkeyKeyLabel("Space", "macos")).toBe("Space");
  expect(hotkeyKeyLabel("F13", "linux")).toBe("F13");
});

test("ISO physical keys fail closed on Linux and before platform detection", () => {
  for (const platform of ["linux", null, undefined, "unknown"]) {
    for (const key of ["§", "<", "a", "\\", " "]) {
      expect(tauriKeyName(key, "IntlBackslash", platform), `${platform}: ${key}`).toBe(null);
    }
    expect(tauriKeyName("a", "KeyA", platform)).toBe("A");
    expect(tauriKeyName(" ", "Space", platform)).toBe("Space");
  }
});

test("ISO physical keys remain supported on macOS and Windows", () => {
  for (const platform of ["macos", "windows"]) {
    for (const key of ["§", "<", "a", "\\", "Dead", " "]) {
      expect(tauriKeyName(key, "IntlBackslash", platform)).toBe("IntlBackslash");
    }
  }
});

test("bare F14 and F15 warn about brightness keys on macOS only", () => {
  for (const key of ["F14", "f15"]) {
    expect(brightnessKeyHotkeyWarning(key, "macos")).toMatch(/brightness/);
    expect(brightnessKeyHotkeyWarning(key, "windows")).toBe(null);
  }
  for (const key of ["F13", "F16", "Shift+F14", "F140"]) {
    expect(brightnessKeyHotkeyWarning(key, "macos"), key).toBe(null);
  }
});
