import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const script = source.match(/<script lang="ts">([\s\S]*?)<\/script>/)[1];
const parsed = ts.createSourceFile("Settings.ts", script, ts.ScriptTarget.Latest, true);
const names = ["applySetting", "onLanguageChange", "clearDictionaryAndSwitchLanguage",
  "glossaryHasUnsavedChanges", "rememberGlossaryRecovery", "discardGlossaryChanges"];
const functions = names.map((name) => {
  const declaration = parsed.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
  assert.ok(declaration, `Missing ${name}`);
  return declaration.getText(parsed);
}).join("\n");

function harness({ clearFailure = "", retryFailure = "", refreshFailure = false, empty = false } = {}) {
  let release;
  const context = vm.createContext({ options: { clearFailure, retryFailure, refreshFailure, empty },
    pauseClear: () => new Promise((resolve) => { release = resolve; }) });
  vm.runInContext(ts.transpileModule(`
    let stored = { language: "sv", initial_prompt: "global hints", hotkey_profiles: [],
      profile_glossaries: { default: options.empty ? "" : "mergea = mördsa", english: "merge = merch" } };
    let settings = structuredClone(stored);
    let settingsError = "", blockedLanguageChange = null, languageSaving = false, models = [];
    let glossarySaving = false, glossaryScopeId = "english", glossaryDraft = "english draft";
    let glossaryEditBaseline = { scopeId: "english", source: "merge = merch" };
    let glossaryConflictScopeId = null, glossaryDraftGeneration = 1, glossaryDraftInitialized = true;
    let lastStoredGlossarySources = {}, recoveredGlossaryDrafts = [];
    const dictionaryConflictPrefix = "Dictionary changed elsewhere:";
    const calls = [];
    let holdClear = false, cleared = false;
    async function setLanguage(value) {
      calls.push(["language", value]);
      if (stored.profile_glossaries.default.trim() && stored.language !== value)
        throw "Profile 'default' has a personal dictionary; clear it before changing the profile language";
      if (cleared && options.retryFailure) throw options.retryFailure;
      stored.language = value;
    }
    async function setProfileGlossary(id, value, expected) {
      calls.push(["dictionary", id, value, expected]);
      if (holdClear) await pauseClear();
      if (options.clearFailure) throw options.clearFailure;
      if (expected !== stored.profile_glossaries[id]) throw "Dictionary changed elsewhere: default";
      stored.profile_glossaries[id] = value;
      cleared = true;
    }
    async function getSettings() {
      if (cleared && options.refreshFailure) throw "read failed";
      return structuredClone(stored);
    }
    async function getModelInfo() { return []; }
    async function refreshProfileModels() {}
    function glossarySourceForScope(id) { return settings.profile_glossaries[id] ?? settings.initial_prompt; }
    ${functions}
    globalThis.exercise = {
      select: async (value = "en") => { const select = { value }; await onLanguageChange({currentTarget: select, target: select}); return select.value; },
      clear: clearDictionaryAndSwitchLanguage,
      cancel: () => { blockedLanguageChange = null; },
      changeStoredDictionary: () => { stored.profile_glossaries.default = "new words"; },
      selectDefaultDraft: () => { glossaryScopeId = "default"; glossaryDraft = "default draft";
        glossaryEditBaseline = { scopeId: "default", source: stored.profile_glossaries.default }; },
      hold: () => { holdClear = true; },
      snapshot: () => JSON.parse(JSON.stringify({ stored, settings, settingsError, blockedLanguageChange,
        languageSaving, calls, glossaryDraft, glossaryEditBaseline, recoveredGlossaryDrafts })),
    };
  `, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None } }).outputText,
  vm.createContext(Object.assign(context, { structuredClone })));
  return { ...context.exercise, snapshot: () => JSON.parse(JSON.stringify(context.exercise.snapshot())), release: () => release() };
}

test("rejection restores the saved selection and previews the blocking dictionary, not the editor scope", async () => {
  const h = harness();
  assert.equal(await h.select(), "sv");
  const state = h.snapshot();
  assert.equal(state.blockedLanguageChange.source, "mergea = mördsa");
  assert.equal(state.blockedLanguageChange.language, "en");
  assert.equal(state.glossaryDraft, "english draft");
  assert.equal(state.calls.length, 1);
  h.cancel();
  await h.clear();
  assert.equal(h.snapshot().calls.length, 1, "cancel must prevent clearing and retrying");
});

test("confirmed recovery clears only default, switches language and preserves the other draft", async () => {
  const h = harness();
  await h.select();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.stored.language, "en");
  assert.deepEqual(state.stored.profile_glossaries, { default: "", english: "merge = merch" });
  assert.equal(state.stored.initial_prompt, "global hints");
  assert.equal(state.glossaryDraft, "english draft");
  assert.equal(state.blockedLanguageChange, null);
  assert.equal(state.languageSaving, false);
  assert.equal(state.settingsError, "");
});

test("default draft is recoverable after its saved dictionary is explicitly cleared", async () => {
  const h = harness();
  h.selectDefaultDraft();
  await h.select();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.glossaryDraft, "");
  assert.equal(state.glossaryEditBaseline, null);
  assert.equal(state.recoveredGlossaryDrafts[0].draft, "default draft");
  assert.equal(state.recoveredGlossaryDrafts[0].scopeId, "default");
});

test("dictionary changed after preview is not cleared and language is not retried", async () => {
  const h = harness();
  await h.select();
  h.changeStoredDictionary();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.stored.language, "sv");
  assert.equal(state.stored.profile_glossaries.default, "new words");
  assert.equal(state.calls.filter(([command]) => command === "language").length, 1);
  assert.match(state.settingsError, /could not be cleared.*Dictionary changed elsewhere/);
  assert.equal(state.blockedLanguageChange, null);
});

test("failed dictionary persistence preserves the dictionary and draft", async () => {
  const h = harness({ clearFailure: "disk full" });
  h.selectDefaultDraft();
  await h.select();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.stored.language, "sv");
  assert.equal(state.stored.profile_glossaries.default, "mergea = mördsa");
  assert.equal(state.glossaryDraft, "default draft");
  assert.match(state.settingsError, /language was not changed: disk full/);
});

test("failed language retry reports the completed clear and does not allow repeating it", async () => {
  const h = harness({ retryFailure: "write failed" });
  await h.select();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.stored.profile_glossaries.default, "");
  assert.equal(state.stored.language, "sv");
  assert.match(state.settingsError, /dictionary was cleared, but the language change failed: write failed/);
  await h.clear();
  assert.equal(h.snapshot().calls.length, 3);
});

test("refresh failure after successful mutation is reported without offering another clear", async () => {
  const h = harness({ refreshFailure: true });
  await h.select();
  await h.clear();
  const state = h.snapshot();
  assert.equal(state.stored.language, "en");
  assert.equal(state.blockedLanguageChange, null);
  assert.equal(state.languageSaving, false);
  assert.match(state.settingsError, /reopen Settings to confirm the saved language/);
});

test("duplicate recovery and language selections are blocked while clearing", async () => {
  const h = harness();
  await h.select();
  h.hold();
  const pending = h.clear();
  await h.clear();
  assert.equal(await h.select("no"), "sv");
  assert.equal(h.snapshot().calls.length, 2);
  h.release();
  await pending;
  assert.equal(h.snapshot().stored.language, "en");
});

test("ordinary language change without dictionary succeeds without confirmation", async () => {
  const h = harness({ empty: true });
  assert.equal(await h.select(), "en");
  assert.equal(h.snapshot().blockedLanguageChange, null);
  assert.equal(h.snapshot().calls.length, 1);
});
