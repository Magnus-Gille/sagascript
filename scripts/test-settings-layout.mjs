import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const content = readFileSync(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const onboardingContent = readFileSync(new URL("../src/lib/Onboarding.svelte", import.meta.url), "utf8");

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

test("recording shortcuts use clear mode names and independent helpers", () => {
  assert.match(content, /label: "Hold to record"/);
  assert.match(content, /label: "Press to start\/stop"/);
  assert.match(content, /Hold the shortcut while speaking\. Release to stop\./);
  assert.match(content, /Press the shortcut to start recording\. Press it again to stop\./);
  assert.match(content, /shortcutControls[\s\S]*shortcutControl\.helper/);
  assert.match(content, /Each shortcut above is independent and either or both may be configured\./);
  assert.doesNotMatch(content, />Push to talk</);
  assert.doesNotMatch(content, />Toggle</);
});

test("onboarding explains both recording modes", () => {
  assert.match(onboardingContent, /Configure separate shortcuts for these recording modes in Dictate\./);
  assert.match(onboardingContent, /Hold to record/);
  assert.match(onboardingContent, /Press to start\/stop/);
  assert.match(onboardingContent, /Hold the shortcut while speaking\. Release to stop\./);
  assert.match(onboardingContent, /Press the shortcut to start recording\. Press it again to stop\./);
  assert.doesNotMatch(onboardingContent, /Hold to record, release to transcribe/);
});
