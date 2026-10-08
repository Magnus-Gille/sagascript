// Isolated visual smoke for update recovery. No native app, audio, or user data.
import assert from "node:assert/strict";
import { readFile, mkdir } from "node:fs/promises";
import { join } from "node:path";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const outputDir = process.env.QA_OUTPUT_DIR || "/private/tmp/sagascript-255-qa";
await mkdir(outputDir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 760, height: 960 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(error.stack || error.message));
const mock = await readFile(new URL("./fixtures/transcription-browser-mock.js", import.meta.url), "utf8");
const diarization = {
  schema_version: 1,
  source_sha256: "a".repeat(64),
  duration_seconds: 4,
  build_revision: "fixture-revision",
  build_version: "fixture-version",
  diagnostics_included: false,
  decoder: null,
  asr_segments: [],
  transcript_modified: true,
  parameters: {
    threshold: 0.5,
    min_segment_seconds: 0.1,
    min_gap_seconds: 0.1,
    min_speaker_seconds: 0.1,
    absorb_max_distance: 0.5,
    hint_merge_max_distance: 0.5,
  },
  activity: [
    { start: 0, end: 1, speakers: ["spk-1", "spk-2"] },
    { start: 1, end: 2, speakers: ["spk-1", "spk-2", "spk-3"] },
    { start: 2, end: 4, speakers: ["spk-3"] },
  ],
  regions: [{
    index: 0, start: 1, end: 1, track: 0, speaker: "spk-1", embedding_status: "degenerate",
    assigned_centroid_distance: null, nearest_other_centroid_distance: null,
    used_track_fallback: true, active_speech_seconds: 0, overlapping_speech_seconds: 0,
  }],
  attributions: [{
    index: 0, start: 1, end: 1, speaker: "spk-1", reason: "invalid_timestamp", support: [],
    margin_seconds: null, gap_seconds: null,
  }],
};
const transcript = {
  schema_version: 2, source_sha256: diarization.source_sha256, language: "sv", model: "fixture",
  duration_seconds: 4,
  segments: [{ id: "seg-1", start: 0, end: 4, text: "Hej från mötet", speaker: "spk-1" }],
  speakers: [{ id: "spk-1", label: "Speaker 1" }],
  diarization,
};
const review = {
  schema_version: 1, original: transcript, original_revision: "original", generation: 0,
  batches: [], revision: "revision-0",
};
const recovery = {
  schema_version: 1, saved_at: "2026-09-26T10:00:00Z",
  dictation: { text: "Återställd diktering" },
  files: [{ job_id: "file-1", path: "/fixtures/recovered.wav", text: "Återställd filtext" }],
  meetings: [{
    job_id: "meeting-1", path: "/fixtures/recovered-meeting.wav", review: { review, transcript },
    editor_draft: { labels: { "spk-1": "Anna" }, mergeTargets: {},
      texts: { "seg-1": "Redigerat möte" }, speakers: { "seg-1": "spk-1" } },
    proposal: null,
  }],
};
if (process.env.QA_ONLY === "file") recovery.meetings = [];
if (process.env.QA_ONLY === "meeting") recovery.files = [];
const url = process.env.QA_URL || "http://127.0.0.1:5243/?tab=transcribe";
let corruptRecovery = false;
let nativeFirst = false;
await page.route(url, (route) => route.fulfill({ contentType: "text/html",
  body: `<html><head><meta charset="utf-8"><title>Recovery QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script>window.qaRecovery=${JSON.stringify(recovery)};window.qaRecoveryLoadError=${corruptRecovery};window.qaRecoveryDelayMs=${nativeFirst ? 650 : 300};window.qaLastNativeDictation="Earlier native result";window.qaLastNativeDelayMs=${nativeFirst ? 0 : 650};</script><script type="module">${mock}</script></body></html>`,
}));

try {
  await page.goto(url);
  await page.waitForFunction(() => window.qa?.calls.some((call) => call.cmd === "load_update_recovery"));
  await page.evaluate(() => window.qa.prepareUpdate("qa-early-nonce"));
  await page.waitForFunction(() => window.qa.calls.some((call) =>
    call.cmd === "complete_update_preparation" && call.args.nonce === "qa-early-nonce"));
  const earlyCalls = await page.evaluate(() => window.qa.calls);
  const earlySaved = earlyCalls.find((call) => call.cmd === "save_update_recovery");
  assert.equal(earlySaved.args.payload.files.length, 1);
  assert.equal(earlySaved.args.payload.meetings.length, 1);
  await page.evaluate(() => window.qa.abortUpdate());
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).waitFor({ timeout: 5000 });
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).click();
  assert.equal(await page.locator('[role="tabpanel"]:visible textarea.transcribe-result').inputValue(), "Återställd filtext");
  await page.screenshot({ path: join(outputDir, "sagascript-update-recovered-file.png"), fullPage: true });
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).click();
  await page.getByRole("heading", { name: "Acoustic activity" }).waitFor();
  await page.getByText(/Simultaneous speaker activity appears in 2 intervals/).waitFor();
  await page.getByText("Transcript edited", { exact: true }).waitFor();
  await page.getByText(/does not duplicate words/).waitFor();
  await page.getByText(/Additional diagnostic evidence was not included for this run\./).waitFor();
  const speakerDraft = page.getByRole("textbox", { name: "Rename Speaker 1" });
  assert.equal(await speakerDraft.inputValue(), "Anna");
  assert.equal(await page.locator('[role="tabpanel"]:visible .meeting-progress').count(), 0,
    "restored reviews have no measured processing durations");
  await speakerDraft.scrollIntoViewIfNeeded();
  await page.screenshot({ path: join(outputDir, "sagascript-update-recovered-meeting.png"), fullPage: true });
  await page.evaluate(() => window.qa.prepareUpdate("qa-nonce"));
  await page.waitForFunction(() => window.qa.calls.some((call) =>
    call.cmd === "complete_update_preparation" && call.args.nonce === "qa-nonce"));
  const calls = await page.evaluate(() => window.qa.calls);
  const saved = calls.findLast((call) => call.cmd === "save_update_recovery");
  assert.equal(saved.args.payload.files.length, 1);
  assert.equal(saved.args.payload.meetings.length, 1);
  assert.equal(saved.args.payload.dictation.text, "Återställd diktering");
  const saveCount = await page.evaluate(() => window.qa.calls.filter(call => call.cmd === "save_update_recovery").length);
  await page.evaluate(() => { window.qaNativePending = true; window.qaLastNativeDictation = "New unsaved dictation"; window.qa.prepareUpdate("qa-conflicting-native-result"); });
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-conflicting-native-result"));
  const collision = await page.evaluate(() => ({
    ack: window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args,
    saves: window.qa.calls.filter(call => call.cmd === "save_update_recovery").length,
  }));
  assert.match(collision.ack.error, /different unsaved dictation/i);
  assert.equal(collision.saves, saveCount, "a second pending result must never replace the recovered draft");
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.getByRole("button", { name: "Copy result" }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "New unsaved dictation");
  assert.equal(await page.evaluate(() => window.qaNativePending), true, "copying the recovered draft must leave the other native result pending");
  nativeFirst = true;
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Återställd diktering");
  await page.locator("textarea.test-result").fill("Återställd diktering, korrigerad");
  await page.evaluate(() => window.qa.prepareUpdate("qa-edited-recovered"));
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-edited-recovered"));
  assert.equal(await page.evaluate(() => window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args.error), null);
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Återställd diktering, korrigerad");
  await page.evaluate(() => window.qa.abortUpdate());
  await page.evaluate(() => { window.qaNativePending = true; window.qaLastNativeDictation = "New unsaved dictation"; window.qa.dictationResult("New unsaved dictation"); });
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  const beforeRedictation = await page.evaluate(() => window.qa.calls.filter(call => call.cmd === "save_update_recovery").length);
  await page.evaluate(() => window.qa.prepareUpdate("qa-redictation-conflict"));
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-redictation-conflict"));
  const redictation = await page.evaluate(() => ({
    ack: window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args,
    saves: window.qa.calls.filter(call => call.cmd === "save_update_recovery").length,
  }));
  assert.match(redictation.ack.error, /different unsaved dictation/i);
  assert.equal(redictation.saves, beforeRedictation);

  // A successful native delivery must finish its pending/edited cleanup even
  // when rereading another persisted recovery branch discovers an unknown
  // field. The invalid bytes remain untouched and no clear/save is attempted.
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.evaluate(() => {
    window.qaRecovery.meetings[0].unknown_recovery_field = "retained";
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native result to copy";
    window.qa.dictationResult("Native result to copy");
  });
  await page.locator("textarea.test-result").fill("Native result edited before copy");
  const copyCallsBefore = await page.evaluate(() => window.qa.calls.length);
  await page.getByRole("button", { name: "Copy result", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  const copyResult = await page.evaluate((start) => ({
    pending: window.qaNativePending,
    mutations: window.qa.calls.slice(start).filter(call => ["save_update_recovery", "clear_update_recovery"].includes(call.cmd)),
    retained: window.qaRecovery.meetings[0].unknown_recovery_field,
  }), copyCallsBefore);
  assert.equal(copyResult.pending, false, "Copy acknowledges the native result despite retained recovery");
  assert.deepEqual(copyResult.mutations, [], "Copy does not mutate unreadable recovery bytes");
  assert.equal(copyResult.retained, "retained");
  await page.getByRole("alert").filter({ hasText: "Update recovery draft was retained" }).waitFor();

  // Save follows the same successful-delivery path and must retain the draft.
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.evaluate(() => {
    window.qaRecovery.meetings[0].unknown_recovery_field = "retained-for-save";
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native result to save";
    window.qa.dictationResult("Native result to save");
  });
  await page.locator("textarea.test-result").fill("Native result edited before save");
  const saveCallsBefore = await page.evaluate(() => window.qa.calls.length);
  await page.getByRole("button", { name: /Save result/ }).click();
  await page.getByText("Saved.", { exact: true }).waitFor();
  const saveResult = await page.evaluate((start) => ({
    pending: window.qaNativePending,
    mutations: window.qa.calls.slice(start).filter(call => ["save_update_recovery", "clear_update_recovery"].includes(call.cmd)),
    retained: window.qaRecovery.meetings[0].unknown_recovery_field,
  }), saveCallsBefore);
  assert.equal(saveResult.pending, false, "Save acknowledges the native result despite retained recovery");
  assert.deepEqual(saveResult.mutations, [], "Save does not mutate unreadable recovery bytes");
  assert.equal(saveResult.retained, "retained-for-save");
  await page.getByRole("alert").filter({ hasText: "Update recovery draft was retained" }).waitFor();

  // A proposal branch that becomes invalid after restore must block updater
  // preparation while preserving the exact saved bytes.
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  const proposalInvalidSnapshot = await page.evaluate(() => {
    window.qaRecovery.meetings[0].proposal = { proposal: { proposed: { invalid: true } } };
    return JSON.stringify(window.qaRecovery);
  });
  await page.evaluate(() => window.qa.prepareUpdate("qa-invalid-proposal"));
  await page.waitForFunction(() => window.qa.calls.some(call =>
    call.cmd === "complete_update_preparation" && call.args.nonce === "qa-invalid-proposal"));
  const invalidProposal = await page.evaluate(() => ({
    ack: window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args,
    mutations: window.qa.calls.filter(call => ["save_update_recovery", "clear_update_recovery"].includes(call.cmd)),
    recovery: JSON.stringify(window.qaRecovery),
  }));
  assert.match(invalidProposal.ack.error, /cannot be read without discarding data/i);
  assert.deepEqual(invalidProposal.mutations, [], "invalid proposal bytes cannot be overwritten or cleared");
  assert.equal(invalidProposal.recovery, proposalInvalidSnapshot);

  corruptRecovery = true;
  await page.goto(url);
  await page.getByRole("alert").filter({ hasText: "Update blocked: unreadable recovery drafts were retained" }).waitFor();
  await page.evaluate(() => window.qa.prepareUpdate("qa-unreadable"));
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-unreadable"));
  const rejected = await page.evaluate(() => window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args);
  assert.match(rejected.error, /Synthetic recovery read failure/);
  corruptRecovery = false;
  recovery.meetings[0].review.transcript.diarization.schema_version = 99;
  const unreadableSnapshot = JSON.stringify(recovery);
  await page.goto(url);
  await page.getByRole("alert").filter({ hasText: "Update blocked: unreadable recovery drafts were retained" }).waitFor();
  await page.evaluate(() => window.qa.prepareUpdate("qa-invalid-acoustic-report"));
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-invalid-acoustic-report"));
  const invalidDraft = await page.evaluate(() => ({
    ack: window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args,
    mutations: window.qa.calls.filter(call => ["save_update_recovery", "clear_update_recovery"].includes(call.cmd)),
    recovery: JSON.stringify(window.qaRecovery),
  }));
  assert.match(invalidDraft.ack.error, /cannot be read/);
  assert.deepEqual(invalidDraft.mutations, [], "invalid persisted drafts cannot be overwritten or cleared");
  assert.equal(invalidDraft.recovery, unreadableSnapshot);
  await page.screenshot({ path: join(outputDir, "sagascript-update-unreadable-acoustic-drafts.png"), fullPage: true });
  assert.deepEqual(errors, []);
  console.log("PASS: recovered dictation/file/meeting visible, meeting draft restored, update snapshot acknowledged; no page errors.");
} catch (error) {
  console.error("Recovery QA diagnostics", {
    errors,
    body: await page.locator("body").innerText().catch(() => "<unavailable>"),
    calls: await page.evaluate(() => window.qa?.calls ?? []).catch(() => []),
  });
  await page.screenshot({ path: join(outputDir, "sagascript-update-recovery-failure.png"), fullPage: true }).catch(() => undefined);
  throw error;
} finally {
  await browser.close();
}
