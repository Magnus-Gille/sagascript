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
  assert.match(content, />\+ Add profile<\/button>/);
  assert.doesNotMatch(content, />\+ Add language<\/button>/);
  assert.match(content, /label: "Hold to record"/);
  assert.match(content, /label: "Press to start\/stop"/);
  assert.match(content, /Hold the shortcut while speaking\. Release to stop\./);
  assert.match(content, /Press the shortcut to start recording\. Press it again to stop\./);
  assert.match(content, /shortcutControls[\s\S]*shortcutControl\.helper/);
  assert.match(content, /Each shortcut above is independent and either or both may be configured\./);
  assert.doesNotMatch(content, />Push to talk</);
  assert.doesNotMatch(content, />Toggle</);
});

test("Dictate offers a model per profile instead of a global dictation model picker", () => {
  const dictateStart = content.indexOf('{#if activeTab === "dictate"}');
  const dictateEnd = content.indexOf('{#if activeTab === "settings"}', dictateStart);
  assert.ok(dictateStart >= 0 && dictateEnd > dictateStart, "Dictate section is present");
  const dictateSource = content.slice(dictateStart, dictateEnd);
  assert.match(dictateSource, /Speech model for \{profile\.name\}/);
  assert.match(dictateSource, /profileModelOptions\[profile\.id\]/);
  assert.match(content, /getProfileModelInfo/);
  assert.doesNotMatch(dictateSource, /Pianissimo is experimental and applies to every Swedish dictation shortcut/);
});

test("onboarding explains both recording modes", () => {
  assert.match(onboardingContent, /Configure separate shortcuts for these recording modes in Dictate\./);
  assert.match(onboardingContent, /Hold to record/);
  assert.match(onboardingContent, /Press to start\/stop/);
  assert.match(onboardingContent, /Hold the shortcut while speaking\. Release to stop\./);
  assert.match(onboardingContent, /Press the shortcut to start recording\. Press it again to stop\./);
  assert.doesNotMatch(onboardingContent, /Hold to record, release to transcribe/);
});

test("Settings shows engine host identity and Pianissimo engine controls without outdated strings", () => {
  const header = content.slice(content.indexOf('<header class="window-header">'), content.indexOf("</header>"));
  assert.match(header, /engineHostIdentity/);
  assert.match(content, /getEngineStatus/);
  for (const text of ["Prepare Pianissimo when", "When I press the dictation key", "When Sagascript starts",
    "Only when needed", "Unload after idle", "Requires macOS 14 or later on Apple Silicon"]) {
    assert(content.includes(text), `Settings contains "${text}"`);
  }
  assert.match(content, /engineIdleChoices = \[5, 10, 30, 60, 0\]/);
  assert.match(content, /setEnginePrewarm/);
  assert.match(content, /setEngineIdleUnloadMinutes/);
  assert.doesNotMatch(content, /714|Pianissimo Q8|macOS 13/);
});
