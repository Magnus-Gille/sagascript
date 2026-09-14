// Run against an isolated Vite server. No native audio, models, or user data.
// PLAYWRIGHT_MODULE=/path/to/playwright/index.mjs node scripts/qa-transcription-tabs.mjs
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 760, height: 960 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", error => errors.push(error.message));
const mock = await readFile(new URL("./fixtures/transcription-browser-mock.js", import.meta.url), "utf8");
const url = process.env.QA_URL || "http://127.0.0.1:5242/?tab=transcribe";
await page.route(url, route => route.fulfill({ contentType: "text/html",
  body: `<html><head><title>Transcription QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module">${mock}</script></body></html>` }));
const status = (name, state) => page.getByRole("tab", { name: `${name} ${state}`, exact: true });
async function waitStatus(name, state) { await status(name, state).waitFor(); }
async function drop(paths) { await page.evaluate(paths => window.qa.drop(paths), paths); }
async function finish(path, error = null) { await page.evaluate(([path, error]) => window.qa.finish(path, error), [path, error]); }
async function meeting(path, state = "completed") { await page.evaluate(([path, state]) => window.qa.finishMeeting(path, state), [path, state]); }
try {
  await page.goto(url);
  await page.getByRole("button", { name: "Open Files...", exact: true }).waitFor();
  const optionsBox = await page.locator('.transcribe-options').boundingBox();
  const dropBox = await page.locator('.drop-zone').boundingBox();
  assert.ok(optionsBox.y + optionsBox.height + 7 <= dropBox.y, 'settings must stay above the drop zone with spacing');
  assert.equal(await page.getByText('No profile keeps the selected language and global hint context.', { exact: true }).count(), 0);
  await drop(["/fixtures/one.wav", "/fixtures/two.wav", "/fixtures/three.wav"]);
  await waitStatus("one.wav", "running");
  await waitStatus("two.wav", "queued");
  await waitStatus("three.wav", "queued");
  await finish("/fixtures/one.wav");
  await waitStatus("one.wav", "completed");
  await waitStatus("two.wav", "running");
  // A second drop while busy must append, never disappear behind a busy guard.
  await drop(["/fixtures/four.wav"]);
  await waitStatus("four.wav", "queued");
  assert.equal(await status("one.wav", "completed").getAttribute("aria-selected"), "true", "appending while busy must keep the viewed result selected");
  await finish("/fixtures/two.wav", "Synthetic decode failure");
  await waitStatus("two.wav", "failed");
  await waitStatus("three.wav", "running");
  await finish("/fixtures/three.wav");
  await waitStatus("four.wav", "running");
  await finish("/fixtures/four.wav");
  await waitStatus("four.wav", "completed");
  for (const name of ["one", "three", "four"]) {
    await status(`${name}.wav`, "completed").click();
    assert.equal(await page.locator('[role="tabpanel"]:visible textarea.transcribe-result').inputValue(), `Transcript for /fixtures/${name}.wav`);
  }
  await status("two.wav", "failed").click();
  await page.getByText("Synthetic decode failure", { exact: true }).waitFor();
  await status("three.wav", "completed").click();
  assert.equal(await status("three.wav", "completed").getAttribute("aria-selected"), "true");
  await page.waitForFunction(() => getComputedStyle(document.querySelector(".result-tab.active")).borderColor === "rgb(114, 230, 207)");
  await page.screenshot({ path: "dist/sagascript-242-results.png", fullPage: true });
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  assert.equal(await page.locator('[role="tabpanel"]:visible textarea.transcribe-result').inputValue(), "Transcript for /fixtures/three.wav");
  // Keyboard navigation follows the tab pattern.
  await status("three.wav", "completed").focus();
  await page.keyboard.press("Home");
  assert.equal(await status("one.wav", "completed").getAttribute("aria-selected"), "true");
  // File picker shares the queue, including the single-file case.
  await page.getByRole("button", { name: "Open Files...", exact: true }).click();
  await waitStatus("picked.wav", "running");
  await finish("/fixtures/picked.wav");
  await waitStatus("picked.wav", "completed");
  // Diarized jobs wait for terminal snapshots, and a temporary poll error holds the queue.
  await page.getByRole("checkbox", { name: "Speaker diarization" }).check();
  await drop(["/fixtures/meeting-one.wav", "/fixtures/meeting-two.wav", "/fixtures/meeting-three.wav"]);
  await waitStatus("meeting-one.wav", "running");
  await page.evaluate(() => window.qa.failPoll("/fixtures/meeting-one.wav"));
  await page.getByRole("button", { name: "Retry status check", exact: true }).waitFor();
  await waitStatus("meeting-two.wav", "queued");
  await meeting("/fixtures/meeting-one.wav");
  await page.getByRole("button", { name: "Retry status check", exact: true }).click();
  await waitStatus("meeting-one.wav", "completed");
  await waitStatus("meeting-two.wav", "running");
  const completedSpeaker = page.getByRole("textbox", { name: "Rename Speaker 1", exact: true });
  assert.equal(await completedSpeaker.isEnabled(), true, "completed review remains editable while next file transcribes");
  await completedSpeaker.fill("Working Alice");
  await meeting("/fixtures/meeting-two.wav");
  await waitStatus("meeting-three.wav", "running");
  await meeting("/fixtures/meeting-three.wav");
  await waitStatus("meeting-three.wav", "completed");
  // Cancellation must release the queue only after its terminal status is read.
  await drop(["/fixtures/meeting-cancel.wav", "/fixtures/meeting-after.wav"]);
  await waitStatus("meeting-cancel.wav", "running");
  await page.getByRole("button", { name: "Cancel meeting", exact: true }).click();
  await waitStatus("meeting-cancel.wav", "cancelled");
  await waitStatus("meeting-after.wav", "running");
  await meeting("/fixtures/meeting-after.wav");
  await waitStatus("meeting-after.wav", "completed");
  // Independent mounted drafts survive both result and top-level tab switches.
  await status("meeting-one.wav", "completed").click();
  const nameInput = page.getByRole("textbox", { name: "Rename Speaker 1", exact: true });
  await nameInput.fill("Unsaved Alice");
  await status("meeting-two.wav", "completed").click();
  assert.equal(await nameInput.inputValue(), "Speaker 1");
  await nameInput.fill("Unsaved Bob");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  assert.equal(await nameInput.inputValue(), "Unsaved Bob");
  await status("meeting-one.wav", "completed").click();
  assert.equal(await nameInput.inputValue(), "Unsaved Alice");
  assert.deepEqual(await page.evaluate(() => {
    const ids = [...document.querySelectorAll("[id]")].map(node => node.id);
    return ids.filter((id, index) => ids.indexOf(id) !== index);
  }), []);
  assert.equal(await page.evaluate(() => window.qa.maximum()), 1, "transcriptions must never overlap");
  const paths = await page.evaluate(() => window.qa.calls.filter(call => ["transcribe_file", "begin_meeting_file"].includes(call.cmd)).map(call => call.args.filePath));
  assert.deepEqual(paths, ["/fixtures/one.wav", "/fixtures/two.wav", "/fixtures/three.wav", "/fixtures/four.wav", "/fixtures/picked.wav", "/fixtures/meeting-one.wav", "/fixtures/meeting-two.wav", "/fixtures/meeting-three.wav", "/fixtures/meeting-cancel.wav", "/fixtures/meeting-after.wav"]);
  assert.equal(await page.evaluate(() => window.qa.calls.filter(call => call.cmd === 'transcribe_file').every(call => call.args.autoPaste === false)), true);
  // Polish must remain per-file after extraction: a recent selection cannot
  // change Save's source, and re-run queues alongside the retained review.
  await status("one.wav", "completed").click();
  await page.getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" }).selectOption("/fixtures/meeting-two.wav");
  await page.getByRole("button", { name: "Copy", exact: true }).click();
  await page.getByRole("button", { name: "Save…", exact: true }).click();
  const saved = await page.evaluate(() => window.qa.calls.filter(call => call.cmd === "save_transcription_text").at(-1));
  assert.deepEqual(saved.args, { text: "Transcript for /fixtures/one.wav", fileName: "one.txt", directory: "/fixtures" });
  await page.getByRole("checkbox", { name: "Speaker diarization" }).uncheck();
  await drop(["/fixtures/cancel-plain.wav", "/fixtures/after-plain.wav"]);
  await waitStatus("cancel-plain.wav", "running");
  const runId = await page.evaluate(() => window.qa.calls.filter(call => call.cmd === "transcribe_file").at(-1).args.runId);
  assert.ok(runId);
  await page.evaluate(() => window.qa.progress("wrong-run", "transcribing", 90));
  await page.evaluate(runId => window.qa.progress(runId, "resampling", 60), runId);
  await page.getByText("Converting to 16 kHz mono", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Stop transcription", exact: true }).click();
  await waitStatus("after-plain.wav", "queued");
  await finish("/fixtures/cancel-plain.wav");
  await waitStatus("cancel-plain.wav", "completed");
  await page.getByText("Finished before Stop took effect.", { exact: true }).waitFor();
  await waitStatus("after-plain.wav", "running");
  await page.getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" }).selectOption("/fixtures/cancel-plain.wav");
  await page.getByRole("button", { name: "Re-run", exact: true }).click();
  await waitStatus("cancel-plain.wav", "queued");
  await finish("/fixtures/after-plain.wav");
  await waitStatus("cancel-plain.wav", "running");
  await finish("/fixtures/cancel-plain.wav");
  await page.waitForFunction(() => [...document.querySelectorAll('[role="tab"]')].filter(tab => tab.textContent.includes("cancel-plain.wav") && tab.textContent.includes("completed")).length === 2);
  assert.equal(await page.evaluate(() => window.qa.maximum()), 1);
  await page.getByRole("button", { name: "What is speaker diarization?", exact: true }).click();
  await page.getByRole("dialog", { name: "What is speaker diarization?", exact: true }).waitFor();
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "What is speaker diarization?");
  assert.deepEqual(errors, []);
  await status("meeting-one.wav", "completed").click();
  await nameInput.scrollIntoViewIfNeeded();
  await page.screenshot({ path: "dist/sagascript-242-meetings.png", fullPage: true });
  console.log("PASS: real Svelte UI queue, retained results, failure continuation, append, keyboard tabs, picker, serialized meetings, poll retry, cancellation, independent drafts, unique IDs; no browser errors.");
} finally {
  await context.close();
  await browser.close();
}
