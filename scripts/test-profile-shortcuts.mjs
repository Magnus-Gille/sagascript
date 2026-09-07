import assert from "node:assert/strict";
import test from "node:test";

import {
  allProfileShortcutValues,
  canonicalShortcut,
  displayProfileShortcut,
  profileWithShortcut,
  suggestedProfileShortcuts,
} from "../src/lib/profile-shortcuts.js";

const legacyProfile = {
  id: "en",
  name: "English",
  shortcut: "Control+Option+E",
  language: "en",
};

test("legacy profiles display the old binding in the matching mode only", () => {
  assert.equal(displayProfileShortcut(legacyProfile, "push_to_talk_shortcut", "push"), "Control+Option+E");
  assert.equal(displayProfileShortcut(legacyProfile, "toggle_shortcut", "push"), "");
  assert.equal(displayProfileShortcut(legacyProfile, "toggle_shortcut", "toggle"), "Control+Option+E");
  assert.equal(legacyProfile.push_to_talk_shortcut, undefined, "display must not migrate or auto-save");

  const toggleOnly = { ...legacyProfile, push_to_talk_shortcut: null, toggle_shortcut: "Super+T", shortcut: "Super+T" };
  assert.equal(displayProfileShortcut(toggleOnly, "push_to_talk_shortcut", "push"), "");
  assert.equal(displayProfileShortcut(toggleOnly, "toggle_shortcut", "push"), "Super+T");
});

test("explicit profile bindings replace legacy routing and keep the compatibility field synchronized", () => {
  const withPush = profileWithShortcut(legacyProfile, "push_to_talk_shortcut", "Super+S");
  assert.deepEqual(allProfileShortcutValues([withPush], "toggle"), ["Super+S"]);
  assert.equal(withPush.shortcut, "Super+S");

  const withBoth = profileWithShortcut(withPush, "toggle_shortcut", "Shift+Super+S");
  assert.deepEqual(allProfileShortcutValues([withBoth], "push"), ["Super+S", "Shift+Super+S"]);
  assert.equal(displayProfileShortcut(withBoth, "push_to_talk_shortcut", "push"), "Super+S");
  assert.equal(displayProfileShortcut(withBoth, "toggle_shortcut", "toggle"), "Shift+Super+S");
  assert.equal(withBoth.shortcut, "Super+S");

  const cleared = profileWithShortcut(withBoth, "push_to_talk_shortcut", null);
  assert.equal(cleared.shortcut, "Shift+Super+S");
  assert.deepEqual(allProfileShortcutValues([cleared], "push"), ["Shift+Super+S"]);
});

test("adding a second explicit binding preserves the legacy slot and mode", () => {
  const legacyPush = profileWithShortcut(legacyProfile, "toggle_shortcut", "Shift+Super+S", "push");
  assert.equal(legacyPush.push_to_talk_shortcut, legacyProfile.shortcut);
  assert.equal(legacyPush.toggle_shortcut, "Shift+Super+S");
  assert.equal(legacyPush.shortcut, legacyProfile.shortcut);
  assert.deepEqual(allProfileShortcutValues([legacyPush], "toggle"), [legacyProfile.shortcut, "Shift+Super+S"]);

  const legacyToggle = profileWithShortcut(legacyProfile, "push_to_talk_shortcut", "Super+S", "toggle");
  assert.equal(legacyToggle.toggle_shortcut, legacyProfile.shortcut);
  assert.equal(legacyToggle.push_to_talk_shortcut, "Super+S");
  assert.equal(legacyToggle.shortcut, "Super+S");
});

test("new profile suggestions avoid legacy and customized bindings", () => {
  const existing = [
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
  assert.equal(suggestion.push_to_talk_shortcut, "Super+E");
  assert.equal(suggestion.toggle_shortcut, "Shift+Super+E");
  assert.equal(suggestion.shortcut, "Super+E");
  assert.equal(canonicalShortcut(suggestion.push_to_talk_shortcut), "super+keye");
});

test("suggestions use the Windows modifier spelling outside macOS", () => {
  const suggestion = suggestedProfileShortcuts([], "Control+Option+Shift+F12", "windows");
  assert.equal(suggestion.push_to_talk_shortcut, "Control+S");
  assert.equal(suggestion.toggle_shortcut, "Control+Shift+S");
});

test("exhausted suggestions use null slots and normalize the untouched PTT slot", () => {
  const occupied = [
    { shortcut: "Super+S", push_to_talk_shortcut: "Super+S", toggle_shortcut: "Shift+Super+S" },
    { shortcut: "Super+E", push_to_talk_shortcut: "Super+E", toggle_shortcut: "Shift+Super+E" },
  ];
  const suggestion = suggestedProfileShortcuts(occupied, "Control+Option+F12", "macos");
  assert.equal(suggestion.push_to_talk_shortcut, null);
  assert.equal(suggestion.toggle_shortcut, null);

  const draft = profileWithShortcut(
    { ...suggestion, id: "third", name: "Third", language: "en" },
    "push_to_talk_shortcut",
    "Super+N",
  );
  assert.equal(draft.push_to_talk_shortcut, "Super+N");
  assert.equal(draft.toggle_shortcut, null);
  assert.equal(draft.shortcut, "Super+N");
});

test("exhausted suggestions normalize the untouched toggle slot", () => {
  const occupied = [
    { shortcut: "Super+S", push_to_talk_shortcut: "Super+S", toggle_shortcut: "Shift+Super+S" },
    { shortcut: "Super+E", push_to_talk_shortcut: "Super+E", toggle_shortcut: "Shift+Super+E" },
  ];
  const suggestion = suggestedProfileShortcuts(occupied, "Control+Option+F12", "macos");
  const draft = profileWithShortcut(
    { ...suggestion, id: "third", name: "Third", language: "en" },
    "toggle_shortcut",
    "Shift+Super+N",
  );
  assert.equal(draft.push_to_talk_shortcut, null);
  assert.equal(draft.toggle_shortcut, "Shift+Super+N");
  assert.equal(draft.shortcut, "Shift+Super+N");
});
