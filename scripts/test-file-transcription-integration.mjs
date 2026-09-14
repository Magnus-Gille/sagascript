import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const fileSource = await readFile(new URL("../src/lib/FileTranscription.svelte", import.meta.url), "utf8");
const settingsSource = await readFile(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");
const compile = source => ts.transpileModule(source, { compilerOptions: {
  module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022,
} }).outputText;
async function load(name) {
  const source = await readFile(new URL(`../src/lib/${name}.ts`, import.meta.url), "utf8");
  return import(`data:text/javascript;base64,${Buffer.from(compile(source)).toString("base64")}`);
}
const stages = await load("transcribe-stages");
const ui = await load("transcribe-ui-state");
const queue = await load("transcription-queue");
const recent = await load("recent-transcriptions");
function functions(source, names) {
  const script = source.split('<script lang="ts">')[1].split("</script>")[0];
  const ast = ts.createSourceFile("component.ts", script, ts.ScriptTarget.Latest, true);
  return ast.statements.filter(node => ts.isFunctionDeclaration(node) && names.includes(node.name.text))
    .map(node => node.getText(ast)).join("\n");
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fileHarness(path, overrides = {}) {
  let listener;
  let removed = 0;
  const calls = [];
  const native = deferred();
  const deps = { ...stages, ...ui, job: { path, profileId: "swedish", prompt: "Astrid", diarize: false },
    listen: async (event, handler) => { assert.equal(event, "plain-transcription-progress"); listener = handler; return () => { removed++; }; },
    transcribeFile: async (...args) => { calls.push(args); return native.promise; },
    cancelFileTranscription: async id => { calls.push(["cancel", id]); return true; },
    copyTranscriptionText: async text => { calls.push(["copy", text]); },
    saveTranscriptionText: async (...args) => { calls.push(["save", ...args]); return true; },
    waitForMeetingActions: async () => {}, revealTranscriptionResult: async () => {},
    onMissingFile: path => calls.push(["missing", path]), ...overrides };
  const source = `let transcribing = false, cancellingPlain = false, plainRequestStarted = false;
    let plainRunId = null, meetingJobStatus = null, meetingPollGeneration = 0, meetingDocumentRevision = 0;
    let meetingError = '', transcriptionProgress = 0, transcribeError = '', transcriptionResult = '';
    let resultActionMessage = '', transcribeElapsedSec = 0, plainStages = initialStages();
    ${functions(fileSource, ["handleFileTranscription", "cancelPlainTranscription", "copyTranscriptionResult", "saveTranscriptionResult", "meetingFailureText"])}
    return { start: () => handleFileTranscription(job.path), cancel: cancelPlainTranscription,
      copy: copyTranscriptionResult, save: saveTranscriptionResult,
      state: () => ({transcribing, cancellingPlain, plainStages, transcribeError, transcriptionResult, resultActionMessage}) };`;
  const instance = new Function(...Object.keys(deps), compile(source))(...Object.values(deps));
  return { ...instance, calls, native, progress: payload => listener({ payload }), removed: () => removed };
}
const flush = () => new Promise(resolve => setImmediate(resolve));

test("per-file progress is run-scoped, subscribed before work, and duplicate starts cannot spawn workers", async () => {
  const h = fileHarness("/audio/one.wav");
  const run = h.start();
  await flush();
  await h.start();
  assert.equal(h.calls.length, 1);
  const [path, options] = h.calls[0];
  assert.equal(path, "/audio/one.wav");
  assert.equal(options.profileId, "swedish");
  assert.equal(options.prompt, "Astrid");
  assert.ok(options.runId);
  h.progress({ runId: "stale", phase: "transcribing", percent: 90 });
  assert.equal(h.state().plainStages.index, 0);
  h.progress({ runId: options.runId, phase: "resampling", percent: 60 });
  assert.equal(h.state().plainStages.index, 1);
  assert.equal(h.state().plainStages.percent, 60);
  h.native.resolve("one result");
  await run;
  assert.equal(h.state().plainStages.status, "completed");
  assert.equal(h.removed(), 1);
  assert.doesNotMatch(settingsSource, /listen\("(?:transcription-progress|transcription-phase|audio-decode-progress)"/);
});

test("Stop before submission cancels locally without starting native inference", async () => {
  const gate = deferred();
  const h = fileHarness("/audio/one.wav", { waitForMeetingActions: () => gate.promise });
  const run = h.start();
  await h.cancel();
  gate.resolve();
  await run;
  assert.deepEqual(h.calls, []);
  assert.equal(h.state().plainStages.status, "cancelled");
  assert.equal(h.state().transcribing, false);
});

test("Stop keeps the worker busy until settlement; backend success wins the cancellation race", async () => {
  const h = fileHarness("/audio/one.wav", { cancelFileTranscription: async () => false });
  const run = h.start();
  await flush();
  await h.cancel();
  assert.equal(h.state().transcribing, true);
  assert.equal(h.state().cancellingPlain, true);
  h.native.resolve("authoritative success");
  await run;
  assert.equal(h.state().plainStages.status, "completed");
  assert.equal(h.state().transcriptionResult, "authoritative success");
  assert.equal(h.state().transcribeError, "");
  assert.equal(h.state().resultActionMessage, "Finished before Stop took effect.");
});

test("late cancellation reply cannot alter a settled result", async () => {
  const cancel = deferred();
  const h = fileHarness("/audio/one.wav", { cancelFileTranscription: () => cancel.promise });
  const run = h.start();
  await flush();
  const stop = h.cancel();
  h.native.resolve("finished");
  await run;
  cancel.reject(new Error("late cancel failure"));
  await stop;
  assert.equal(h.state().transcribeError, "");
  assert.equal(h.state().plainStages.status, "completed");
});

test("failed Stop followed by native success does not mark a successful file failed", async () => {
  const h = fileHarness("/audio/one.wav", { cancelFileTranscription: async () => { throw new Error("cancel unavailable"); } });
  const run = h.start();
  await flush();
  await h.cancel();
  assert.equal(h.state().transcribeError, "cancel unavailable");
  h.native.resolve("finished");
  await run;
  assert.equal(h.state().transcribeError, "");
  assert.equal(h.state().plainStages.status, "completed");
});

test("accepted cancellation discards partial output and missing sources notify only their own path", async () => {
  const h = fileHarness("/audio/one.wav");
  const run = h.start();
  await flush();
  await h.cancel();
  h.native.reject(new Error("cancelled"));
  await run;
  assert.equal(h.state().plainStages.status, "cancelled");
  assert.equal(h.state().transcriptionResult, "");
  const missing = fileHarness("/audio/missing.wav");
  const attempt = missing.start();
  await flush();
  missing.native.reject(new Error("Failed to open file: No such file or directory"));
  await attempt;
  assert.ok(missing.calls.some(call => call[0] === "missing" && call[1] === "/audio/missing.wav"));
});

test("each result saves and copies its own output and source even while another file is busy", async () => {
  const first = fileHarness("/first/same.wav");
  const firstRun = first.start();
  await flush();
  first.native.resolve("first text");
  await firstRun;
  const second = fileHarness("/second/same.wav");
  const secondRun = second.start();
  await flush();
  await first.save();
  await first.copy();
  assert.deepEqual(first.calls.slice(-2), [["save", "first text", "same.txt", "/first"], ["copy", "first text"]]);
  assert.equal(second.state().transcribing, true);
  second.native.resolve("second text");
  await secondRun;
  await second.save();
  assert.deepEqual(second.calls.at(-1), ["save", "second text", "same.txt", "/second"]);
});

test("re-run appends a fresh job with the selected request's options and obeys the serial scheduler", () => {
  const current = { id: "existing", path: "/other.wav", profileId: null, prompt: null, diarize: false, status: "running" };
  const request = { path: "/chosen.wav", profileId: "swedish", prompt: "Astrid", diarize: true };
  const deps = { ...queue, ...recent, profileForId: id => id === "swedish" };
  const factory = new Function(...Object.keys(deps), "current", "request", compile(`
    let fileJobs = [current], recentTranscriptions = [request], selectedRerunPath = request.path;
    let selectedFileId = current.id, rerunError = '';
    ${functions(settingsSource, ["retryLastTranscription"])}
    retryLastTranscription(); return { fileJobs, selectedFileId, rerunError };`));
  const result = factory(...Object.values(deps), current, request);
  assert.equal(result.fileJobs[0], current);
  const added = result.fileJobs[1];
  assert.equal(added.status, "queued");
  assert.notEqual(added.id, current.id);
  assert.deepEqual({ path: added.path, profileId: added.profileId, prompt: added.prompt, diarize: added.diarize }, request);
  assert.equal(queue.nextQueuedFile(result.fileJobs, false), null);
  assert.equal(queue.nextQueuedFile(queue.updateFileJob(result.fileJobs, current.id, "completed"), false).id, added.id);
});
