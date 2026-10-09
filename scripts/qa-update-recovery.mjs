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
let initialNativeDictation = "Earlier native result";
let recoveryDelayMs = 300;
let nativeDelayMs = 650;
await page.route(url, (route) => route.fulfill({ contentType: "text/html",
  body: `<html><head><meta charset="utf-8"><title>Recovery QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script>window.qaRecovery=${JSON.stringify(recovery)};window.qaRecoveryLoadError=${corruptRecovery};window.qaRecoveryDelayMs=${nativeFirst ? 650 : recoveryDelayMs};window.qaLastNativeDictation=${JSON.stringify(initialNativeDictation)};window.qaLastNativeDelayMs=${nativeFirst ? 0 : nativeDelayMs};</script><script type="module">${mock}</script></body></html>`,
}));

try {
  const waitForPreparation = async (nonce) => {
    await page.waitForFunction((expectedNonce) => window.qa.calls.some((call) =>
      call.cmd === "complete_update_preparation" && call.args.nonce === expectedNonce), nonce);
    return page.evaluate((expectedNonce) => window.qa.calls.findLast((call) =>
      call.cmd === "complete_update_preparation" && call.args.nonce === expectedNonce).args, nonce);
  };

  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).waitFor();
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).waitFor();

  // Q1: prepare and discard must serialize their durable mutations. The
  // blocked prepare read represents a deterministic load/save interleave; a
  // direct clear would otherwise be followed by a stale recovery save.
  await page.evaluate(() => {
    window.qa.blockRecovery("load");
    window.qa.prepareUpdate("qa-queued-discard");
  });
  await page.waitForFunction(() => window.qa.recoveryGateStarted("load"));
  await page.evaluate(() => window.qa.abortUpdate());
  await page.getByRole("button", { name: "Discard recovered drafts" }).click();
  await page.evaluate(() => window.qa.releaseRecovery("load"));
  const queuedDiscard = await waitForPreparation("qa-queued-discard");
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "clear_update_recovery"));
  assert.equal(queuedDiscard.error, null);
  assert.equal(await page.evaluate(() => window.qaRecovery), null,
    "discard must remain durable after a prepare read was already in flight");

  // Q1b: a late file delivery cleanup must reread after a held prepare save
  // and preserve the newer in-memory meeting edit captured by preparation.
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).click();
  await page.getByRole("heading", { name: "Acoustic activity" }).waitFor();
  await page.getByLabel(/Edit transcript segment at/).fill("New unsaved meeting snapshot");
  await page.evaluate(() => {
    window.qa.blockRecovery("save");
    window.qa.prepareUpdate("qa-queued-copy");
  });
  await page.waitForFunction(() => window.qa.recoveryGateStarted("save"));
  await page.evaluate(() => window.qa.abortUpdate());
  await page.getByRole("tab", { name: "recovered.wav completed" }).click();
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.evaluate(() => window.qa.releaseRecovery("save"));
  const queuedCopy = await waitForPreparation("qa-queued-copy");
  assert.equal(queuedCopy.error, null);
  await page.waitForFunction(() => window.qaRecovery?.files?.length === 0
    && window.qaRecovery?.meetings?.[0]?.editor_draft?.texts?.["seg-1"] === "New unsaved meeting snapshot");

  // Q2a: an edited native result remains in the same editor lineage across
  // abort/retry, so retry may replace the bytes persisted by the first prepare.
  const initialRecoveryDictation = recovery.dictation;
  recovery.dictation = null;
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native E1";
    window.qa.dictationResult("Native E1");
  });
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Native E1");
  await page.locator("textarea.test-result").fill("Native E1 edited");
  await page.evaluate(() => window.qa.prepareUpdate("qa-native-first"));
  assert.equal((await waitForPreparation("qa-native-first")).error, null);
  await page.evaluate(() => window.qa.abortUpdate());
  await page.locator("textarea.test-result").fill("Native E2 edited");
  await page.evaluate(() => window.qa.prepareUpdate("qa-native-retry"));
  assert.equal((await waitForPreparation("qa-native-retry")).error, null,
    "same-lineage edit must be retryable after an aborted update");
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Native E2 edited");

  // Q2b: the same lineage rule applies to a recovered editor. Delivery must
  // remove the earlier persisted bytes, then a later prepare must be empty.
  recovery.dictation = { text: "Recovered E1" };
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Recovered E1");
  await page.locator("textarea.test-result").fill("Recovered E1 edited");
  await page.evaluate(() => window.qa.prepareUpdate("qa-recovered-first"));
  assert.equal((await waitForPreparation("qa-recovered-first")).error, null);
  await page.evaluate(() => window.qa.abortUpdate());
  await page.locator("textarea.test-result").fill("Recovered E2 edited");
  const recoveredCopySaveCount = await page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "save_update_recovery").length);
  await page.getByRole("button", { name: "Copy result", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.waitForFunction((before) => window.qa.calls.filter((call) => call.cmd === "save_update_recovery").length > before
    && window.qaRecovery?.dictation === null, recoveredCopySaveCount);
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation), null,
    "delivery must remove only the earlier persisted draft in the same lineage");
  await page.evaluate(() => window.qa.prepareUpdate("qa-recovered-after-copy"));
  assert.equal((await waitForPreparation("qa-recovered-after-copy")).error, null);
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation), null);

  // Q2-no-op: delivering a distinct native replacement must not issue a
  // cleanup write when its captured lineage has no matching durable bytes.
  const initialRecoveryFiles = recovery.files;
  const initialRecoveryMeetings = recovery.meetings;
  recovery.files = [];
  recovery.meetings = [];
  recovery.dictation = { text: "Persisted D" };
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Replacement N";
    window.qa.dictationResult("Replacement N");
  });
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Replacement N");
  const noOpCallsBefore = await page.evaluate(() => window.qa.calls.length);
  await page.getByRole("button", { name: "Copy result", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  const noOpResult = await page.evaluate((start) => ({
    mutations: window.qa.calls.slice(start).filter((call) => ["save_update_recovery", "clear_update_recovery"].includes(call.cmd)),
    dictation: window.qaRecovery?.dictation?.text,
  }), noOpCallsBefore);
  assert.deepEqual(noOpResult.mutations, [], "unmatched delivery cleanup must not rewrite durable recovery");
  assert.equal(noOpResult.dictation, "Persisted D");
  recovery.files = initialRecoveryFiles;
  recovery.meetings = initialRecoveryMeetings;

  // Q2c: a distinct native replacement advances lineage and retains the old
  // persisted draft behind the conflict guard.
  recovery.dictation = null;
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Control D1";
    window.qa.dictationResult("Control D1");
  });
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Control D1");
  await page.locator("textarea.test-result").fill("Control D1 edited");
  await page.evaluate(() => window.qa.prepareUpdate("qa-control-first"));
  assert.equal((await waitForPreparation("qa-control-first")).error, null);
  await page.evaluate(() => window.qa.abortUpdate());
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Control D2";
    window.qa.dictationResult("Control D2");
  });
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Control D2");
  await page.evaluate(() => window.qa.prepareUpdate("qa-control-retry"));
  const controlRetry = await waitForPreparation("qa-control-retry");
  assert.match(controlRetry.error, /different unsaved dictation/i);
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Control D1 edited");

  // Q3a: a recovered delivery held across a newer native replacement must
  // remove only the captured recovered draft, then leave the replacement N
  // visible and retryable.
  recovery.files = [];
  recovery.meetings = [];
  recovery.dictation = { text: "Recovered D" };
  initialNativeDictation = "Earlier native result";
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Recovered D");
  await page.evaluate(() => window.qa.blockGate("copy-transcription"));
  await page.getByRole("button", { name: "Copy result", exact: true }).click();
  await page.waitForFunction(() => window.qa.gateStarted("copy-transcription"));
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native N";
    window.qa.dictationResult("Native N");
  });
  await page.evaluate(() => window.qa.releaseGate("copy-transcription"));
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qaRecovery === null);
  assert.equal(await page.locator("textarea.test-result").inputValue(), "Native N");
  assert.equal(await page.evaluate(() => window.qaNativePending), true);
  await page.evaluate(() => window.qa.prepareUpdate("qa-copy-native-next"));
  assert.equal((await waitForPreparation("qa-copy-native-next")).error, null);

  // Q3b: Save uses the same captured delivery context and actual save IPC.
  recovery.dictation = { text: "Recovered Save D" };
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Recovered Save D");
  await page.evaluate(() => window.qa.blockGate("save-transcription"));
  await page.getByRole("button", { name: "Save result…", exact: true }).click();
  await page.waitForFunction(() => window.qa.gateStarted("save-transcription"));
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native Save N";
    window.qa.dictationResult("Native Save N");
  });
  await page.evaluate(() => window.qa.releaseGate("save-transcription"));
  await page.getByText("Saved.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qaRecovery === null);
  assert.equal(await page.locator("textarea.test-result").inputValue(), "Native Save N");
  assert.equal(await page.evaluate(() => window.qaNativePending), true);
  await page.evaluate(() => window.qa.prepareUpdate("qa-save-native-next"));
  assert.equal((await waitForPreparation("qa-save-native-next")).error, null);

  // Q3c: cancelling Save never acknowledges or cleans the captured drafts.
  recovery.dictation = { text: "Cancelled D" };
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Cancelled D");
  await page.evaluate(() => {
    window.qa.setSaveResult(false);
    window.qa.blockGate("save-transcription");
  });
  await page.getByRole("button", { name: "Save result…", exact: true }).click();
  await page.waitForFunction(() => window.qa.gateStarted("save-transcription"));
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native Cancel N";
    window.qa.dictationResult("Native Cancel N");
  });
  await page.evaluate(() => window.qa.releaseGate("save-transcription"));
  await page.getByText("Save cancelled — nothing was written.", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Cancelled D");
  assert.equal(await page.evaluate(() => window.qaNativePending), true);
  assert.equal(await page.locator("textarea.test-result").inputValue(), "Native Cancel N");

  // Q3d: discard remains disabled for the entire active preparation, so no
  // clear can occur before the preparation acknowledgement.
  recovery.dictation = { text: "Active Prepare D" };
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.evaluate(() => {
    window.qa.blockRecovery("load");
    window.qa.prepareUpdate("qa-discard-disabled");
  });
  await page.waitForFunction(() => window.qa.recoveryGateStarted("load"));
  const discardButton = page.getByRole("button", { name: "Discard recovered drafts" });
  assert.equal(await discardButton.isDisabled(), true);
  assert.equal(await page.evaluate(() => window.qa.calls.some((call) => call.cmd === "clear_update_recovery")), false);
  await page.evaluate(() => window.qa.releaseRecovery("load"));
  assert.equal((await waitForPreparation("qa-discard-disabled")).error, null);
  assert.equal(await page.evaluate(() => window.qa.calls.some((call) => call.cmd === "clear_update_recovery")), false);
  await page.evaluate(() => window.qa.abortUpdate());

  // Q3e: clearing an owned same-lineage edit to whitespace may save null;
  // another persisted draft still remains protected by the conflict guard.
  recovery.dictation = { text: "Owned E" };
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Owned E");
  await page.locator("textarea.test-result").fill("Owned E edited");
  await page.evaluate(() => window.qa.prepareUpdate("qa-owned-first"));
  assert.equal((await waitForPreparation("qa-owned-first")).error, null);
  await page.evaluate(() => window.qa.abortUpdate());
  await page.locator("textarea.test-result").fill("   ");
  await page.evaluate(() => window.qa.prepareUpdate("qa-owned-empty"));
  assert.equal((await waitForPreparation("qa-owned-empty")).error, null);
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation), null);

  // Q3f: a held recovered Copy must not acknowledge a hidden native result
  // after Discard changes the live recovered flag while the action is away.
  recovery.dictation = { text: "Recovered Edited D" };
  initialNativeDictation = "Native L4 N";
  await page.goto(url);
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Recovered Edited D");
  await page.locator("textarea.test-result").fill("Recovered Edited D changed");
  await page.evaluate(() => window.qa.blockGate("copy-transcription"));
  await page.getByRole("button", { name: "Copy result", exact: true }).click();
  await page.waitForFunction(() => window.qa.gateStarted("copy-transcription"));
  await page.getByRole("button", { name: "Discard recovered drafts" }).click();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "clear_update_recovery"));
  const acknowledgeCallsBeforeL4 = await page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "acknowledge_update_result").length);
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Native L4 N";
    window.qa.releaseGate("copy-transcription");
  });
  await page.getByText("Copied result. Another unsaved dictation is now shown.", { exact: true }).waitFor();
  assert.equal(await page.evaluate(() => window.qaNativePending), true);
  assert.equal(await page.evaluate((before) => window.qa.calls.filter((call) => call.cmd === "acknowledge_update_result").length, acknowledgeCallsBeforeL4), acknowledgeCallsBeforeL4);

  // Q3g: Copy and Save started before preparation must queue their durable
  // delivery cleanup behind the held preparation acknowledgement. The
  // preparation snapshot remains durable until that acknowledgement releases,
  // then the queued delivery cleanup completes without dropping the action.
  const runHeldPreparationDelivery = async ({ action, actionName, nonce, gate }) => {
    recovery.dictation = { text: `Queued ${actionName} D` };
    recovery.files = [];
    recovery.meetings = [];
    initialNativeDictation = "Earlier native result";
    recoveryDelayMs = 0;
    nativeDelayMs = 0;
    await page.goto(url);
    await page.getByRole("button", { name: "Dictate", exact: true }).click();
    await page.waitForFunction((expected) => document.querySelector("textarea.test-result")?.value === expected, `Queued ${actionName} D`);
    await page.evaluate((deliveryGate) => {
      window.qa.blockGate("complete-preparation");
      window.qa.blockGate(deliveryGate);
    }, gate);
    await page.getByRole("button", { name: action, exact: true }).click();
    await page.waitForFunction((deliveryGate) => window.qa.gateStarted(deliveryGate), gate);
    await page.evaluate((expectedNonce) => window.qa.prepareUpdate(expectedNonce), nonce);
    await page.waitForFunction(() => window.qa.gateStarted("complete-preparation"));
    const heldLoadCount = await page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "load_update_recovery").length);
    const heldSaveCount = await page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "save_update_recovery").length);
    await page.evaluate((deliveryGate) => window.qa.releaseGate(deliveryGate), gate);
    await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "save_update_recovery"));
    await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 0)));
    const held = await page.evaluate(() => ({
      saves: window.qa.calls.filter((call) => call.cmd === "save_update_recovery").length,
      loads: window.qa.calls.filter((call) => call.cmd === "load_update_recovery").length,
      clears: window.qa.calls.filter((call) => call.cmd === "clear_update_recovery").length,
      recovery: window.qaRecovery,
    }));
    assert.equal(held.loads, heldLoadCount, `${actionName} cleanup must not load recovery before preparation acknowledgement`);
    assert.equal(held.saves, heldSaveCount, `${actionName} cleanup must not save recovery before preparation acknowledgement`);
    assert.equal(held.clears, 0, `${actionName} must not clear recovery before preparation acknowledgement`);
    assert.equal(held.recovery?.dictation?.text, `Queued ${actionName} D`);
    await page.evaluate(() => window.qa.releaseGate("complete-preparation"));
    assert.equal((await waitForPreparation(nonce)).error, null);
    await page.waitForFunction(() => window.qaRecovery === null);
    await page.getByText(actionName === "Copy" ? "Copied to clipboard." : "Saved.", { exact: true }).waitFor();
  };

  await runHeldPreparationDelivery({ action: "Copy result", actionName: "Copy", nonce: "qa-copy-held-preparation", gate: "copy-transcription" });
  await runHeldPreparationDelivery({ action: "Save result…", actionName: "Save", nonce: "qa-save-held-preparation", gate: "save-transcription" });
  recoveryDelayMs = 300;
  nativeDelayMs = 650;

  recovery.dictation = initialRecoveryDictation;
  recovery.files = initialRecoveryFiles;
  recovery.meetings = initialRecoveryMeetings;
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).waitFor();
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).waitFor();

  // D1: a newer native result must not make delivery of a recovered file
  // reconstruct the durable snapshot from potentially stale in-memory state.
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "New unsaved dictation";
    window.qa.dictationResult("New unsaved dictation");
  });
  await page.getByRole("tab", { name: "recovered.wav completed" }).click();
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "save_update_recovery"));
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Återställd diktering");

  // D1 also covers meetings independently: saving a recovered meeting after
  // a newer native result retains the recovered dictation branch.
  nativeFirst = false;
  const savedMeetingDraft = recovery.meetings[0].editor_draft;
  recovery.meetings[0].editor_draft = { labels: {}, mergeTargets: {}, texts: {}, speakers: {} };
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).waitFor();
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "New unsaved dictation";
    window.qa.dictationResult("New unsaved dictation");
  });
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).click();
  await page.waitForFunction(() => {
    const button = [...document.querySelectorAll("button")].find((candidate) => candidate.textContent?.trim() === "Save review");
    return button instanceof HTMLButtonElement && !button.disabled;
  });
  await page.getByRole("button", { name: "Save review", exact: true }).click();
  await page.getByText("Review saved.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "save_update_recovery"));
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Återställd diktering");
  recovery.meetings[0].editor_draft = savedMeetingDraft;

  // D2: cleanup must not synchronously serialize an unrelated in-memory edit.
  // The editor permits this value, while the recovery normalizer correctly
  // bounds it; delivery still removes only the copied file entry.
  nativeFirst = false;
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).waitFor();
  await page.getByRole("tab", { name: "recovered-meeting.wav completed" }).click();
  const oversizedSpeakerLabel = "x".repeat(500_001);
  const speakerInput = page.getByRole("textbox", { name: "Rename Speaker 1" });
  await speakerInput.fill(oversizedSpeakerLabel);
  await page.getByRole("tab", { name: "recovered.wav completed" }).click();
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "save_update_recovery"));
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Återställd diktering");

  // Reload a clean valid restore before the remaining preparation and
  // corruption cases; each corruption below must happen after hydration.
  nativeFirst = false;
  await page.goto(url);
  // If an existing native result wins the restore race, the persisted
  // dictation is intentionally skipped; delivering a restored file still
  // removes only that file and retains the skipped dictation.
  await page.waitForFunction(() => window.qa?.calls.some((call) => call.cmd === "load_update_recovery"));
  await page.waitForTimeout(50);
  await page.evaluate(() => {
    window.qaNativePending = true;
    window.qaLastNativeDictation = "Existing native result";
    window.qa.dictationResult("Existing native result");
  });
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.locator("textarea.test-result").waitFor();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "Existing native result");
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.getByRole("tab", { name: "recovered.wav completed" }).waitFor();
  await page.getByRole("tab", { name: "recovered.wav completed" }).click();
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await page.getByText("Copied to clipboard.", { exact: true }).waitFor();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "save_update_recovery"));
  assert.equal(await page.evaluate(() => window.qaRecovery.dictation.text), "Återställd diktering");

  nativeFirst = false;
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
  nativeFirst = false;
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
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
  nativeFirst = false;
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
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
  nativeFirst = false;
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

  nativeFirst = false;
  await page.goto(url);
  await page.getByRole("status", { name: "Recovered drafts" }).waitFor();
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
