import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");

test("Settings restores every persisted result as a completed, reviewable job", () => {
  assert.match(source, /parseUpdateRecoveryPayload\(await loadUpdateRecovery\(\)\)/);
  assert.match(source, /status: "completed" as const/);
  assert.match(source, /initialRecoveryFile=\{fileRecoveryEntries\.find/);
  assert.match(source, /initialRecoveryMeeting=\{meetingRecoveryEntries\.find/);
  assert.match(source, /Recovered drafts/);
  assert.match(source, /onclick=\{\(\) => void discardRecoveredDrafts\(\)\}/);
});

test("update preparation snapshots current dictation, file, and meeting entries", () => {
  const preparation = source.slice(source.indexOf("async function prepareForUpdate"), source.indexOf("async function copyTestResult"));
  assert.match(preparation, /await tick\(\)/);
  assert.match(preparation, /createUpdateRecoveryPayload\(\{/);
  assert.match(preparation, /dictation: testResultRecoveryPending && testResult\.trim\(\) \? \{ text: testResult \} : null/);
  assert.match(preparation, /files: fileRecoveryEntries/);
  assert.match(preparation, /meetings: meetingRecoveryEntries/);
  assert.match(preparation, /serializeUpdateRecoveryPayload\(payload\)/);
  assert.match(preparation, /await saveUpdateRecovery\(payload\)/);
  assert.match(preparation, /await completeUpdatePreparation\(nonce, null\)/);
  assert.match(preparation, /await completeUpdatePreparation\(nonce, message\)/);
});

test("recovery callbacks are wired to clear entries after explicit result actions", () => {
  assert.match(source, /onFileRecoveryChange=\{\(entry\) => onFileRecoveryChange\(entry, job\.id\)\}/);
  assert.match(source, /onMeetingRecoveryChange=\{\(entry\) => onMeetingRecoveryChange\(entry, job\.id\)\}/);
  assert.match(source, /function onFileRecoveryChange\(entry: UpdateRecoveryFile \| null, jobId\?: string\)/);
  assert.match(source, /function onMeetingRecoveryChange\(entry: UpdateRecoveryMeeting \| null, jobId\?: string\)/);
  assert.match(source, /if \(entry === null\)/);
  assert.match(source, /testResultRecoveryPending = false/);
});

test("recovered jobs cannot trigger automatic transcription or paste", () => {
  const recoveryBlock = source.slice(source.indexOf("async function restoreUpdateRecovery"), source.indexOf("async function discardRecoveredDrafts"));
  assert.doesNotMatch(recoveryBlock, /handleFileTranscription|startMeetingFileTranscription|paste/);
  assert.match(recoveryBlock, /status: "completed" as const/);
});
