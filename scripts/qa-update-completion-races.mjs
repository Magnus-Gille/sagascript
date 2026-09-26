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
  await page.evaluate(() => { window.qa.finishMeeting("/fixtures/race-meeting.wav"); window.qa.prepareUpdate("completion-race"); });
  await page.waitForFunction(() => typeof window.qaReleaseReview === "function");
  await page.waitForTimeout(100);
  assert.equal(await page.evaluate(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation")), false, "must wait for the terminal result");
  await page.evaluate(() => window.qaReleaseReview());
  await page.waitForFunction(() => window.qa.calls.some(c => c.cmd === "complete_update_preparation"));
  const calls = await page.evaluate(() => window.qa.calls);
  const saved = calls.findLast(c => c.cmd === "save_update_recovery").args.payload;
  assert.equal(saved.meetings.length, 1);
  assert.equal(saved.meetings[0].review.transcript.source_sha256, "/fixtures/race-meeting.wav");
  assert.equal(calls.findLast(c => c.cmd === "complete_update_preparation").args.error, null);
  assert.deepEqual(errors, []);
  const output = process.env.QA_OUTPUT_DIR || "/private/tmp/sagascript-update-completion-qa";
  await mkdir(output, { recursive: true });
  await page.screenshot({ path: join(output, "completion-recovered.png"), fullPage: true });
  console.log("PASS: update waits for completed meeting delivery and persists its result.");
} finally {
  await browser.close();
}
