import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const settingsSource = await readFile(
  new URL("../src/lib/Settings.svelte", import.meta.url),
  "utf8",
);
const onboardingSource = await readFile(
  new URL("../src/lib/Onboarding.svelte", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);
const mainSource = await readFile(
  new URL("../src-tauri/src/main.rs", import.meta.url),
  "utf8",
);
const brandMarkSource = await readFile(
  new URL("../assets/brand/sagascript-mark.svg", import.meta.url),
  "utf8",
);
const appIconSource = await readFile(
  new URL("../assets/brand/sagascript-app-icon.svg", import.meta.url),
  "utf8",
);
const faviconSource = await readFile(
  new URL("../site/public/favicon.svg", import.meta.url),
  "utf8",
);
const siteMarkSource = await readFile(
  new URL("../site/public/sagascript-mark.svg", import.meta.url),
  "utf8",
);
const siteCssSource = await readFile(
  new URL("../site/app/globals.css", import.meta.url),
  "utf8",
);

test("release navigation exposes only Dictate, Transcribe, and Settings", () => {
  for (const label of ["Dictate", "Transcribe", "Settings"]) {
    assert.match(settingsSource, new RegExp(`>\\s*${label}\\s*<`));
  }
  assert.doesNotMatch(settingsSource, />\s*Teach\s*</);
  assert.doesNotMatch(settingsSource, /activeTab\s*===\s*["']teach["']/);
});

test("manual model and decoder controls stay behind Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const advancedEnd = settingsSource.indexOf("</details>", advancedStart);
  assert.ok(advancedStart >= 0, "Advanced disclosure is missing");
  assert.ok(advancedEnd > advancedStart, "Advanced disclosure is not closed");

  const advancedSource = settingsSource.slice(advancedStart, advancedEnd);
  assert.match(advancedSource, /<summary>Advanced<\/summary>/);
  assert.match(advancedSource, /Manual model choice/);
  assert.match(advancedSource, /Decoding mode/);
  assert.match(advancedSource, /Temperature fallback/);
  assert.match(advancedSource, /Voice activity detection/);
});

test("ordinary settings keep the useful controls outside Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const ordinarySource = settingsSource.slice(0, advancedStart);

  assert.match(ordinarySource, /Show recording overlay/);
  assert.match(ordinarySource, /Personal dictionary/);
});

test("onboarding starts with language instead of a disposable welcome step", () => {
  assert.match(onboardingSource, /currentStep:\s*Step\s*=\s*\$state\("language"\)/);
  assert.doesNotMatch(onboardingSource, /type Step = [^\n]*"welcome"/);
  assert.doesNotMatch(onboardingSource, /Welcome to Sagascript/);
});

test("onboarding automatically prepares one hidden recommended engine", () => {
  assert.match(onboardingSource, /nextStep\(\);\s*void startDownload\(\);/s);
  assert.match(onboardingSource, /local speech engine/i);
  assert.doesNotMatch(onboardingSource, /modelInfo\[selectedLanguage\]\.name/);
  assert.match(onboardingSource, /Continue without speech engine/);
  assert.match(onboardingSource, /Speech engine is not installed yet/);
});

test("dictation profiles expose missing speech engines without opening Advanced", () => {
  const advancedStart = settingsSource.indexOf('<details class="advanced-section">');
  const ordinarySource = settingsSource.slice(0, advancedStart);

  assert.match(ordinarySource, /getEffectiveModelInfo/);
  assert.match(ordinarySource, /Download speech engine/);
  assert.match(ordinarySource, /Speech engine ready/);
});

test("onboarding prevents duplicate setup requests and exposes language selection", () => {
  assert.match(onboardingSource, /if \(languageSaving\) return;/);
  assert.match(onboardingSource, /disabled=\{languageSaving\}/);
  assert.equal((onboardingSource.match(/aria-pressed=\{selectedLanguage ===/g) ?? []).length, 3);
});

test("Accessibility onboarding reopens System Settings without prompting again", () => {
  const reopenStart = onboardingSource.indexOf("async function reopenAccessibilitySettings");
  const reopenEnd = onboardingSource.indexOf("\n  }", reopenStart);

  assert.ok(reopenStart >= 0, "Accessibility reopen helper is missing");
  assert.ok(reopenEnd > reopenStart, "Accessibility reopen helper is not closed");

  const reopenSource = onboardingSource.slice(reopenStart, reopenEnd);
  assert.match(reopenSource, /await openAccessibilitySettings\(\)/);
  assert.doesNotMatch(
    reopenSource,
    /requestAccessibilityPermission|beginPollOperation|startPoll/,
  );
  assert.match(
    apiSource,
    /export async function openAccessibilitySettings\(\): Promise<void> \{\s*return invoke\("open_accessibility_settings"\);/,
  );
  assert.match(
    mainSource,
    /commands::open_accessibility_settings,/,
    "Accessibility settings command is not registered with Tauri",
  );
  assert.match(
    onboardingSource,
    /\{:else if accessibilityChecking\}[\s\S]*onclick=\{reopenAccessibilitySettings\}[\s\S]*Open Accessibility Settings Again/,
  );
});

test("menu bar uses the one monochrome S template without a text marker", () => {
  assert.match(mainSource, /include_bytes!\("\.\.\/icons\/tray-icon@2x\.png"\)/);
  assert.match(mainSource, /\.icon_as_template\(true\)/);
  assert.doesNotMatch(mainSource, /\.title\("S"\)/);
});

test("profile and update menu states remain explicit after interaction", () => {
  assert.match(mainSource, /select_profile_menu\(app, &profile\)/);
  assert.match(mainSource, /if selected \{ "✓ " \} else \{ "" \}/);
  assert.match(mainSource, /open_update_release\(&version\)[\s\S]*available_version = None;/);
  assert.match(mainSource, /items\.check\.set_text\("Check Again…"\)/);
});

test("mobile site keeps navigation and readable terminal text", () => {
  assert.doesNotMatch(siteCssSource, /\.site-header nav\s*\{\s*display:\s*none/);
  assert.match(siteCssSource, /\.terminal-bar b[^}]*color:\s*#8f8f88/);
  assert.match(siteCssSource, /\.terminal code span[^}]*color:\s*#8f8f88/);
});

test("app and website use the exact canonical single-S geometry", () => {
  const canonicalPath = brandMarkSource.match(/<path\s+d="([^"]+)"/)?.[1];
  assert.ok(canonicalPath, "Canonical S path is missing");

  for (const [name, source] of [
    ["app icon", appIconSource],
    ["website favicon", faviconSource],
    ["website mark", siteMarkSource],
  ]) {
    assert.ok(
      source.includes(`d="${canonicalPath}"`),
      `${name} has drifted from the canonical S geometry`,
    );
  }
});
