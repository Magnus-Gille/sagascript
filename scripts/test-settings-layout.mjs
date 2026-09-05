import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const content = readFileSync(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");

test("Settings presents build identity above the ordered navigation tabs", () => {
  const settingsWindow = '<div class="settings-window">';
  const header = '<header class="window-header">';
  const tabs = '<div class="tabs">';
  assert.equal(content.split(header).length - 1, 1, "Exactly one build header");
  assert.equal(content.split(tabs).length - 1, 1, "Exactly one tab bar");
  const windowStart = content.indexOf(settingsWindow);
  assert(windowStart >= 0, "Settings window exists");
  const headerStart = content.indexOf(header, windowStart);
  const tabsStart = content.indexOf(tabs, windowStart);
  assert(headerStart >= 0 && tabsStart >= 0, "Header and tabs follow the window opening");
  const headerEnd = content.indexOf("</header>", headerStart);
  assert(headerEnd > headerStart && headerEnd < tabsStart, "Header must precede tabs");
  const headerContent = content.slice(headerStart, headerEnd);
  for (const field of ["version", "git_hash", "build_date"]) {
    assert(headerContent.includes(`{buildInfo.${field}}`), `Header contains ${field}`);
  }
  const tabsEnd = content.indexOf("</div>", tabsStart);
  assert(tabsEnd > tabsStart, "Tab bar closes");
  const tabsContent = content.slice(tabsStart, tabsEnd);
  const indices = ["Dictate", "Transcribe", "Settings"].map((label) => tabsContent.indexOf(label));
  assert(indices.every((index) => index >= 0), "All tab labels remain");
  assert(indices[0] < indices[1] && indices[1] < indices[2], "Tab order remains unchanged");
});
