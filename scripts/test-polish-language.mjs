import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = async (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");
const [api, onboarding, settings, overlay] = await Promise.all([
  source("src/lib/api.ts"),
  source("src/lib/Onboarding.svelte"),
  source("src/lib/Settings.svelte"),
  source("src/lib/Overlay.svelte"),
]);

test("Polish is exposed consistently in the language selectors", () => {
  assert.match(api, /export type Language = "en" \| "sv" \| "no" \| "fi" \| "pl" \| "auto";/);
  assert.match(onboarding, /type OnboardingLanguage = "en" \| "sv" \| "no" \| "fi" \| "pl";/);
  assert.match(onboarding, /selectedLanguage === "pl"/);
  assert.match(onboarding, /<span class="lang-flag">PL<\/span>/);
  assert.match(onboarding, /<span class="lang-name">Polski<\/span>/);
  assert.match(onboarding, /pl: "142 MB"/);
  assert.match(settings, /<option value="pl">Polish<\/option>/);
  assert.match(settings, /case "pl": return "Polish";/);
  assert.match(overlay, /pl: "Polish"/);
});

test("Polish keeps the generic multilingual Base recommendation", () => {
  assert.match(onboarding, /pl: "base"/);
  assert.doesNotMatch(onboarding, /polish.*model|model.*polish/i);
  assert.match(settings, /getModelInfo\(\)/);
  assert.match(settings, /getEffectiveModelInfo\(profile\.language\)/);
  assert.match(settings, /\{#each models as model\}/);
  assert.doesNotMatch(settings, /const\s+polishModels\s*=/);
});

test("five onboarding languages fit through a responsive grid", () => {
  assert.match(onboarding, /\.content\s*\{[\s\S]*min-width:\s*0;/);
  assert.match(onboarding, /\.step\s*\{[\s\S]*width:\s*100%;[\s\S]*max-width:\s*380px;/);
  assert.match(onboarding, /\.step\s*\{[\s\S]*min-width:\s*0;/);
  assert.match(onboarding, /\.language-options\s*\{[\s\S]*display:\s*grid;[\s\S]*grid-template-columns:\s*repeat\(5,\s*minmax\(0,\s*1fr\)\)/);
  assert.match(onboarding, /\.language-option\s*\{[\s\S]*min-width:\s*0;[\s\S]*width:\s*100%;/);
  assert.equal((onboarding.match(/aria-pressed=\{selectedLanguage ===/g) ?? []).length, 5);
});

test("existing language choices remain present", () => {
  for (const label of ["English", "Svenska", "Norsk", "Suomi", "Polski"]) {
    assert.match(onboarding, new RegExp(`<span class="lang-name">${label}<\\/span>`));
  }
  for (const value of ["en", "sv", "no", "fi", "pl", "auto"]) {
    assert.match(settings, new RegExp(`<option value="${value}"`));
  }
});
