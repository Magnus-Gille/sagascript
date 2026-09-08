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
  const brace = scriptSource.indexOf("{", start);
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
      if (escaped) escaped = false;
      else if (ch === "\\") escaped = true;
      else if (ch === quote) quote = null;
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
    if (ch === '"' || ch === "'" || ch === "`") {
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

function deferred() {
  let resolve;
  const promise = new Promise((nextResolve) => { resolve = nextResolve; });
  return { promise, resolve };
}

function tick() {
  return new Promise((resolve) => setImmediate(resolve));
}

function makeControls(overrides = {}) {
  return {
    calls: [],
    initialReview: { id: "active-review", revision: 7 },
    initialTranscript: { id: "active-transcript" },
    initialProposal: { proposal: { revision: 8 } },
    initialPlan: { id: "selected-plan", plan: { context: { previous_revision: 7 } } },
    planResult: { id: "selected-plan", plan: { context: { previous_revision: 7 } } },
    previewResult: { proposal: { revision: 9 }, steps: [] },
    reprocessingResult: { id: "reprocessing-result", proposal: { revision: 9 } },
    acceptResult: {
      review: { id: "accepted-review", revision: 9 },
      transcript: { id: "accepted-transcript" },
    },
    snapshots: [],
    planGate: null,
    acceptGate: null,
    previewGate: null,
    beginGate: null,
    actionQueue: Promise.resolve(),
    ...overrides,
  };
}

function createHarness(controls) {
  const functions = [
    "meetingFailureText",
    "waitForMeetingActions",
    "enqueueMeetingAction",
    "acceptMeetingReview",
    "initializeMeetingReview",
    "pollMeetingJob",
    "planCurrentMeeting",
    "executeCurrentMeetingPlan",
    "acceptCurrentMeetingProposal",
  ].map(functionSource).join("\n");
  const harness = `
    let meetingReview = controls.initialReview;
    let meetingTranscript = controls.initialTranscript;
    let meetingJobId = "job-active";
    let meetingJobStatus = "running";
    let meetingPhase = "Running";
    let meetingError = "";
    let meetingPollingFailed = false;
    let meetingPollGeneration = 1;
    let meetingPollActive = false;
    let meetingDocumentRevision = meetingReview.revision;
    let meetingReviewResetKey = 0;
    let meetingReviewDraftDirty = false;
    let meetingReprocessingPlan = controls.initialPlan;
    let meetingProposal = controls.initialProposal;
    let meetingReprocessingResult = null;
    let meetingReprocessingBusy = false;
    let meetingActionQueue = controls.actionQueue;
    let meetingReviewInit = null;
    let transcribing = false;
    let transcriptionProgress = 0;
    let transcribePrompt = "";

    function selectedTranscribeProfile() { return null; }

    async function planMeetingReprocessing(...args) {
      controls.calls.push({ kind: "plan", args });
      if (controls.planGate) await controls.planGate;
      return controls.planResult;
    }

    async function acceptMeetingProposal(...args) {
      controls.calls.push({ kind: "accept", args });
      if (controls.acceptGate) await controls.acceptGate;
      return controls.acceptResult;
    }

    async function beginMeetingReprocessing(...args) {
      controls.calls.push({ kind: "begin", args });
      if (controls.beginGate) await controls.beginGate;
      return "job-reprocess";
    }

    async function previewMeetingProposal(...args) {
      controls.calls.push({ kind: "preview", args });
      if (controls.previewGate) await controls.previewGate;
      return controls.previewResult;
    }

    async function createMeetingReview() {
      return { review: controls.initialReview, transcript: controls.initialTranscript };
    }

    async function getMeetingJob() { return null; }

    function waitForMeetingPoll() { return Promise.resolve(); }

    async function pollMeetingJobClient(options) {
      for (const snapshot of controls.snapshots) await options.onSnapshot(snapshot);
    }

    ${functions}

    globalThis.exercise = {
      acceptCurrentMeetingProposal,
      executeCurrentMeetingPlan,
      planCurrentMeeting,
      pollMeetingJob,
      setActionQueue: (queue) => { meetingActionQueue = queue; },
      setMeetingDocumentRevision: (value) => { meetingDocumentRevision = value; },
      setMeetingPollGeneration: (value) => { meetingPollGeneration = value; },
      setMeetingReviewDraftDirty: (value) => { meetingReviewDraftDirty = value; },
      setMeetingReprocessingBusy: (value) => { meetingReprocessingBusy = value; },
      waitForMeetingReviewInit: async () => { if (meetingReviewInit) await meetingReviewInit; },
      snapshot: () => ({
        meetingReview,
        meetingProposal,
        meetingReprocessingPlan,
        meetingReprocessingResult,
        meetingReprocessingBusy,
        meetingReviewDraftDirty,
        meetingDocumentRevision,
        meetingPollGeneration,
        meetingReviewResetKey,
        meetingJobId,
        transcribing,
      }),
    };
  `;
  const context = vm.createContext({ console, queueMicrotask, setTimeout, clearTimeout, setImmediate, controls });
  vm.runInContext(transpile(harness), context, { timeout: 1000 });
  return context.exercise;
}

test("meeting callbacks retain revision and generation guards", () => {
  assert.match(scriptSource, /revision !== meetingDocumentRevision/);
  assert.match(scriptSource, /generation !== meetingPollGeneration/);
  assert.match(scriptSource, /meetingReviewDraftDirty/);
  assert.match(scriptSource, /await waitForMeetingActions\(\)/);
});

for (const [label, invalidate] of [
  ["review revision", (exercise) => exercise.setMeetingDocumentRevision(8)],
  ["poll generation", (exercise) => exercise.setMeetingPollGeneration(2)],
]) {
  test(`async proposal acceptance rejects a stale ${label}`, async () => {
    const controls = makeControls();
    const gate = deferred();
    controls.acceptGate = gate.promise;
    const exercise = createHarness(controls);
    const pending = exercise.acceptCurrentMeetingProposal();
    await tick();
    assert.deepEqual(controls.calls.map((call) => call.kind), ["accept"]);

    invalidate(exercise);
    gate.resolve();
    await assert.rejects(pending, /review changed before acceptance/);

    const state = exercise.snapshot();
    assert.equal(state.meetingReview.id, "active-review");
    assert.equal(state.meetingReviewResetKey, 0);
    assert.equal(state.meetingReprocessingBusy, false);
  });
}

test("planning stores its selection without replacing the active review", async () => {
  const controls = makeControls();
  const exercise = createHarness(controls);
  await exercise.planCurrentMeeting("recluster", 0.5, true);

  const state = exercise.snapshot();
  assert.equal(state.meetingReview.id, "active-review");
  assert.equal(state.meetingReprocessingPlan.id, "selected-plan");
  assert.deepEqual(controls.calls.map((call) => call.kind), ["plan"]);
});

test("completed reprocessing stores a proposal beside, not over, the active review", async () => {
  const controls = makeControls({
    snapshots: [{
      id: "job-reprocess",
      status: "completed",
      phase: "done",
      error: null,
      transcript: null,
      reprocessing: { id: "result-1", proposal: { revision: 9 } },
    }],
    initialPlan: { id: "old-plan" },
  });
  controls.previewResult = { proposal: { revision: 9 }, steps: [{ status: "Applied" }] };
  const exercise = createHarness(controls);
  await exercise.pollMeetingJob("job-reprocess", 1);
  await exercise.waitForMeetingReviewInit();

  const state = exercise.snapshot();
  assert.equal(state.meetingReview.id, "active-review");
  assert.deepEqual(state.meetingProposal, controls.previewResult);
  assert.deepEqual(state.meetingReprocessingResult, controls.snapshots[0].reprocessing);
  assert.equal(state.meetingReprocessingPlan, null);
  assert.deepEqual(controls.calls.map((call) => call.kind), ["preview"]);
});

for (const [label, setState] of [
  ["a dirty draft", (exercise) => exercise.setMeetingReviewDraftDirty(true)],
  ["a newly busy reprocessing action", (exercise) => exercise.setMeetingReprocessingBusy(true)],
]) {
  test(`execution rechecks ${label} after the action queue`, async () => {
    const controls = makeControls();
    const gate = deferred();
    const exercise = createHarness(controls);
    exercise.setActionQueue(gate.promise);
    const pending = exercise.executeCurrentMeetingPlan();
    await tick();
    assert.equal(controls.calls.length, 0);

    setState(exercise);
    gate.resolve();
    await pending;

    const state = exercise.snapshot();
    assert.equal(controls.calls.length, 0, "reprocessing must not start after state changes while queued");
    assert.equal(state.transcribing, false);
  });
}
