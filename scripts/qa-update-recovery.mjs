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
await page.route(url, (route) => route.fulfill({ contentType: "text/html",
  body: `<html><head><meta charset="utf-8"><title>Recovery QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script>window.qaRecovery=${JSON.stringify(recovery)};window.qaRecoveryDelayMs=300;</script><script type="module">${mock}</script></body></html>`,
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
