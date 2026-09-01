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
  assert.doesNotMatch(onboardingSource, /Skip for now/);
});

test("onboarding prevents duplicate setup requests and exposes language selection", () => {
  assert.match(onboardingSource, /if \(languageSaving\) return;/);
  assert.match(onboardingSource, /disabled=\{languageSaving\}/);
  assert.equal((onboardingSource.match(/aria-pressed=\{selectedLanguage ===/g) ?? []).length, 3);
});

test("menu bar uses the one monochrome S template without a text marker", () => {
  assert.match(mainSource, /include_bytes!\("\.\.\/icons\/tray-icon\.png"\)/);
  assert.match(mainSource, /\.icon_as_template\(true\)/);
  assert.doesNotMatch(mainSource, /\.title\("S"\)/);
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
