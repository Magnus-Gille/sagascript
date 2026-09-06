import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const settings = readFileSync(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const presenterSettings = readFileSync(new URL("../src/lib/PresenterSettings.svelte", import.meta.url), "utf8");

test("Dictate renders fixed presenter status feedback", () => {
  assert.match(settings, /listen\("presenter-status"/);
  assert.match(settings, /let presenterStatus: PresenterStatus \| null = \$state\(null\)/);
  assert.match(settings, /Submit key sent; delivery not confirmed/);
  assert.match(settings, /Not sent — copy recognized text from Dictate/);
  assert.match(settings, /Submit may have been sent — check destination before retrying/);
  assert.match(settings, /No speech detected/);
});

test("Presenter UI uses manual-copy wording off macOS", () => {
  assert.match(presenterSettings, /Manual copy on this platform/);
  assert.doesNotMatch(presenterSettings, /Insert-only on this platform/);
});
