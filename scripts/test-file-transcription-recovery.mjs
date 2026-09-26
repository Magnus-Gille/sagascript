import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../src/lib/FileTranscription.svelte", import.meta.url), "utf8");

test("file transcription exposes job-scoped recovery hydration and update callbacks", () => {
  assert.match(source, /initialRecoveryFile\?: UpdateRecoveryFile \| null/);
  assert.match(source, /initialRecoveryMeeting\?: UpdateRecoveryMeeting \| null/);
  assert.match(source, /onFileRecoveryChange\?: \(entry: UpdateRecoveryFile \| null\)/);
  assert.match(source, /onMeetingRecoveryChange\?: \(entry: UpdateRecoveryMeeting \| null\)/);
  assert.match(source, /recovery\.job_id !== job\.id/);
  assert.match(source, /transcriptionResult = recovery\.text/);
  assert.match(source, /acceptMeetingReview\(recovery\.review, generation, true\)/);
});

test("meeting recovery restores editor drafts and proposals through MeetingReview", () => {
  assert.match(source, /meetingRecoveryDraft = \{/);
  assert.match(source, /editor_draft:/);
  assert.match(source, /meetingProposal = recovery\.proposal/);
  assert.match(source, /initialDraftSnapshot=\{meetingRecoveryDraft\}/);
  assert.match(source, /onDraftSnapshotChange=\{onMeetingReviewDraftSnapshotChange\}/);
  assert.match(source, /onMeetingRecoveryChange\(\{[\s\S]*editor_draft: snapshot\.drafts/);
});

test("explicit file and review saves clear recovery while unmounting stays silent", () => {
  assert.match(source, /onFileRecoveryChange\(null\)/);
  assert.match(source, /onMeetingRecoveryChange\(null\)/);
  const destroyBody = source.match(/onDestroy\(\(\) => \{([\s\S]*?)\n  \}\);/)?.[1] ?? "";
  assert.doesNotMatch(destroyBody, /on(?:File|Meeting)RecoveryChange\(null\)/);
});
