import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";
import vm from "node:vm";

const settingsSource = await readFile(
  new URL("../src/lib/Settings.svelte", import.meta.url),
  "utf8",
);
const scriptSource = settingsSource.match(/<script lang="ts">([\s\S]*?)<\/script>/)?.[1];
assert.ok(scriptSource, "Settings.svelte should have a TypeScript script block");

function functionSource(name) {
  const plainStart = scriptSource.indexOf(`function ${name}`);
  const asyncStart = scriptSource.indexOf(`async function ${name}`);
  const start = asyncStart >= 0 && (plainStart < 0 || asyncStart < plainStart)
    ? asyncStart
    : plainStart;
  assert.ok(start >= 0, `Settings.svelte should define ${name}`);
  let brace = scriptSource.indexOf("{", start);
  assert.ok(brace >= 0, `${name} should have a body`);
  let depth = 0;
  let quote = null;
  let escaped = false;
  let lineComment = false;
  let blockComment = false;
  for (let i = brace; i < scriptSource.length; i += 1) {
    const ch = scriptSource[i];
    const next = scriptSource[i + 1];
    if (lineComment) {
      if (ch === "\n") lineComment = false;
      continue;
    }
    if (blockComment) {
      if (ch === "*" && next === "/") {
        blockComment = false;
        i += 1;
      }
      continue;
    }
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (ch === "\\") {
        escaped = true;
      } else if (ch === quote) {
        quote = null;
      }
      continue;
    }
    if (ch === "/" && next === "/") {
      lineComment = true;
      i += 1;
      continue;
    }
    if (ch === "/" && next === "*") {
      blockComment = true;
      i += 1;
      continue;
    }
    if (ch === "\"" || ch === "'" || ch === "`") {
      quote = ch;
      continue;
    }
    if (ch === "{") depth += 1;
    if (ch === "}" && --depth === 0) return scriptSource.slice(start, i + 1);
  }
  throw new Error(`Could not find the end of ${name}`);
}

function transpile(source) {
  return ts.transpileModule(source, {
    compilerOptions: {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.None,
      verbatimModuleSyntax: false,
    },
  }).outputText;
}

function createHarness({ failure = null } = {}) {
  const functions = [
    "glossaryHasUnsavedChanges",
    "saveGlossary",
    "onGlossaryInput",
    "discardGlossaryChanges",
    "commitGlossaryScopeChange",
    "onGlossaryScopeChange",
    "promptGlossaryNavigation",
    "restoreGlossaryNavigationFocus",
    "finishPendingGlossaryNavigation",
    "saveAndFinishGlossaryNavigation",
    "discardAndFinishGlossaryNavigation",
    "stayOnGlossaryDraft",
    "requestTabChange",
  ].map(functionSource).join("\n");
  const harness = `
    let settings = {
      initial_prompt: "global saved",
      profile_glossaries: { english: "english saved", swedish: "swedish saved" },
      hotkey_profiles: [
        { id: "english", name: "English", language: "en" },
        { id: "swedish", name: "Swedish", language: "sv" },
      ],
    };
    let settingsError = "";
    let activeTab = "settings";
    let glossaryScopeId = "";
    let glossaryDraft = settings.initial_prompt;
    let glossaryDraftInitialized = true;
    let glossaryScopeGeneration = 0;
    let glossaryDraftGeneration = 0;
    let lastStoredGlossarySources = { "": settings.initial_prompt };
    let glossaryEditBaseline = null;
    let glossarySaving = false;
    let glossarySaveInFlight = null;
    let pendingGlossaryNavigation = null;
    let glossaryReturnFocusEl = null;
    let recoveredGlossaryDrafts = [];
    let glossaryConflictScopeId = null;
    class HTMLElement {}
    const calls = [];
    const dictionaryConflictPrefix = "Dictionary changed elsewhere:";
    const failure = ${JSON.stringify(failure)};

    function explicitProfiles(source = settings) {
      return source?.hotkey_profiles.filter((profile) => profile.language !== "auto") ?? [];
    }
    function profileForId(profileId, source = settings) {
      if (!profileId) return null;
      return explicitProfiles(source).find((profile) => profile.id === profileId) ?? null;
    }
    function glossarySourceForScope(scopeId, source = settings) {
      if (!source || scopeId === "") return source?.initial_prompt ?? "";
      return source.profile_glossaries[scopeId] ?? "";
    }
    function isValidGlossaryScope(scopeId, source = settings) {
      return scopeId === "" || profileForId(scopeId, source) !== null;
    }
    function rememberGlossaryRecovery(scopeId, draft, conflicted = false) {
      recoveredGlossaryDrafts.push({ scopeId, draft, conflicted });
    }
    function removeGlossaryRecovery(scopeId, draft) {
      recoveredGlossaryDrafts = recoveredGlossaryDrafts.filter(
        (recovery) => recovery.scopeId !== scopeId || recovery.draft !== draft,
      );
    }
    function isCurrentGlossaryRequest(request) {
      return request.generation === glossaryScopeGeneration
        && request.scopeId === glossaryScopeId
        && request.draftGeneration === glossaryDraftGeneration;
    }
    async function getSettings() {
      return settings;
    }
    async function refreshProfileModels() {}
    async function setInitialPrompt(value, expectedSource) {
      calls.push({ command: "setInitialPrompt", scopeId: "", value, expectedSource });
      if (failure) throw new Error(failure);
      settings = { ...settings, initial_prompt: value };
    }
    async function setProfileGlossary(scopeId, value, expectedSource) {
      calls.push({ command: "setProfileGlossary", scopeId, value, expectedSource });
      if (failure) throw new Error(failure);
      settings = {
        ...settings,
        profile_glossaries: { ...settings.profile_glossaries, [scopeId]: value },
      };
    }
    async function applySetting(mutate, errorSink, reportError = true) {
      if (reportError) settingsError = "";
      try {
        await mutate();
        settings = await getSettings();
        await refreshProfileModels(settings.hotkey_profiles);
        return true;
      } catch (error) {
        const message = typeof error === "string" ? error : error?.message || "Failed to save setting.";
        if (reportError) settingsError = message;
        if (errorSink) errorSink.value = message;
        return false;
      }
    }
    async function refreshDictionaryAfterConflict(primaryError, request) {
      settings = await getSettings();
      if (isCurrentGlossaryRequest(request)) {
        settingsError = primaryError;
        glossaryConflictScopeId = request.scopeId;
      } else {
        rememberGlossaryRecovery(request.scopeId, request.value, true);
      }
    }
    ${functions}
    globalThis.exercise = {
      saveGlossary,
      onGlossaryInput,
      discardGlossaryChanges,
      commitGlossaryScopeChange,
      onGlossaryScopeChange,
      saveAndFinishGlossaryNavigation,
      discardAndFinishGlossaryNavigation,
      stayOnGlossaryDraft,
      requestTabChange,
      snapshot: () => ({
        settings,
        settingsError,
        activeTab,
        glossaryScopeId,
        glossaryDraft,
        glossaryDraftGeneration,
        glossaryEditBaseline,
        glossarySaving,
        pendingGlossaryNavigation,
        glossaryConflictScopeId,
        calls: [...calls],
      }),
    };
  `;
  const context = vm.createContext({ console, queueMicrotask, setTimeout, clearTimeout });
  vm.runInContext(transpile(harness), context, { timeout: 1000 });
  return context.exercise;
}

function input(exercise, value) {
  exercise.onGlossaryInput({ target: { value } });
}

test("caret edits are local: input and blur do not persist", async () => {
  assert.doesNotMatch(settingsSource, /onblur=\{onInitialPromptBlur\}/);
  const exercise = createHarness();
  input(exercise, "global d");
  input(exercise, "global draft");
  assert.equal(exercise.snapshot().calls.length, 0);
  assert.equal(exercise.snapshot().glossaryDraft, "global draft");
  await Promise.resolve();
  assert.equal(exercise.snapshot().calls.length, 0);
});

test("the first dictionary edit establishes a reactive dirty baseline", () => {
  assert.match(
    scriptSource,
    /let glossaryEditBaseline: \{ scopeId: string; source: string; generation: number \} \| null = \$state\(null\)/,
    "the dirty baseline must participate in Svelte reactivity",
  );
  const exercise = createHarness();
  assert.equal(exercise.snapshot().glossaryEditBaseline, null);
  input(exercise, "global draft");
  const state = exercise.snapshot();
  assert.deepEqual(JSON.parse(JSON.stringify(state.glossaryEditBaseline)), {
    scopeId: "",
    source: "global saved",
    generation: 1,
  });
  exercise.requestTabChange("dictate");
  assert.equal(exercise.snapshot().pendingGlossaryNavigation.kind, "tab");
});

test("Save persists the active scope with its edit baseline as expected_source", async () => {
  const exercise = createHarness();
  input(exercise, "global draft");
  assert.equal(await exercise.saveGlossary(), true);
  assert.deepEqual(JSON.parse(JSON.stringify(exercise.snapshot().calls)), [{
    command: "setInitialPrompt",
    scopeId: "",
    value: "global draft",
    expectedSource: "global saved",
  }]);

  exercise.commitGlossaryScopeChange("english");
  input(exercise, "english draft");
  assert.equal(await exercise.saveGlossary(), true);
  assert.deepEqual(JSON.parse(JSON.stringify(exercise.snapshot().calls.at(-1))), {
    command: "setProfileGlossary",
    scopeId: "english",
    value: "english draft",
    expectedSource: "english saved",
  });
});

test("Discard restores the saved source without persisting", () => {
  const exercise = createHarness();
  input(exercise, "global draft");
  exercise.discardGlossaryChanges();
  const state = exercise.snapshot();
  assert.equal(state.glossaryDraft, "global saved");
  assert.equal(state.glossaryEditBaseline, null);
  assert.equal(state.calls.length, 0);
});

test("clean dictionary action buttons do not advertise an active wait", () => {
  const disabledActionRule = settingsSource.match(
    /\.dictionary-actions button:disabled,[\s\S]*?opacity:\s*0\.6;/,
  )?.[0];
  assert.ok(disabledActionRule, "dictionary disabled-button styling should remain explicit");
  assert.match(
    disabledActionRule,
    /cursor:\s*default/,
    "a clean disabled Save button must not show macOS's spinning wait cursor",
  );
  assert.doesNotMatch(
    disabledActionRule,
    /cursor:\s*wait/,
    "the disabled state is also used when no save is in progress",
  );
});

test("dirty scope navigation offers Save, Discard, and Stay", async () => {
  const exercise = createHarness();
  input(exercise, "global draft");
  exercise.onGlossaryScopeChange({ target: { value: "english" }, currentTarget: { value: "english" } });
  assert.equal(exercise.snapshot().pendingGlossaryNavigation.kind, "scope");
  assert.equal(exercise.snapshot().pendingGlossaryNavigation.scopeId, "english");
  assert.equal(exercise.snapshot().glossaryScopeId, "");

  exercise.stayOnGlossaryDraft();
  assert.equal(exercise.snapshot().pendingGlossaryNavigation, null);
  assert.equal(exercise.snapshot().glossaryScopeId, "");

  exercise.onGlossaryScopeChange({ target: { value: "english" }, currentTarget: { value: "english" } });
  exercise.discardAndFinishGlossaryNavigation();
  assert.equal(exercise.snapshot().glossaryScopeId, "english");
  assert.equal(exercise.snapshot().glossaryDraft, "english saved");

  exercise.commitGlossaryScopeChange("");
  input(exercise, "global draft 2");
  exercise.onGlossaryScopeChange({ target: { value: "english" }, currentTarget: { value: "english" } });
  await exercise.saveAndFinishGlossaryNavigation();
  assert.equal(exercise.snapshot().glossaryScopeId, "english");
  assert.equal(exercise.snapshot().calls.at(-1).expectedSource, "global saved");
});

test("dirty tab navigation offers the same decisions", () => {
  const exercise = createHarness();
  input(exercise, "global draft");
  exercise.requestTabChange("dictate");
  assert.equal(exercise.snapshot().pendingGlossaryNavigation.kind, "tab");
  assert.equal(exercise.snapshot().pendingGlossaryNavigation.tab, "dictate");
  assert.equal(exercise.snapshot().activeTab, "settings");
  exercise.stayOnGlossaryDraft();
  assert.equal(exercise.snapshot().activeTab, "settings");
  exercise.requestTabChange("dictate");
  exercise.discardAndFinishGlossaryNavigation();
  assert.equal(exercise.snapshot().activeTab, "dictate");
});

for (const failure of ["write failed", "Dictionary changed elsewhere: current source"]) {
  test(`failed or conflict save retains the draft and blocks navigation (${failure})`, async () => {
    const exercise = createHarness({ failure });
    input(exercise, "draft that must survive");
    assert.equal(await exercise.saveGlossary(), false);
    const failed = exercise.snapshot();
    assert.equal(failed.glossaryDraft, "draft that must survive");
    assert.notEqual(failed.glossaryEditBaseline, null);
    exercise.requestTabChange("dictate");
    const blocked = exercise.snapshot();
    assert.equal(blocked.activeTab, "settings");
    assert.equal(blocked.pendingGlossaryNavigation.kind, "tab");
    assert.equal(blocked.pendingGlossaryNavigation.tab, "dictate");
    if (failure.startsWith("Dictionary")) {
      assert.equal(blocked.glossaryConflictScopeId, "");
      assert.match(blocked.settingsError, /^Dictionary changed elsewhere:/);
    }
  });
}
