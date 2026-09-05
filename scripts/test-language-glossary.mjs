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
  assert.match(settingsSource, /function isCurrentGlossaryRequest\(request: GlossarySaveRequest\)[\s\S]*request\.generation === glossaryScopeGeneration[\s\S]*request\.scopeId === glossaryScopeId[\s\S]*request\.draftGeneration === glossaryDraftGeneration/);
  assert.doesNotMatch(settingsSource, /else if \(!conflict && settings\)[\s\S]*glossaryDraft = glossarySourceForScope/);
});

test("clean scopes follow persisted refreshes while dirty and newer drafts survive old failures", () => {
  assert.match(settingsSource, /let lastStoredGlossarySources: Record<string, string> = \{\}/);
  assert.match(settingsSource, /const previousStored = lastStoredGlossarySources\[currentScope\]/);
  assert.match(settingsSource, /glossaryDraft === previousStored/);
  assert.match(settingsSource, /lastStoredGlossarySources\[currentScope\] = currentStored/);
  assert.match(settingsSource, /let glossaryDraftGeneration = 0/);
  assert.match(settingsSource, /glossaryDraftGeneration \+= 1/);
  assert.match(settingsSource, /draftGeneration: glossaryDraftGeneration/);
  assert.match(settingsSource, /let requestIsCurrent\s*=\s*[\s\S]*draftGeneration === glossaryDraftGeneration/);
  assert.match(settingsSource, /if \(!requestIsCurrent\) return;/);
});

test("dictionary saves use the edit baseline and preserve concurrent conflicts", () => {
  assert.match(settingsSource, /let glossaryEditBaseline: \{ scopeId: string; source: string; generation: number \} \| null = null/);
  assert.match(settingsSource, /const editBaseline = glossaryEditBaseline/);
  assert.match(settingsSource, /editBaseline\.generation <= draftGeneration/);
  assert.match(settingsSource, /setInitialPrompt\(value, expectedSource\)/);
  assert.match(settingsSource, /setProfileGlossary\(scopeId, value, expectedSource\)/);
  assert.match(settingsSource, /async function refreshDictionaryAfterConflict\(primaryError: string, request: GlossarySaveRequest\)/);
  assert.match(settingsSource, /settings = await getSettings\(\);[\s\S]*settingsError = primaryError/);
  assert.match(settingsSource, /const saveError = \{ value: "" \};/);
  assert.match(settingsSource, /const conflict = saveError\.value\.startsWith\(dictionaryConflictPrefix\)/);
  assert.match(settingsSource, /glossaryEditBaseline === editBaseline/);
  assert.match(settingsSource, /glossaryEditBaseline = \{ \.\.\.editBaseline, source: value \}/);
  assert.match(settingsSource, /if \(saved\) \{[\s\S]*glossaryEditBaseline = null;/);
  assert.match(settingsSource, /settingsError = saved \? "" : saveError\.value/);
  assert.match(settingsSource, /if \(previousConflict\) \{[\s\S]*settingsError = ""/);
  assert.match(settingsSource, /This dictionary changed elsewhere\. Your draft is preserved[\s\S]*close and reopen Settings/);
  assert.match(settingsSource, /type RecoveredGlossaryDraft = \{ scopeId: string; draft: string; conflicted: boolean \}/);
  assert.match(settingsSource, /let recoveredGlossaryDrafts: RecoveredGlossaryDraft\[\] = \$state\(\[\]\)/);
  assert.match(settingsSource, /function rememberGlossaryRecovery\(scopeId: string, draft: string, conflicted = false\)/);
  assert.match(settingsSource, /recovery\.scopeId === scopeId && recovery\.draft === draft/);
  assert.match(settingsSource, /Unsaved draft.*glossaryScopeLabel\(recovery\.scopeId\)/s);
  assert.match(settingsSource, /never saved or copied automatically into another dictionary/);
});

test("late saves preserve scoped drafts without replacing newer scope state", () => {
  assert.match(settingsSource, /async function refreshDictionaryAfterConflict\(primaryError: string, request: GlossarySaveRequest\)/);
  assert.match(settingsSource, /refreshDictionaryAfterConflict\(saveError\.value, request\)/);
  assert.match(settingsSource, /if \(isCurrentGlossaryRequest\(request\)\) \{[\s\S]*glossaryConflictScopeId = request\.scopeId;/);
  assert.match(settingsSource, /else \{\s*rememberGlossaryRecovery\(request\.scopeId, request\.value, true\);/);
  assert.match(settingsSource, /function isCurrentGlossaryRequest\(request: GlossarySaveRequest\)/);
  assert.match(settingsSource, /requestIsCurrent = isCurrentGlossaryRequest\(request\);/);
  assert.match(settingsSource, /if \(!requestIsCurrent\) return;/);
  assert.match(settingsSource, /const previousConflict = glossaryConflictScopeId === previousScope/);
  assert.match(settingsSource, /rememberGlossaryRecovery\(previousScope, glossaryDraft, previousConflict\)/);
  assert.match(settingsSource, /recoveredGlossaryDrafts as recovery \(recovery\.scopeId \+ "\\u0000" \+ recovery\.draft\)/);
});

test("clean unchanged blur does not invoke a dictionary save", () => {
  assert.match(settingsSource, /if \(!editBaseline && value === \(lastStoredGlossarySources\[scopeId\] \?\? glossarySourceForScope\(scopeId\)\)\) return;/);
  assert.match(settingsSource, /setProfileGlossary\(scopeId, value, expectedSource\), saveError, false/);
});

test("non-CAS dictionary failures retain the typed draft and baseline", () => {
  assert.match(settingsSource, /settingsError = saved \? "" : saveError\.value/);
  assert.doesNotMatch(settingsSource, /else if \(!conflict && settings\)[\s\S]*glossaryDraft = glossarySourceForScope/);
  assert.match(settingsSource, /else if \(!saved && !requestIsCurrent\) \{\s*rememberGlossaryRecovery\(scopeId, value\);/);
});

test("selected profile fixes the file language and dictionary together", () => {
  assert.match(settingsSource, /languageLabel\(transcribeLanguage\(\)\)/);
  assert.match(settingsSource, /return selectedTranscribeProfile\(\)\?\.language \?\? settings\?\.language \?\? "auto"/);
  assert.match(settingsSource, /This profile fixes the file language and uses its personal dictionary/);
  assert.match(settingsSource, /Temporary hint-only context for this import/);
});
