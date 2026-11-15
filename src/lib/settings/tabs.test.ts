// The list the Settings window's tab strip, tray navigation and URL all go by (SAGA-379).
import { expect, test } from "vitest";
import { SETTINGS_TABS } from "./tabs";

test("the tabs are Dictate, Transcribe and Settings, in that order, with unique ids", () => {
  const ids = SETTINGS_TABS.map((tab) => tab.id);
  expect(ids).toEqual(["dictate", "transcribe", "settings"]);
  expect(new Set(ids).size).toBe(ids.length);
  expect(SETTINGS_TABS.map((tab) => tab.label)).toEqual(["Dictate", "Transcribe", "Settings"]);
});

test("only Dictate is created when its tab opens; the others stay mounted", () => {
  expect(SETTINGS_TABS.filter((tab) => !tab.keepMounted).map((tab) => tab.id)).toEqual(["dictate"]);
});
