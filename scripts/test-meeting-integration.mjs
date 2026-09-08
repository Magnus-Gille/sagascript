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
const typesModule = ts.transpileModule(typesSource, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const { initialMeetingDrafts, reconcileMeetingDrafts } = await import(
  `data:text/javascript;base64,${Buffer.from(typesModule).toString("base64")}`
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
  assert.match(settingsSource, /meetingActionQueue = queued\.then\(\(\) => undefined, \(\) => undefined\)/);
  assert.match(settingsSource, /await waitForMeetingActions\(\)/);
  assert.match(settingsSource, /meetingDocumentRevision/);
  assert.match(settingsSource, /generation !== meetingPollGeneration/);
  const importBody = settingsSource.split("async function startMeetingFileTranscription(")[1]
    .split("async function handleFileTranscription(")[0];
  assert.doesNotMatch(importBody, /\+\+meetingDocumentRevision|meetingDocumentRevision\s*\+=/,
    "starting or failing an import must not remount the previous review's unsaved drafts");
  assert.match(settingsSource, /meetingFailureText\(error, "Could not check meeting progress\."\)/);
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

test("meeting review exposes explicit corrections, playback, and all export formats", () => {
  for (const field of ["schema_version", "source_sha256", "language", "model", "duration_seconds", "segments", "speakers"]) {
    assert.match(typesSource, new RegExp(`\\b${field}\\b`));
  }
  for (const format of ["plain", "markdown", "json", "srt", "vtt"]) {
    assert.match(reviewSource, new RegExp(`format: "${format}"`));
  }
  assert.match(reviewSource, /onApply: \(operations: CorrectionOperation\[\]\) => Promise<void>/);
  assert.match(reviewSource, /onUndo: \(\) => Promise<void>/);
  assert.match(reviewSource, /onReset: \(\) => Promise<void>/);
  assert.match(reviewSource, /onSave: \(\) => Promise<boolean>/);
  assert.match(reviewSource, /onExport: \(format: MeetingExportFormat\) => Promise<boolean>/);
  assert.match(reviewSource, /Save review/);
  assert.match(reviewSource, /Unapplied edits are not included in saves\/exports/);
  assert.match(reviewSource, /function persistenceDisabled\(\): boolean/);
  assert.match(reviewSource, /disabled=\{persistenceDisabled\(\)\}/);
  assert.match(reviewSource, /const hasUnsavedDrafts = \$derived\.by/);
  assert.match(reviewSource, /function discardMerge\(speakerId: string\)/);
  assert.match(reviewSource, /Clear merge selection/);
  assert.match(reviewSource, /actionNotice/);
  assert.match(reviewSource, /Audio access is temporary and is detached when you leave this review/);
  assert.match(reviewSource, /Apply/);
  assert.match(reviewSource, /Discard/);
  assert.match(reviewSource, /convertFileSrc\(attachment\.token, "meeting-audio"\)/);
  assert.match(reviewSource, /onwheel=\{disableFollow\}/);
  assert.match(reviewSource, /ontouchmove=\{disableFollow\}/);
  assert.match(reviewSource, /svelte:window onkeydown=\{handleKeyboardScroll\}/);
  assert.match(reviewSource, /activeSegmentsAtTime/);
  assert.match(reviewSource, /audioUrl \? activeSegmentsAtTime/);
  assert.match(reviewSource, /onerror=\{handleAudioError\}/);
  assert.match(reviewSource, /This audio cannot be played/);
  assert.doesNotMatch(reviewSource, />Playing:/, "paused cursor highlights must not claim playback");
  assert.match(settingsSource, /async function exportMeetingReview\(format: MeetingExportFormat\): Promise<boolean>/);
  assert.match(settingsSource, /async function saveCurrentMeetingReview\(\): Promise<boolean>/);
  assert.match(settingsSource, /saveMeetingReview\(review, format\)/);
  assert.match(settingsSource, /saveMeetingReview\(review, "json"\)/);
  for (const command of [
    "create_meeting_review",
    "apply_meeting_corrections",
    "undo_meeting_review",
    "reset_meeting_review",
    "open_meeting_review",
    "save_meeting_review",
    "attach_meeting_audio",
    "detach_meeting_audio",
  ]) {
    assert.match(apiSource, new RegExp(`invoke\\("${command}"`));
  }
  assert.match(settingsSource, /createMeetingReview\(transcript\)/);
  assert.match(settingsSource, /meetingReviewInit = initializeMeetingReview/);
  assert.match(settingsSource, /Open saved review/);
  assert.match(reviewSource, /reconcileMeetingDrafts/);
  assert.match(reviewSource, /resetDraftKey/);
  assert.match(reviewSource, /onDraftDirtyChange\?: \(dirty: boolean\) => void/);
  assert.match(reviewSource, /committedResetKey === resetDraftKey/);
  assert.match(reviewSource, /Undo the last correction and discard unsaved edits/);
  assert.match(settingsSource, /replace the current review if it succeeds/);
  assert.match(settingsSource, /meetingReviewDraftDirty/);
  assert.match(settingsSource, /onDraftDirtyChange=\{onMeetingReviewDraftDirtyChange\}/);
  assert.match(settingsSource, /Leave meeting review and discard unapplied edits/);
  assert.match(settingsSource, /const stillCurrent =/);
  assert.match(settingsSource, /if \(!stillCurrent\)[\s\S]*detachMeetingAudio\(attachment\.token\)/);
  assert.match(reviewSource, /review\.original\.segments/);
  assert.doesNotMatch(reviewSource, /{@html/);
  assert.doesNotMatch(reviewSource, /localStorage|fetch\(|AudioContext|MediaRecorder/);
  assert.doesNotMatch(reviewSource, /\.play\(\)/, "editing and timestamp controls must not autoplay audio");
});

test("review draft reconciliation preserves dirty segment B while applying segment A and another speaker rename", () => {
  const transcript = {
    schema_version: 1,
    source_sha256: "source-a",
    language: "en",
    model: "model",
    duration_seconds: 4,
    segments: [
      { id: "a", start: 0, end: 1, text: "A original", speaker: "s1" },
      { id: "b", start: 1, end: 2, text: "B original", speaker: "s2" },
    ],
    speakers: [
      { id: "s1", label: "Speaker 1" },
      { id: "s2", label: "Speaker 2" },
    ],
  };
  const drafts = initialMeetingDrafts(transcript);
  drafts.texts.a = "A saved";
  drafts.texts.b = "B still being edited";
  const afterSegmentA = {
    ...transcript,
    segments: [{ ...transcript.segments[0], text: "A saved" }, transcript.segments[1]],
  };
  const reconciledA = reconcileMeetingDrafts(
    transcript,
    afterSegmentA,
    drafts,
    [{ kind: "edit_segment", segment_id: "a", text: "A saved" }],
  );
  assert.equal(reconciledA.texts.a, "A saved");
  assert.equal(reconciledA.texts.b, "B still being edited");

  reconciledA.labels.s2 = "Unsubmitted speaker name";
  const afterSpeakerRename = {
    ...afterSegmentA,
    speakers: [{ id: "s1", label: "Alice" }, afterSegmentA.speakers[1]],
  };
  const reconciledB = reconcileMeetingDrafts(
    afterSegmentA,
    afterSpeakerRename,
    reconciledA,
    [{ kind: "rename_speaker", speaker_id: "s1", label: "Alice" }],
  );
  assert.equal(reconciledB.labels.s1, "Alice");
  assert.equal(reconciledB.labels.s2, "Unsubmitted speaker name");
  assert.equal(reconciledB.texts.b, "B still being edited");
});
