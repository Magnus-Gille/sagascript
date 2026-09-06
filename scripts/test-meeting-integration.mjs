import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const settingsSource = await readFile(
  new URL("../src/lib/Settings.svelte", import.meta.url),
  "utf8",
);
const apiSource = await readFile(new URL("../src/lib/api.ts", import.meta.url), "utf8");
const reviewSource = await readFile(
  new URL("../src/lib/MeetingReview.svelte", import.meta.url),
  "utf8",
);
const pollingSource = await readFile(
  new URL("../src/lib/meeting-job-client.ts", import.meta.url),
  "utf8",
);
const typesSource = await readFile(
  new URL("../src/lib/meeting-types.ts", import.meta.url),
  "utf8",
);
const pollingModule = ts.transpileModule(pollingSource, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { pollMeetingJob } = await import(
  `data:text/javascript;base64,${Buffer.from(pollingModule).toString("base64")}`
);

test("diarized imports use the job API while ordinary imports keep transcribeFile", () => {
  assert.match(apiSource, /invoke\("begin_meeting_file", \{ filePath, prompt, profileId \}\)/);
  assert.match(apiSource, /invoke\("get_meeting_job", \{ jobId \}\)/);
  assert.match(apiSource, /invoke\("cancel_meeting_job", \{ jobId \}\)/);
  assert.match(settingsSource, /if \(transcribeDiarize\) \{\s*await startMeetingFileTranscription/s);
  assert.match(settingsSource, /beginMeetingFile\(filePath, prompt, profileId\)/);
  assert.match(settingsSource, /transcribeFile\(filePath, \{[\s\S]*?diarize: false/);
  assert.match(settingsSource, /disabled=\{transcribing\}/);
});

test("polling is serialized, stale generations are ignored, and cancellation waits for terminal state", async () => {
  assert.match(settingsSource, /pollMeetingJobClient\(\{/);
  assert.match(pollingSource, /snapshot = await options\.get\(options\.jobId\)/);
  assert.match(pollingSource, /await options\.wait\(\)/);
  assert.match(settingsSource, /generation !== meetingPollGeneration/);
  assert.match(settingsSource, /meetingJobStatus === "cancelling"/);
  assert.match(settingsSource, /transcribing = false;[\s\S]*?snapshot\.status === "completed"/);
  assert.match(settingsSource, /meetingPollingFailed = true/);
  assert.match(settingsSource, /Retry status check/);
  assert.match(settingsSource, /Meeting completed without a transcript/);
  assert.match(settingsSource, /meetingActionQueue = queued\.catch/);
  assert.match(settingsSource, /await waitForMeetingActions\(\)/);
  assert.match(settingsSource, /meetingDocumentRevision/);
  assert.match(settingsSource, /\{#key meetingDocumentRevision\}/);
  assert.match(settingsSource, /generation !== meetingPollGeneration \|\| meetingJobId !== jobId/);

  const snapshots = [
    { id: "job-1", status: "running", phase: "loading", error: null, transcript: null },
    { id: "job-1", status: "cancelling", phase: "cancelling", error: null, transcript: null },
    { id: "job-1", status: "cancelled", phase: "cancelled", error: null, transcript: null },
  ];
  const calls = [];
  let inFlight = 0;
  let maxInFlight = 0;
  const seen = [];
  let waits = 0;
  let busy = true;
  const fakeApi = {
    get: async () => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      calls.push("get");
      const snapshot = snapshots.shift();
      inFlight -= 1;
      return snapshot;
    },
  };
  await pollMeetingJob({
    jobId: "job-1",
    get: fakeApi.get,
    isCurrent: () => true,
    onSnapshot: (snapshot) => {
      seen.push(snapshot.status);
      if (["completed", "cancelled", "failed"].includes(snapshot.status)) busy = false;
    },
    onFailure: assert.fail,
    wait: async () => { waits += 1; },
  });
  assert.deepEqual(calls, ["get", "get", "get"]);
  assert.deepEqual(seen, ["running", "cancelling", "cancelled"]);
  assert.equal(waits, 2);
  assert.equal(maxInFlight, 1);
  assert.equal(busy, false);

  let current = true;
  let staleSnapshots = 0;
  let staleFailure = 0;
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const stalePoll = pollMeetingJob({
    jobId: "job-stale",
    get: async () => pending,
    isCurrent: () => current,
    onSnapshot: () => { staleSnapshots += 1; },
    onFailure: () => { staleFailure += 1; },
    wait: async () => {},
  });
  current = false;
  release({ id: "job-stale", status: "completed", phase: "done", error: null, transcript: null });
  await stalePoll;
  assert.equal(staleSnapshots, 0);
  assert.equal(staleFailure, 0);

  let foreignError = "";
  let foreignSnapshots = 0;
  await pollMeetingJob({
    jobId: "job-owned",
    get: async () => ({ id: "job-foreign", status: "completed", phase: "done", error: null, transcript: null }),
    isCurrent: () => true,
    onSnapshot: () => { foreignSnapshots += 1; },
    onFailure: (error) => { foreignError = error.message; },
    wait: async () => assert.fail("foreign snapshots must not wait"),
  });
  assert.match(foreignError, /identity changed/);
  assert.equal(foreignSnapshots, 0);

  let retry = false;
  busy = true;
  await pollMeetingJob({
    jobId: "job-error",
    get: async () => { throw new Error("temporary status outage"); },
    isCurrent: () => true,
    onSnapshot: () => assert.fail("failed status read must not produce a snapshot"),
    onFailure: () => { retry = true; },
    wait: async () => assert.fail("failed status read must not wait"),
  });
  assert.equal(busy, true);
  assert.equal(retry, true);

  let preservedTranscript = { source_sha256: "old-document" };
  let missingTranscriptError = "";
  await pollMeetingJob({
    jobId: "job-missing-transcript",
    get: async () => ({ id: "job-missing-transcript", status: "completed", phase: "done", error: null, transcript: null }),
    isCurrent: () => true,
    onSnapshot: (snapshot) => {
      if (snapshot.status === "completed" && snapshot.transcript) preservedTranscript = snapshot.transcript;
      else if (snapshot.status === "completed") missingTranscriptError = "Meeting completed without a transcript.";
    },
    onFailure: assert.fail,
    wait: async () => assert.fail("terminal status must not wait"),
  });
  assert.deepEqual(preservedTranscript, { source_sha256: "old-document" });
  assert.equal(missingTranscriptError, "Meeting completed without a transcript.");
});

test("meeting review exposes typed schema, callback-only edits, and all export formats", () => {
  for (const field of ["schema_version", "source_sha256", "language", "model", "duration_seconds", "segments", "speakers"]) {
    assert.match(typesSource, new RegExp(`\\b${field}\\b`));
  }
  for (const format of ["plain", "markdown", "json", "srt", "vtt"]) {
    assert.match(reviewSource, new RegExp(`format: "${format}"`));
  }
  assert.match(reviewSource, /onRename: \(id: string, label: string\) => Promise<void>/);
  assert.match(reviewSource, /onMerge: \(from: string, into: string\) => Promise<void>/);
  assert.match(reviewSource, /onExport: \(format: MeetingExportFormat\) => Promise<void>/);
  for (const command of ["rename_meeting_speaker", "merge_meeting_speakers", "save_meeting_export"]) {
    assert.match(apiSource, new RegExp(`invoke\\("${command}"`));
  }
  assert.match(reviewSource, /draftSourceSha !== transcript\.source_sha256/);
  assert.doesNotMatch(reviewSource, /{@html/);
  assert.doesNotMatch(reviewSource, /localStorage|fetch\(|AudioContext|MediaRecorder/);
});
