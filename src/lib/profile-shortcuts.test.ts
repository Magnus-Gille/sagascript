import { expect, test } from "vitest";
import type { HotkeyProfile } from "./api";
import {
  allProfileShortcutValues,
  canonicalShortcut,
  displayProfileShortcut,
  profileWithShortcut,
  suggestedProfileShortcuts,
} from "./profile-shortcuts";

const legacyProfile: HotkeyProfile = {
  id: "en",
  name: "English",
  shortcut: "Control+Option+E",
  language: "en",
};

test("legacy profiles display the old binding in the matching mode only", () => {
  expect(displayProfileShortcut(legacyProfile, "push_to_talk_shortcut", "push")).toBe("Control+Option+E");
  expect(displayProfileShortcut(legacyProfile, "toggle_shortcut", "push")).toBe("");
  expect(displayProfileShortcut(legacyProfile, "toggle_shortcut", "toggle")).toBe("Control+Option+E");
  expect(legacyProfile.push_to_talk_shortcut, "display must not migrate or auto-save").toBe(undefined);

  const toggleOnly = { ...legacyProfile, push_to_talk_shortcut: null, toggle_shortcut: "Super+T", shortcut: "Super+T" };
  expect(displayProfileShortcut(toggleOnly, "push_to_talk_shortcut", "push")).toBe("");
  expect(displayProfileShortcut(toggleOnly, "toggle_shortcut", "push")).toBe("Super+T");
});

test("explicit profile bindings replace legacy routing and keep the compatibility field synchronized", () => {
  const withPush = profileWithShortcut(legacyProfile, "push_to_talk_shortcut", "Super+S");
  expect(allProfileShortcutValues([withPush], "toggle")).toStrictEqual(["Super+S"]);
  expect(withPush.shortcut).toBe("Super+S");

  const withBoth = profileWithShortcut(withPush, "toggle_shortcut", "Shift+Super+S");
  expect(allProfileShortcutValues([withBoth], "push")).toStrictEqual(["Super+S", "Shift+Super+S"]);
  expect(displayProfileShortcut(withBoth, "push_to_talk_shortcut", "push")).toBe("Super+S");
  expect(displayProfileShortcut(withBoth, "toggle_shortcut", "toggle")).toBe("Shift+Super+S");
  expect(withBoth.shortcut).toBe("Super+S");

  const cleared = profileWithShortcut(withBoth, "push_to_talk_shortcut", null);
  expect(cleared.shortcut).toBe("Shift+Super+S");
  expect(allProfileShortcutValues([cleared], "push")).toStrictEqual(["Shift+Super+S"]);
});

test("adding a second explicit binding preserves the legacy slot and mode", () => {
  const legacyPush = profileWithShortcut(legacyProfile, "toggle_shortcut", "Shift+Super+S", "push");
  expect(legacyPush.push_to_talk_shortcut).toBe(legacyProfile.shortcut);
  expect(legacyPush.toggle_shortcut).toBe("Shift+Super+S");
  expect(legacyPush.shortcut).toBe(legacyProfile.shortcut);
  expect(allProfileShortcutValues([legacyPush], "toggle")).toStrictEqual([legacyProfile.shortcut, "Shift+Super+S"]);

  const legacyToggle = profileWithShortcut(legacyProfile, "push_to_talk_shortcut", "Super+S", "toggle");
  expect(legacyToggle.toggle_shortcut).toBe(legacyProfile.shortcut);
  expect(legacyToggle.push_to_talk_shortcut).toBe("Super+S");
  expect(legacyToggle.shortcut).toBe("Super+S");
});

test("new profile suggestions avoid legacy and customized bindings", () => {
  const existing: HotkeyProfile[] = [
    legacyProfile,
    {
      id: "sv",
      name: "Swedish",
      shortcut: "Super+S",
      push_to_talk_shortcut: "Super+S",
      toggle_shortcut: "Shift+Super+S",
      language: "sv",
    },
  ];
  const suggestion = suggestedProfileShortcuts(existing, "Control+Option+F12", "macos");
  expect(suggestion.push_to_talk_shortcut).toBe("Super+E");
  expect(suggestion.toggle_shortcut).toBe("Shift+Super+E");
  expect(suggestion.shortcut).toBe("Super+E");
  expect(canonicalShortcut(suggestion.push_to_talk_shortcut)).toBe("super+keye");
});

test("suggestions use the Windows modifier spelling outside macOS", () => {
  const suggestion = suggestedProfileShortcuts([], "Control+Option+Shift+F12", "windows");
  expect(suggestion.push_to_talk_shortcut).toBe("Control+S");
  expect(suggestion.toggle_shortcut).toBe("Control+Shift+S");
});

test("exhausted suggestions use null slots and normalize the untouched PTT slot", () => {
  const occupied = [
    { shortcut: "Super+S", push_to_talk_shortcut: "Super+S", toggle_shortcut: "Shift+Super+S" },
    { shortcut: "Super+E", push_to_talk_shortcut: "Super+E", toggle_shortcut: "Shift+Super+E" },
  ];
  const suggestion = suggestedProfileShortcuts(occupied, "Control+Option+F12", "macos");
  expect(suggestion.push_to_talk_shortcut).toBe(null);
  expect(suggestion.toggle_shortcut).toBe(null);

  const draft = profileWithShortcut({ ...suggestion, id: "third", name: "Third", language: "en" }, "push_to_talk_shortcut", "Super+N");
  expect(draft.push_to_talk_shortcut).toBe("Super+N");
  expect(draft.toggle_shortcut).toBe(null);
  expect(draft.shortcut).toBe("Super+N");
});

test("exhausted suggestions normalize the untouched toggle slot", () => {
  const occupied = [
    { shortcut: "Super+S", push_to_talk_shortcut: "Super+S", toggle_shortcut: "Shift+Super+S" },
    { shortcut: "Super+E", push_to_talk_shortcut: "Super+E", toggle_shortcut: "Shift+Super+E" },
  ];
  const suggestion = suggestedProfileShortcuts(occupied, "Control+Option+F12", "macos");
  const draft = profileWithShortcut({ ...suggestion, id: "third", name: "Third", language: "en" }, "toggle_shortcut", "Shift+Super+N");
  expect(draft.push_to_talk_shortcut).toBe(null);
  expect(draft.toggle_shortcut).toBe("Shift+Super+N");
  expect(draft.shortcut).toBe("Shift+Super+N");
});
