// SAGA-564: the Record window in Chromium against the mocked IPC (fixture data,
// no microphone, no installed app): start/stop, the recording indicator, the
// notices, the failed-session error box, the recent list and the recovery note.
// Screenshots of each state go to QA_OUTPUT_DIR (default: the temp dir).
import assert from "node:assert/strict";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const outputDir = process.env.QA_OUTPUT_DIR || tmpdir();
await mkdir(outputDir, { recursive: true });
const outputPath = (name) => join(outputDir, name);
const fixture = await readFile(new URL("./fixtures/transcription-browser-mock.js", import.meta.url), "utf8");
let server;
let browser;
let page;
const errors = [];

// The app styles one (dark) theme and ignores prefers-color-scheme, so one
// screenshot per state is the whole picture.
async function shoot(name) {
  await page.screenshot({ path: outputPath(`record-${name}.png`), fullPage: true });
}

async function diagnostics(error) {
  if (!page) return;
  const body = await page
    .locator("body")
    .innerText()
    .catch(() => "<body unavailable>");
  await Promise.allSettled([
    page.screenshot({ path: outputPath("record-window-failure.png"), fullPage: true, timeout: 5000 }),
    writeFile(
      outputPath("record-window-failure.json"),
      `${JSON.stringify({ errors, failure: error instanceof Error ? error.stack || error.message : String(error), body }, null, 2)}\n`,
    ),
  ]);
}

try {
  let url = process.env.QA_URL;
  if (!url) {
    const { createServer } = await import("vite");
    server = await createServer({ server: { host: "127.0.0.1", port: 0, strictPort: false } });
    await server.listen();
    const address = server.httpServer?.address();
    if (!address || typeof address === "string") throw new Error("Vite did not expose a TCP port.");
    url = `http://127.0.0.1:${address.port}/`;
  }
  const recordUrl = new URL(url);
  recordUrl.search = "?window=record";

  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 560, height: 900 } });
  page = await context.newPage();
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route(recordUrl.href, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<html><head><title>Record window QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module">${fixture}</script></body></html>`,
    }),
  );

  // Idle: the heading, an enabled start button and the recent recordings.
  await page.goto(recordUrl.href);
  await page.getByRole("heading", { name: "Record", exact: true }).waitFor();
  const start = page.getByRole("button", { name: "Start recording", exact: true });
  await start.waitFor();
  assert.equal(await start.isEnabled(), true, "a returning user can start at once");
  assert.equal(await page.getByRole("status", { name: "Recording state" }).count(), 0, "no indicator while idle");
  await page.getByText("Lecture", { exact: true }).first().waitFor();
  await page.getByText("Interview", { exact: true }).first().waitFor();
  await shoot("idle");

  // Start: the indicator and the stop button; the quality cannot change meanwhile.
  await start.click();
  const indicator = page.getByRole("status", { name: "Recording state" });
  await indicator.waitFor();
  await page.getByRole("button", { name: "Stop recording", exact: true }).waitFor();
  assert.match((await indicator.innerText()).trim(), /recording/i);
  assert.equal(await page.getByRole("radio", { name: /Compact/ }).isDisabled(), true, "quality is locked while recording");
  await page.evaluate(() =>
    window.qa.recordingSessionChanged({
      state: "recording",
      recording_id: "7a2d9e64-1b3c-4f58-8e07-c4d2a91b6f30",
      folder_name: "2026-11-06 10.00 Recording",
      title: "Recording",
      quality: "lossless",
      elapsed_ms: 3_725_000,
      level: 0.35,
      bytes_written: 1_100_000_000,
    }),
  );
  await page.getByRole("timer", { name: "Elapsed time" }).filter({ hasText: "1:02:05" }).waitFor();
  assert.ok(Number(await page.getByRole("meter", { name: "Input level" }).getAttribute("aria-valuenow")) > 0, "the meter moves");
  await shoot("recording");

  // A gap and a stalled device show as notices while recording continues.
  await page.evaluate(() =>
    window.qa.recordingSessionChanged({
      state: "recording",
      recording_id: "7a2d9e64-1b3c-4f58-8e07-c4d2a91b6f30",
      folder_name: "2026-11-06 10.00 Recording",
      title: "Recording",
      quality: "lossless",
      elapsed_ms: 3_730_000,
      level: 1,
      clipped: true,
      gap_count: 1,
      gap_ms: 400,
      stalled: true,
    }),
  );
  await page.getByRole("list", { name: "Recording notices" }).waitFor();
  await shoot("notices");

  // Stopping from the window finishes the recording and adds it to the list.
  await page.getByRole("button", { name: "Stop recording", exact: true }).click();
  await page.getByRole("region", { name: "Finished recording" }).waitFor();
  await page.getByRole("button", { name: "Start recording", exact: true }).waitFor();
  await shoot("finished");

  // A session that fails by itself (the disk filled) shows the error box and no stuck "stopping".
  await page.getByRole("button", { name: "Start recording", exact: true }).click();
  await page.getByRole("button", { name: "Stop recording", exact: true }).waitFor();
  await page.evaluate(() => window.qa.recordingSessionChanged({ state: "failed", error_code: "recording_disk_full" }));
  const alert = page.getByRole("alert").first();
  await alert.waitFor();
  assert.match(await alert.innerText(), /disk is full/i);
  assert.equal(await page.getByRole("status", { name: "Recording state" }).count(), 0, "no indicator after a failure");
  await shoot("error");
  await alert.getByRole("button", { name: "Dismiss" }).click();
  await page.getByRole("button", { name: "Start recording", exact: true }).waitFor();

  // A session that finishes by itself (finalizing, then done) returns to idle.
  await page.getByRole("button", { name: "Start recording", exact: true }).click();
  await page.getByRole("button", { name: "Stop recording", exact: true }).waitFor();
  await page.evaluate(() => window.qa.recordingSessionChanged({ state: "finalizing", elapsed_ms: 5000 }));
  await page
    .getByRole("status", { name: "Recording state" })
    .filter({ hasText: /saving the recording/i })
    .waitFor();
  await page.evaluate(() => window.qa.recordingSessionChanged({ state: "done" }));
  await page.getByRole("button", { name: "Start recording", exact: true }).waitFor();
  assert.equal(await page.getByRole("status", { name: "Recording state" }).count(), 0, "not stuck in stopping");

  // Recovered recordings are described in their own box.
  await page.evaluate(() =>
    window.qa.recordingsRecovered([
      {
        folder_name: "2026-11-04 09.00 Interview",
        title: "Interview",
        outcome: "recovered",
        frames_kept: 29_472_000,
        tail_bytes: 0,
        message: "Recovered 10:14 of audio.",
      },
    ]),
  );
  await page.getByRole("region", { name: "Recovered recordings" }).waitFor();
  await shoot("recovered");

  assert.deepEqual(errors, [], "no page errors");
  console.log("PASS: Record window start/stop, indicator, notices, failed and self-finished sessions, recovery; no page errors.");
} catch (error) {
  await diagnostics(error);
  throw error;
} finally {
  await browser?.close();
  await server?.close();
}
