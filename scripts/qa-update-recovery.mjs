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
const transcript = {
  schema_version: 1, source_sha256: "fixture-meeting-sha", language: "sv", model: "fixture",
  duration_seconds: 4,
  segments: [{ id: "seg-1", start: 0, end: 4, text: "Hej från mötet", speaker: "spk-1" }],
  speakers: [{ id: "spk-1", label: "Speaker 1" }],
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
  const speakerDraft = page.getByRole("textbox", { name: "Rename Speaker 1" });
  assert.equal(await speakerDraft.inputValue(), "Anna");
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
  corruptRecovery = true;
  await page.goto(url);
  await page.getByRole("alert").filter({ hasText: "Update blocked: unreadable recovery drafts were retained" }).waitFor();
  await page.evaluate(() => window.qa.prepareUpdate("qa-unreadable"));
  await page.waitForFunction(() => window.qa.calls.some(call => call.cmd === "complete_update_preparation" && call.args.nonce === "qa-unreadable"));
  const rejected = await page.evaluate(() => window.qa.calls.findLast(call => call.cmd === "complete_update_preparation").args);
  assert.match(rejected.error, /Synthetic recovery read failure/);
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
