// Regression for native completion preceding delivery to the Settings webview.
import assert from "node:assert/strict";
import { readFile, mkdir } from "node:fs/promises";
import { join } from "node:path";
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 760, height: 960 } });
const errors = [];
page.on("pageerror", error => errors.push(error.message));
const fixture = await readFile(new URL("./fixtures/transcription-browser-mock.js", import.meta.url), "utf8");
const mock = fixture.replace('  calls.push({ cmd, args });', '  calls.push({ cmd, args });\n  if (cmd === "create_meeting_review") await new Promise(resolve => { window.qaReleaseReview = resolve; });');
const url = process.env.QA_URL || "http://127.0.0.1:5243/?tab=transcribe";
await page.route(url, route => route.fulfill({ contentType: "text/html", body: `<html><head><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module">${mock}</script></body></html>` }));
try {
  await page.goto(url);
  await page.getByRole("button", { name: "Open Files...", exact: true }).waitFor();
  await page.getByRole("checkbox", { name: "Speaker diarization" }).check();
  await page.evaluate(() => window.qa.drop(["/fixtures/race-meeting.wav"]));
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "begin_meeting_file"));
  await page.getByRole("button", { name: "Cancel meeting", exact: true }).waitFor();
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "get_meeting_job"));
  await page.evaluate(() => {
    window.qa.holdPoll("/fixtures/race-meeting.wav");
    window.qa.finishMeeting("/fixtures/race-meeting.wav");
    window.qa.prepareUpdate("completion-race");
  });
  await page.waitForTimeout(100);
  const heldPoll = await page.evaluate(() => ({
    completion: window.qa.calls.findLast(c =>
      c.cmd === "complete_update_preparation" && c.args.nonce === "completion-race"),
    reviewCalls: window.qa.calls.filter(c => c.cmd === "create_meeting_review").length,
  }));
  assert.equal(heldPoll.completion, undefined,
    heldPoll.completion
      ? `must wait for the held terminal poll before acknowledging update (error: ${heldPoll.completion.args.error ?? "none"})`
      : "must wait for the held terminal poll before acknowledging update");
  assert.equal(heldPoll.reviewCalls, 0, "held terminal poll must precede review initialization");
  await page.evaluate(() => window.qa.releasePoll("/fixtures/race-meeting.wav"));
  await page.waitForFunction(() => typeof window.qaReleaseReview === "function");
  await page.waitForTimeout(100);
  const earlyCompletion = await page.evaluate(() => window.qa.calls.findLast(c =>
    c.cmd === "complete_update_preparation" && c.args.nonce === "completion-race"));
  assert.equal(earlyCompletion, undefined,
    earlyCompletion
      ? `must wait for the held review before acknowledging update (error: ${earlyCompletion.args.error ?? "none"})`
      : "must wait for the held review before acknowledging update");
  await page.evaluate(() => window.qaReleaseReview());
  await page.waitForFunction(() => window.qa.calls.some(c =>
    c.cmd === "complete_update_preparation" && c.args.nonce === "completion-race"));
  const calls = await page.evaluate(() => window.qa.calls);
  const completion = calls.findLast(c => c.cmd === "complete_update_preparation" && c.args.nonce === "completion-race");
  assert.equal(completion?.args.error, null, "update preparation must succeed after the held review is released");
  const completionIndex = calls.findLastIndex(c => c.cmd === "complete_update_preparation" && c.args.nonce === "completion-race");
  const beforeCompletion = calls.slice(0, completionIndex);
  const savedIndex = beforeCompletion.findLastIndex(c => c.cmd === "save_update_recovery"
    && c.args.payload?.meetings?.some(meeting => meeting.path === "/fixtures/race-meeting.wav"));
  assert.ok(savedIndex >= 0,
    "meeting recovery must be saved before successful update acknowledgement");
  const saved = beforeCompletion[savedIndex].args.payload;
  assert.equal(saved.meetings.length, 1);
  assert.equal(saved.meetings[0].review.transcript.source_sha256, "/fixtures/race-meeting.wav");
  await page.evaluate(() => window.qa.abortUpdate());
  // A completed auto-paste is useful in memory, but is not an unsaved draft.
  await page.evaluate(() => { window.qaNativePending = false; window.qa.dictationResult("already pasted"); });
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.waitForFunction(() => document.querySelector("textarea.test-result")?.value === "already pasted");
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.evaluate(() => window.qa.prepareUpdate("pasted-result"));
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation" && c.args.nonce === "pasted-result"));
  const pasted = await page.evaluate(() => window.qa.calls.findLast(c => c.cmd === "save_update_recovery").args.payload);
  assert.equal(pasted.dictation, null, "successfully pasted text must not be written as a draft");
  await page.evaluate(() => window.qa.abortUpdate());
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.locator("textarea.test-result").fill("edited after paste");
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.evaluate(() => window.qa.prepareUpdate("edited-result"));
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation" && c.args.nonce === "edited-result"));
  const edited = await page.evaluate(() => window.qa.calls.findLast(c => c.cmd === "save_update_recovery").args.payload);
  assert.equal(edited.dictation.text, "edited after paste", "a user-edited result is still an unsaved draft");
  await page.evaluate(() => { window.qa.abortUpdate(); window.qaRecovery = null; window.qaNativePending = true; window.qaLastNativeDictation = "native original"; window.qa.dictationResult("native original"); });
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.locator("textarea.test-result").fill("native result with correction");
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.evaluate(() => window.qa.prepareUpdate("edited-native"));
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation" && c.args.nonce === "edited-native"));
  const editedNative = await page.evaluate(() => window.qa.calls.findLast(c => c.cmd === "complete_update_preparation").args);
  assert.equal(editedNative.error, null, "editing the current native result must not be mistaken for a second draft");
  const corrected = await page.evaluate(() => window.qa.calls.findLast(c => c.cmd === "save_update_recovery").args.payload);
  assert.equal(corrected.dictation.text, "native result with correction");
  await page.evaluate(() => window.qa.abortUpdate());
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await page.getByRole("button", { name: "Copy result" }).click();
  await page.waitForFunction(() => window.qaRecovery?.dictation === null);
  assert.equal(await page.evaluate(() => window.qaNativePending), false);
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.evaluate(() => window.qa.prepareUpdate("after-delivery"));
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation" && c.args.nonce === "after-delivery"));
  assert.equal(await page.evaluate(() => window.qa.calls.findLast(c => c.cmd === "complete_update_preparation").args.error), null,
    "a failed update followed by Copy must be retryable");
  assert.deepEqual(errors, []);
  const output = process.env.QA_OUTPUT_DIR || "/private/tmp/sagascript-update-completion-qa";
  await mkdir(output, { recursive: true });
  await page.screenshot({ path: join(output, "completion-recovered.png"), fullPage: true });
  console.log("PASS: update waits for completed meeting delivery and persists its result.");
} finally {
  await browser.close();
}
