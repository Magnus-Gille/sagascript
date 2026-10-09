import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib/Settings.svelte", import.meta.url), "utf8");

test("Settings restores every persisted result as a completed, reviewable job", () => {
  assert.match(source, /readPersistedUpdateRecoveryPayload\(await loadUpdateRecovery\(\)\)/);
  assert.match(source, /status: "completed" as const/);
  assert.match(source, /initialRecoveryFile=\{fileRecoveryEntries\.find/);
  assert.match(source, /initialRecoveryMeeting=\{meetingRecoveryEntries\.find/);
  assert.match(source, /Recovered drafts/);
  assert.match(source, /onclick=\{\(\) => void discardRecoveredDrafts\(\)\}/);
});

test("update preparation snapshots current dictation, file, and meeting entries", () => {
  const preparation = source.slice(source.indexOf("async function prepareForUpdate"), source.indexOf("async function copyTestResult"));
  assert.match(preparation, /await recoveryRestore;/);
  assert.match(preparation, /await tick\(\)/);
  assert.match(preparation, /createUpdateRecoveryPayload\(\{/);
  assert.match(preparation, /nativeResultPending = await getUpdateResultPending\("live-dictation"\)/);
  assert.match(preparation, /dictation: testResultRecoveryPending && \(nativeResultPending \|\| recoveredDictationActive \|\| testResultEdited\)/);
  assert.match(preparation, /files: fileRecoveryEntries/);
  assert.match(preparation, /meetings: meetingRecoveryEntries/);
  assert.match(preparation, /serializeUpdateRecoveryPayload\(payload\)/);
  assert.match(preparation, /await saveUpdateRecovery\(payload\)/);
  assert.match(preparation, /await completeUpdatePreparation\(nonce, null\)/);
  assert.match(preparation, /await completeUpdatePreparation\(nonce, message\)/);
});

test("a failed startup recovery read prevents update installation", () => {
  const restore = source.slice(source.indexOf("async function restoreUpdateRecovery"), source.indexOf("async function discardRecoveredDrafts"));
  assert.match(restore, /throw error;/);
  assert.match(restore, /recoveryReadError = `Update blocked: unreadable recovery drafts were retained/);
  assert.match(source, /recoveryRestore = restoreUpdateRecovery\(\)/);
  assert.match(source, /void recoveryRestore\.catch/);
});

test("recovery callbacks are wired to clear entries after explicit result actions", () => {
  assert.match(source, /onFileRecoveryChange=\{\(entry\) => onFileRecoveryChange\(entry, job\.id\)\}/);
  assert.match(source, /onMeetingRecoveryChange=\{\(entry\) => onMeetingRecoveryChange\(entry, job\.id\)\}/);
  assert.match(source, /function onFileRecoveryChange\(entry: UpdateRecoveryFile \| null, jobId\?: string\)/);
  assert.match(source, /function onMeetingRecoveryChange\(entry: UpdateRecoveryMeeting \| null, jobId\?: string\)/);
  assert.match(source, /if \(entry === null\)/);
  assert.match(source, /testResultRecoveryPending = false/);
});

test("recovery cleanup removes only explicitly delivered durable drafts", () => {
  const persistence = source.slice(source.indexOf("function enqueueRecoveryCleanup"), source.indexOf("function persistRemainingRecoveredDrafts"));
  assert.match(persistence, /fileJobIds/);
  assert.match(persistence, /meetingJobIds/);
  assert.match(persistence, /dictationTexts/);
  assert.doesNotMatch(persistence, /createUpdateRecoveryPayload\(\{/);
  assert.match(source, /persistRemainingRecoveredDrafts\(\{ fileJobIds: \[jobId\] \}\)/);
  assert.match(source, /persistRemainingRecoveredDrafts\(\{ meetingJobIds: \[jobId\] \}\)/);
  assert.match(source, /const cleanupTexts = \[persistedText, recoveredText, text\]/);
  assert.match(source, /recoveredText: recoveredDictationText/);
});

test("recovery cleanup warnings identify the durable file and repair path", () => {
  assert.match(source, /update-recovery\.json/);
  assert.match(source, /app data folder/);
  assert.match(source, /restart Sagascript/);
  assert.match(source, /repair update-recovery\.json/);
});

test("all durable recovery mutations share one queue through their acknowledgements", () => {
  assert.match(source, /function enqueueRecoveryMutation<T>\(mutation: \(\) => Promise<T>\)/);
  assert.match(source, /return enqueueRecoveryMutation\(async \(\) => \{[\s\S]*readPersistedUpdateRecoveryPayload\(await loadUpdateRecovery\(\)[\s\S]*await saveUpdateRecovery\(payload\)[\s\S]*await completeUpdatePreparation\(nonce, null\)/);
  assert.match(source, /await enqueueRecoveryMutation\(async \(\) => \{\s*await clearUpdateRecovery\(\);/);
  assert.match(source, /const changed = dictationMatches/);
});

test("dictation recovery replacement is restricted to the current editor lineage", () => {
  assert.match(source, /let dictationEditorLineage = 0/);
  assert.match(source, /let nativeEditorOrigin: \{ text: string; lineage: number \} \| null = null/);
  assert.match(source, /lastPersistedDictation: \{ text: string; lineage: number; revision: number \}/);
  assert.match(source, /recoveryPersistenceRevision = 0/);
  assert.match(source, /persistedRevision: lastPersistedDictation\?\.lineage === lineage/);
  assert.match(source, /dictationPersistenceRevision/);
  assert.match(source, /previous\.dictation\.text === removals\.dictationText/);
  assert.match(source, /lastPersistedDictation\.revision === removals\.dictationPersistenceRevision/);
  assert.match(source, /lastPersistedDictation\.lineage === payloadLineage/);
  assert.match(source, /dictationEditorLineage\+\+/);
  assert.match(source, /const delivery = captureDictationDelivery\(text\)/);
  assert.match(source, /persistedText: lastPersistedDictation\?\.lineage === lineage/);
  assert.match(source, /const ownsPersistedDictation = Boolean/);
  assert.match(source, /previous\.dictation\.text !== payload\.dictation\?\.text/);
});

test("delivery cleanup runs before stale-editor handling and captures native origin", () => {
  assert.match(source, /wasEdited: boolean/);
  assert.match(source, /nativeSourceText: string \| null/);
  assert.match(source, /wasEdited: testResultEdited/);
  assert.match(source, /nativeSourceText: nativeEditorOrigin\?\.lineage === lineage \? nativeEditorOrigin.text : null/);
  assert.match(source, /const cleanupTexts = \[persistedText, recoveredText, text\]/);
  assert.match(source, /await clearDeliveredRecoveryDraft\(cleanupTexts, lineage, text, persistedRevision\);\s*if \(testResult !== text/);
  assert.doesNotMatch(source, /lineage === dictationEditorLineage && !wasRecovered/);
  assert.match(source, /nativeEditorOrigin\?\.lineage === dictationEditorLineage/);
  assert.match(source, /const nativeEditorOwnsResult = nativeEditorOrigin\?\.lineage === dictationEditorLineage/);
  assert.match(source, /!testResult\.trim\(\) && !nativeEditorOwnsResult/);
  assert.match(source, /const payloadLineage = dictationEditorLineage/);
  assert.match(source, /lastPersistedDictation\.lineage === payloadLineage/);
});

test("recovered jobs cannot trigger automatic transcription or paste", () => {
  const recoveryBlock = source.slice(source.indexOf("async function restoreUpdateRecovery"), source.indexOf("async function discardRecoveredDrafts"));
  assert.doesNotMatch(recoveryBlock, /handleFileTranscription|startMeetingFileTranscription|paste/);
  assert.match(recoveryBlock, /status: "completed" as const/);
});

test("recovered-draft persistence rereads before any destructive write", () => {
  const persistence = source.slice(source.indexOf("function enqueueRecoveryCleanup"), source.indexOf("function refreshRecoveredDraftsNotice"));
  assert.match(persistence, /readPersistedUpdateRecoveryPayload\(await loadUpdateRecovery\(\)\)/);
  assert.match(persistence, /readPersistedUpdateRecoveryPayload[\s\S]*clearUpdateRecovery/);
});

test("delivery cleanup retains unreadable recovery drafts without failing Copy or Save", () => {
  const cleanup = source.slice(source.indexOf("async function clearDeliveredRecoveryDraft"), source.indexOf("onMount(()"));
  assert.match(cleanup, /try \{/);
  assert.match(cleanup, /recoveryCleanupError\(error\)/);
  assert.match(cleanup, /console\.warn\("Could not clear delivered update recovery draft"/);
});
