// F03: exercise Settings' recent-file select binding and serial re-run queue in
// Chromium. happy-dom's `option:checked` does not reflect its selectOptions()
// selection; Svelte reads that selector for bind:value, so this belongs here.
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

async function waitForNativeCalls(count) {
  await page.waitForFunction((expected) => window.qa.calls.filter((call) => call.cmd === "transcribe_file").length === expected, count);
}

async function waitForMeetingCalls(count) {
  await page.waitForFunction((expected) => window.qa.calls.filter((call) => call.cmd === "begin_meeting_file").length === expected, count);
}

async function waitForQueueCounts(queued, running) {
  await page.waitForFunction(
    ([queuedCount, runningCount]) => {
      const text = document.querySelector('[aria-label="File transcription status"]')?.textContent ?? "";
      return text.includes(`${queuedCount} queued`) && text.includes(`${runningCount} running`);
    },
    [queued, running],
  );
}

async function nativeCalls() {
  return page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "transcribe_file"));
}

async function meetingCalls() {
  return page.evaluate(() => window.qa.calls.filter((call) => call.cmd === "begin_meeting_file"));
}

async function diagnostics(error) {
  if (!page) return;
  const body = await page
    .locator("body")
    .innerText()
    .catch(() => "<body unavailable>");
  const result = {
    errors,
    failure: error instanceof Error ? error.stack || error.message : String(error),
    body,
    calls: await nativeCalls().catch(() => []),
    meetingCalls: await meetingCalls().catch(() => []),
    recentSelection: await page
      .getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" })
      .evaluate((select) => ({
        value: select.value,
        selectedOptions: Array.from(select.selectedOptions, (option) => option.value),
        checkedOptions: Array.from(select.querySelectorAll("option:checked"), (option) => option.value),
      }))
      .catch(() => null),
  };
  await Promise.allSettled([
    page.screenshot({ path: outputPath("rerun-selection-failure.png"), fullPage: true, timeout: 5000 }),
    writeFile(outputPath("rerun-selection-failure.json"), `${JSON.stringify(result, null, 2)}\n`),
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
    url = `http://127.0.0.1:${address.port}/?tab=transcribe`;
  }

  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 760, height: 960 } });
  page = await context.newPage();
  page.on("pageerror", (error) => errors.push(error.message));
  await page.route(url, (route) =>
    route.fulfill({
      contentType: "text/html",
      body: `<html><head><title>Re-run selection QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module">${fixture}</script></body></html>`,
    }),
  );

  await page.goto(url);
  await page.getByRole("button", { name: "Open Files...", exact: true }).waitFor();
  await page.getByRole("button", { name: "Transcribe", exact: true }).click();
  await page.getByRole("combobox", { name: "Profile (optional)" }).selectOption("english");
  await page.getByRole("textbox", { name: "Extra context for this file" }).fill("Names: Astrid");
  await page.evaluate(() => window.qa.drop(["/recordings/older.wav"]));
  await waitForNativeCalls(1);

  await page.evaluate(() => window.qa.drop(["/recordings/current.wav"]));
  await waitForQueueCounts(1, 1);
  const recent = page.getByRole("combobox", { name: "Recent files to re-run (last 5, this session)" });
  await recent.selectOption("/recordings/older.wav");
  const selection = await recent.evaluate((select) => ({
    value: select.value,
    selectedOptions: Array.from(select.selectedOptions, (option) => option.value),
    checkedOptions: Array.from(select.querySelectorAll("option:checked"), (option) => option.value),
  }));
  assert.deepEqual(selection, {
    value: "/recordings/older.wav",
    selectedOptions: ["/recordings/older.wav"],
    checkedOptions: ["/recordings/older.wav"],
  });
  await page.getByRole("button", { name: "Re-run", exact: true }).click();
  await waitForQueueCounts(2, 1);
  assert.equal((await nativeCalls()).length, 1, "Re-run must wait while the original request is still running");
  assert.equal(await page.evaluate(() => window.qa.maximum()), 1, "Only one native worker may be active");

  await page.evaluate(() => window.qa.finish("/recordings/older.wav"));
  await waitForNativeCalls(2);
  await waitForQueueCounts(1, 1);
  const afterFirstSettles = await nativeCalls();
  assert.equal(afterFirstSettles.length, 2, "Only the already queued current file may start after the first settles");
  assert.equal(afterFirstSettles[1].args.filePath, "/recordings/current.wav");
  assert.equal(await page.evaluate(() => window.qa.maximum()), 1, "The second native request occupies the single worker slot");

  await page.evaluate(() => window.qa.finish("/recordings/current.wav"));
  await waitForNativeCalls(3);
  const calls = await nativeCalls();
  assert.deepEqual(
    calls.map(({ args }) => ({
      filePath: args.filePath,
      prompt: args.prompt,
      profileId: args.profileId,
      diarize: args.diarize,
      autoPaste: args.autoPaste,
    })),
    [
      {
        filePath: "/recordings/older.wav",
        prompt: "Names: Astrid",
        profileId: "english",
        diarize: false,
        autoPaste: false,
      },
      {
        filePath: "/recordings/current.wav",
        prompt: "Names: Astrid",
        profileId: "english",
        diarize: false,
        autoPaste: false,
      },
      {
        filePath: "/recordings/older.wav",
        prompt: "Names: Astrid",
        profileId: "english",
        diarize: false,
        autoPaste: false,
      },
    ],
  );
  assert.equal(new Set(calls.map(({ args }) => args.runId)).size, 3, "Every native attempt must receive a fresh run id");

  // Distinguish the saved older request from both the current form and the
  // active request: finish an older English/Astrid run, change to Swedish/Bjorn,
  // hold that current file, then re-run the older selection behind it.
  await page.evaluate(() => window.qa.finish("/recordings/older.wav"));
  await waitForQueueCounts(0, 0);
  const profile = page.getByRole("combobox", { name: "Profile (optional)" });
  const prompt = page.getByRole("textbox", { name: "Extra context for this file" });
  await profile.selectOption("english");
  await prompt.fill("Names: Astrid");
  await page.evaluate(() => window.qa.drop(["/recordings/saved-request.wav"]));
  await waitForNativeCalls(4);
  await page.evaluate(() => window.qa.finish("/recordings/saved-request.wav"));
  await waitForQueueCounts(0, 0);

  await profile.selectOption("swedish");
  await prompt.fill("Names: Bjorn");
  await page.evaluate(() => window.qa.drop(["/recordings/current-options.wav"]));
  await waitForNativeCalls(5);
  await waitForQueueCounts(0, 1);
  await recent.selectOption("/recordings/saved-request.wav");
  const savedRequestSelection = await recent.evaluate((select) => ({
    value: select.value,
    selectedOptions: Array.from(select.selectedOptions, (option) => option.value),
    checkedOptions: Array.from(select.querySelectorAll("option:checked"), (option) => option.value),
  }));
  assert.deepEqual(savedRequestSelection, {
    value: "/recordings/saved-request.wav",
    selectedOptions: ["/recordings/saved-request.wav"],
    checkedOptions: ["/recordings/saved-request.wav"],
  });
  await page.getByRole("button", { name: "Re-run", exact: true }).click();
  await waitForQueueCounts(1, 1);
  assert.equal((await nativeCalls()).length, 5, "The selected saved request must wait behind the current Swedish job");
  await page.evaluate(() => window.qa.finish("/recordings/current-options.wav"));
  await waitForNativeCalls(6);
  const distinctCalls = await nativeCalls();
  assert.deepEqual(
    distinctCalls.slice(3).map(({ args }) => ({
      filePath: args.filePath,
      prompt: args.prompt,
      profileId: args.profileId,
      diarize: args.diarize,
      autoPaste: args.autoPaste,
    })),
    [
      {
        filePath: "/recordings/saved-request.wav",
        prompt: "Names: Astrid",
        profileId: "english",
        diarize: false,
        autoPaste: false,
      },
      {
        filePath: "/recordings/current-options.wav",
        prompt: "Names: Bjorn",
        profileId: "swedish",
        diarize: false,
        autoPaste: false,
      },
      {
        filePath: "/recordings/saved-request.wav",
        prompt: "Names: Astrid",
        profileId: "english",
        diarize: false,
        autoPaste: false,
      },
    ],
  );
  assert.equal(new Set(distinctCalls.map(({ args }) => args.runId)).size, 6, "Reruns preserve options and still get new run ids");
  await page.evaluate(() => window.qa.finish("/recordings/saved-request.wav"));
  await waitForQueueCounts(0, 0);

  // A true diarization choice routes through the meeting API. Complete an
  // older English/Astrid meeting, then hold a plain Swedish/Bjorn file and
  // verify the selected meeting re-run waits and uses its original payload.
  await profile.selectOption("english");
  await prompt.fill("Names: Astrid meeting");
  const diarization = page.getByRole("checkbox", { name: "Speaker diarization" });
  await diarization.check();
  await page.evaluate(() => window.qa.drop(["/meetings/saved-diarized.wav"]));
  await waitForMeetingCalls(1);
  await page.evaluate(() => window.qa.finishMeeting("/meetings/saved-diarized.wav", "completed"));
  await page.getByRole("tab", { name: "saved-diarized.wav completed", exact: true }).waitFor();

  await profile.selectOption("swedish");
  await prompt.fill("Names: Bjorn plain");
  await diarization.uncheck();
  await page.evaluate(() => window.qa.drop(["/meetings/current-plain.wav"]));
  await waitForNativeCalls(7);
  await waitForQueueCounts(0, 1);
  await recent.selectOption("/meetings/saved-diarized.wav");
  await page.getByRole("button", { name: "Re-run", exact: true }).click();
  await waitForQueueCounts(1, 1);
  assert.equal((await nativeCalls()).length, 7, "A saved diarized re-run must wait behind the current plain job");
  assert.equal((await meetingCalls()).length, 1, "The saved meeting re-run must not start early");

  await page.evaluate(() => window.qa.finish("/meetings/current-plain.wav"));
  await waitForMeetingCalls(2);
  assert.equal((await nativeCalls()).length, 7, "The saved diarized request must use begin_meeting_file, not transcribe_file");
  const diarizedCalls = await meetingCalls();
  assert.deepEqual(
    diarizedCalls.map(({ args }) => ({ filePath: args.filePath, prompt: args.prompt, profileId: args.profileId })),
    [
      { filePath: "/meetings/saved-diarized.wav", prompt: "Names: Astrid meeting", profileId: "english" },
      { filePath: "/meetings/saved-diarized.wav", prompt: "Names: Astrid meeting", profileId: "english" },
    ],
  );
  const allCalls = await page.evaluate(() =>
    window.qa.calls.filter((call) => ["transcribe_file", "begin_meeting_file"].includes(call.cmd)),
  );
  assert.equal(
    allCalls.filter((call) => call.cmd === "transcribe_file" && call.args.filePath === "/meetings/saved-diarized.wav").length,
    0,
  );
  assert.equal(await page.evaluate(() => window.qa.maximum()), 1);

  await page.evaluate(() => window.qa.finishMeeting("/meetings/saved-diarized.wav", "completed"));
  await waitForQueueCounts(0, 0);
  assert.deepEqual(errors, []);

  const result = {
    selection,
    savedRequestSelection,
    initialSchedulerCalls: calls,
    differentiatedRequests: distinctCalls.slice(3),
    diarizedMeetingRequests: diarizedCalls,
    maximumConcurrentNativeJobs: await page.evaluate(() => window.qa.maximum()),
    errors,
  };
  await writeFile(outputPath("rerun-selection-result.json"), `${JSON.stringify(result, null, 2)}\n`);
  await page.screenshot({ path: outputPath("rerun-selection.png"), fullPage: true });
  console.log(JSON.stringify(result, null, 2));
} catch (error) {
  await diagnostics(error);
  throw error;
} finally {
  await browser?.close();
  await server?.close();
}
