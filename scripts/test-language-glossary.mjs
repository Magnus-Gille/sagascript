import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const settingsSource = await readFile(
  new URL("../src/lib/Settings.svelte", import.meta.url),
  "utf8",
);
const apiSource = await readFile(
  new URL("../src/lib/api.ts", import.meta.url),
  "utf8",
);

test("profile glossary API preserves the validated camelCase command payload", () => {
  assert.match(
    apiSource,
    /export async function setProfileGlossary\(profileId: string, source: string, expectedSource\?: string\): Promise<void> \{\s*return invoke\("set_profile_glossary", \{ profileId, source, expectedSource \}\);/,
  );
  assert.match(
    apiSource,
    /export async function setInitialPrompt\(prompt: string, expectedSource\?: string\): Promise<void> \{\s*return invoke\("set_initial_prompt", \{ prompt, expectedSource \}\);/,
  );
});

test("file transcription carries an optional profile without changing no-profile behavior", () => {
  assert.match(apiSource, /options\?: \{ prompt\?: string; diarize\?: boolean; profileId\?: string \}/);
  assert.match(apiSource, /profileId: options\?\.profileId \?\? null/);
  assert.match(settingsSource, /const profileId = selectedTranscribeProfile\(\)\?\.id/);
  assert.match(settingsSource, /profileId: profileId \?\? undefined/);
  assert.match(settingsSource, /No profile \(use selected language\)/);
});

test("dictionary scope exposes only explicit profiles and keeps migration guidance visible", () => {
  assert.match(settingsSource, /function explicitProfiles\(source: Settings \| null = settings\)/);
  assert.match(settingsSource, /profile\.language !== "auto"/);
  assert.match(settingsSource, /let glossaryScopeId: string = \$state\(""\)/);
  assert.match(settingsSource, /<select id="dictionary-scope" value=\{glossaryScopeId\} onchange=\{onGlossaryScopeChange\}>/);
  assert.match(settingsSource, /<option value="">Global hints<\/option>/);
  assert.match(settingsSource, /Global entries are hint-only and remain stored/);
  assert.match(settingsSource, /copy an entry into the explicit-language profile/);
});

test("a real profile named global does not collide with the global hints scope", () => {
  assert.match(settingsSource, /scopeId === ""/);
  assert.doesNotMatch(settingsSource, /profile\.id\s*!==\s*"global"/);
  assert.match(settingsSource, /profileForId\(scopeId, source\)/);
});

test("scope switching is local and stale saves cannot overwrite a newer scope", () => {
  const switchStart = settingsSource.indexOf("function onGlossaryScopeChange");
  const switchEnd = settingsSource.indexOf("\n  }", switchStart);
  assert.ok(switchStart >= 0 && switchEnd > switchStart);
  const switchSource = settingsSource.slice(switchStart, switchEnd);
  assert.doesNotMatch(switchSource, /setInitialPrompt|setProfileGlossary|applySetting/);
  assert.match(settingsSource, /let glossaryScopeGeneration = 0/);
  assert.match(settingsSource, /generation !== glossaryScopeGeneration[\s\S]*scopeId !== glossaryScopeId[\s\S]*draftGeneration !== glossaryDraftGeneration/);
  assert.match(settingsSource, /!conflict[\s\S]*glossaryDraft = glossarySourceForScope/);
});

test("clean scopes follow persisted refreshes while dirty and newer drafts survive old failures", () => {
  assert.match(settingsSource, /let lastStoredGlossarySources: Record<string, string> = \{\}/);
  assert.match(settingsSource, /const previousStored = lastStoredGlossarySources\[currentScope\]/);
  assert.match(settingsSource, /glossaryDraft === previousStored/);
  assert.match(settingsSource, /lastStoredGlossarySources\[currentScope\] = currentStored/);
  assert.match(settingsSource, /let glossaryDraftGeneration = 0/);
  assert.match(settingsSource, /glossaryDraftGeneration \+= 1/);
  assert.match(settingsSource, /const draftGeneration = glossaryDraftGeneration/);
  assert.match(settingsSource, /draftGeneration !== glossaryDraftGeneration[\s\S]*return;/);
});

test("dictionary saves use the edit baseline and preserve concurrent conflicts", () => {
  assert.match(settingsSource, /let glossaryEditBaseline: \{ scopeId: string; source: string; generation: number \} \| null = null/);
  assert.match(settingsSource, /const editBaseline = glossaryEditBaseline/);
  assert.match(settingsSource, /editBaseline\.generation <= draftGeneration/);
  assert.match(settingsSource, /setInitialPrompt\(value, expectedSource\)/);
  assert.match(settingsSource, /setProfileGlossary\(scopeId, value, expectedSource\)/);
  assert.match(settingsSource, /async function refreshDictionaryAfterConflict\(primaryError: string\)/);
  assert.match(settingsSource, /settings = await getSettings\(\);[\s\S]*settingsError = primaryError/);
  assert.match(settingsSource, /const saveError = \{ value: "" \};/);
  assert.match(settingsSource, /const conflict = saveError\.value\.startsWith\(dictionaryConflictPrefix\)/);
  assert.match(settingsSource, /glossaryEditBaseline === editBaseline/);
  assert.match(settingsSource, /glossaryEditBaseline = \{ \.\.\.editBaseline, source: value \}/);
  assert.match(settingsSource, /if \(saved\) \{[\s\S]*glossaryEditBaseline = null;/);
  assert.match(settingsSource, /!conflict/);
  assert.match(settingsSource, /if \(settingsError\.startsWith\(dictionaryConflictPrefix\)\) settingsError = ""/);
  assert.match(settingsSource, /This dictionary changed elsewhere\. Your draft is preserved[\s\S]*close and reopen Settings/);
  assert.match(settingsSource, /let orphanGlossaryDraft: \{ scopeId: string; draft: string \} \| null = \$state\(null\)/);
  assert.match(settingsSource, /Unsaved draft for removed profile/);
  assert.match(settingsSource, /it is never saved automatically into Global hints/);
});

test("selected profile fixes the file language and dictionary together", () => {
  assert.match(settingsSource, /languageLabel\(transcribeLanguage\(\)\)/);
  assert.match(settingsSource, /return selectedTranscribeProfile\(\)\?\.language \?\? settings\?\.language \?\? "auto"/);
  assert.match(settingsSource, /This profile fixes the file language and uses its personal dictionary/);
  assert.match(settingsSource, /Temporary hint-only context for this import/);
});
