import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Source-text assertions over the meeting UI, left for phase B of #361. The
// module tests moved to Vitest: polling behaviour to
// src/lib/meeting-job-client.test.ts, draft snapshots (now
// src/lib/meeting-review-drafts.ts) to meeting-review-drafts.test.ts, and
// draft reconciliation to meeting-types.test.ts.

const settingsSource = await readFile(
  new URL("../src/lib/Settings.svelte", import.meta.url),
  "utf8",
);
const fileTranscriptionSource = await readFile(
  new URL("../src/lib/FileTranscription.svelte", import.meta.url),
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

test("diarized imports use the job API while ordinary imports keep transcribeFile", () => {
  assert.match(apiSource, /invoke\("begin_meeting_file", \{ filePath, prompt, profileId \}\)/);
  assert.match(apiSource, /invoke\("get_meeting_job", \{ jobId \}\)/);
  assert.match(apiSource, /invoke\("cancel_meeting_job", \{ jobId \}\)/);
  assert.match(fileTranscriptionSource, /if \(job\.diarize\) \{\s*await startMeetingFileTranscription\(filePath, prompt, profileId\)/s);
  assert.match(fileTranscriptionSource, /const profileId = job\.profileId/);
  assert.match(fileTranscriptionSource, /const prompt = job\.prompt/);
  assert.match(fileTranscriptionSource, /beginMeetingFile\(filePath, prompt, profileId\)/);
  assert.match(fileTranscriptionSource, /transcribeFile\(filePath, \{[\s\S]*?diarize: false/);
  assert.match(settingsSource, /disabled=\{transcribing\}/);
  assert.match(settingsSource, /createFileJobs\(paths, \{[\s\S]*diarize: transcribeDiarize/);
  assert.match(settingsSource, /prompt: transcribePrompt\.trim\(\) \|\| null/);
  assert.match(settingsSource, /profileId: selectedTranscribeProfile\(\)\?\.id \?\? null/);
  assert.match(
    settingsSource,
    /\{#each fileJobs as job \(job\.id\)\}[\s\S]*?hidden=\{selectedFileId !== job\.id\}[\s\S]*?<FileTranscription \{job\}/,
    "file panels stay keyed and mounted while inactive panels are hidden",
  );
});

test("polling is serialized, stale generations are ignored, and cancellation waits for terminal state", async () => {
  assert.match(fileTranscriptionSource, /pollMeetingJobClient\(\{/);
  assert.match(pollingSource, /snapshot = await options\.get\(options\.jobId\)/);
  assert.match(pollingSource, /await options\.wait\(\)/);
  assert.match(fileTranscriptionSource, /generation !== meetingPollGeneration/);
  assert.match(fileTranscriptionSource, /meetingJobStatus === "cancelling"/);
  assert.match(fileTranscriptionSource, /transcribing = false;[\s\S]*?snapshot\.status === "completed"/);
  assert.match(fileTranscriptionSource, /meetingPollingFailed = true/);
  assert.match(fileTranscriptionSource, /Retry status check/);
  assert.match(fileTranscriptionSource, /Meeting completed without a transcript/);
  assert.match(fileTranscriptionSource, /meetingActionQueue = queued\.then\(\(\) => undefined, \(\) => undefined\)/);
  assert.match(fileTranscriptionSource, /await waitForMeetingActions\(\)/);
  assert.match(fileTranscriptionSource, /meetingDocumentRevision/);
  assert.match(fileTranscriptionSource, /generation !== meetingPollGeneration/);
  const importBody = fileTranscriptionSource.split("async function startMeetingFileTranscription(")[1]
    .split("async function handleFileTranscription(")[0];
  assert.doesNotMatch(importBody, /\+\+meetingDocumentRevision|meetingDocumentRevision\s*\+=/,
    "starting or failing an import must not remount the previous review's unsaved drafts");
  assert.match(fileTranscriptionSource, /meetingFailureText\(error, "Could not check meeting progress\."\)/);
  assert.match(fileTranscriptionSource, /generation !== meetingPollGeneration \|\| meetingJobId !== jobId/);
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
  assert.match(reviewSource, /if \(!active && audioEl\) audioEl\.pause\(\)/);
  assert.match(reviewSource, /Audio pauses when you switch tabs\. Audio access ends when you quit Sagascript/);
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
  assert.match(fileTranscriptionSource, /async function exportMeetingReview\(format: MeetingExportFormat\): Promise<boolean>/);
  assert.match(fileTranscriptionSource, /async function saveCurrentMeetingReview\(\): Promise<boolean>/);
  assert.match(fileTranscriptionSource, /saveMeetingReview\(review, format\)/);
  assert.match(fileTranscriptionSource, /saveMeetingReview\(review, "json"\)/);
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
  assert.match(fileTranscriptionSource, /createMeetingReview\(transcript\)/);
  assert.match(fileTranscriptionSource, /meetingReviewInit = initializeMeetingReview/);
  assert.match(settingsSource, /Open saved meeting/);
  assert.match(reviewSource, /reconcileMeetingDrafts/);
  assert.match(reviewSource, /resetDraftKey/);
  assert.match(reviewSource, /onDraftDirtyChange\?: \(dirty: boolean\) => void/);
  assert.match(reviewSource, /initialDraftSnapshot\?: MeetingReviewDraftSnapshot \| null/);
  assert.match(reviewSource, /onDraftSnapshotChange\?: \(snapshot: MeetingReviewDraftSnapshot\) => void/);
  assert.match(reviewSource, /source_sha256/);
  assert.match(reviewSource, /review_revision/);
  assert.match(reviewSource, /restoreMeetingDraftSnapshot/);
  assert.match(reviewSource, /createMeetingReviewDraftSnapshot/);
  assert.match(reviewSource, /committedResetKey === resetDraftKey/);
  assert.match(reviewSource, /Undo the last correction and discard unsaved edits/);
  assert.match(fileTranscriptionSource, /replace the current review if it succeeds/);
  assert.match(fileTranscriptionSource, /meetingReviewDraftDirty/);
  assert.match(fileTranscriptionSource, /onDraftDirtyChange=\{onMeetingReviewDraftDirtyChange\}/);
  assert.match(fileTranscriptionSource, /const stillCurrent =/);
  assert.match(fileTranscriptionSource, /if \(!stillCurrent\)[\s\S]*detachMeetingAudio\(attachment\.token\)/);
  assert.doesNotMatch(settingsSource, /Leave meeting review and discard unapplied edits/);
  assert.match(reviewSource, /review\.original\.segments/);
  assert.match(reviewSource, /overlapSpans\.slice\(0, 100\)/);
  assert.match(reviewSource, /Additional diagnostic evidence was not included for this run/);
  assert.match(reviewSource, /does not duplicate words/);
  assert.doesNotMatch(reviewSource, /speaker confidence[^\n]*%/i);
  assert.doesNotMatch(reviewSource, /{@html/);
  assert.doesNotMatch(reviewSource, /localStorage|fetch\(|AudioContext|MediaRecorder/);
  assert.doesNotMatch(reviewSource, /\.play\(\)/, "editing and timestamp controls must not autoplay audio");
});
