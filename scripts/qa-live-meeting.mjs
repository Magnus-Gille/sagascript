// Browser QA for the isolated live-meeting panel. No native audio, Tauri
// commands, settings, or user data are involved.
import assert from "node:assert/strict";
import { mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");
const outputDir = process.env.QA_OUTPUT_DIR || tmpdir();
await mkdir(outputDir, { recursive: true });
const outputPath = (name) => join(outputDir, name);
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 760, height: 960 } });
const page = await context.newPage();
const errors = [];
page.on("pageerror", (error) => errors.push(error.message));
const url = process.env.QA_URL || "http://127.0.0.1:5242/?tab=live";

await page.route(url, (route) =>
  route.fulfill({
    contentType: "text/html",
    body: `<html><head><title>Live meeting QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module" src="/scripts/fixtures/live-meeting-browser-mock.js"></script></body></html>`,
  }),
);

async function writeFailureDiagnostics(error) {
  const message = error instanceof Error ? error.stack || error.message : String(error);
  const body = await page
    .locator("body")
    .innerText({ timeout: 5000 })
    .catch(() => "<body unavailable>");
  await Promise.allSettled([
    page.screenshot({ path: outputPath("live-meeting-failure.png"), fullPage: true, timeout: 5000 }),
    writeFile(outputPath("live-meeting-failure.body.txt"), body),
    writeFile(outputPath("live-meeting-failure.errors.txt"), [...errors, message].join("\n")),
  ]);
}

try {
  await page.goto(url);
  await page.getByRole("heading", { name: "Live meeting", exact: true }).waitFor();
  await page.getByText("Listening", { exact: true }).waitFor();
  await page.getByText("Preview från mig", { exact: true }).waitFor();
  await page.getByText("Finalt från mötet", { exact: true }).waitFor();
  await page.getByText("Jag · mikrofon", { exact: true }).first().waitFor();
  await page.getByText("Mötet · systemljud", { exact: true }).first().waitFor();
  assert.equal(await page.locator(".transcript-row").count(), 22, "latest page should contain the final 22 rows");
  assert.equal(await page.getByText("Page 3 of 3 · 422 segments", { exact: true }).count(), 1);
  await page.screenshot({ path: outputPath("live-meeting-running-760.png"), fullPage: true });

  await page.setViewportSize({ width: 500, height: 800 });
  await page.getByRole("button", { name: "Previous", exact: true }).click();
  await page.getByText("Page 2 of 3 · 422 segments", { exact: true }).waitFor();
  assert.equal(await page.locator(".transcript-row").count(), 200, "older pages must remain bounded to 200 rows");
  await page.getByText("Historik 210", { exact: true }).waitFor();
  await page.screenshot({ path: outputPath("live-meeting-history-500.png"), fullPage: true });
  await page.getByRole("button", { name: "Return to latest", exact: true }).click();
  await page.getByText("Page 3 of 3 · 422 segments", { exact: true }).waitFor();
  assert.equal(await page.locator(".transcript-row").count(), 22, "returning latest must keep the final page bounded");
  assert.equal(await page.getByRole("checkbox", { name: "Follow latest" }).isChecked(), true);
  await page.screenshot({ path: outputPath("live-meeting-latest-500.png"), fullPage: true });

  await page.getByRole("button", { name: "Stop live meeting", exact: true }).click();
  await page.getByText("Finishing transcript", { exact: true }).waitFor();
  await page.screenshot({ path: outputPath("live-meeting-draining-500.png"), fullPage: true });
  await page.evaluate(() => window.qa.setRefinement("running", 42));
  await page.getByRole("progressbar", { name: "Transcript refinement progress" }).waitFor();
  assert.equal(await page.getByRole("progressbar").getAttribute("value"), "42");
  await page.getByRole("button", { name: "Cancel refinement", exact: true }).waitFor();
  assert.equal(await page.getByRole("button", { name: "Discard", exact: true }).count(), 0);
  await page.getByText("Preview från mig", { exact: true }).waitFor();
  await page.screenshot({ path: outputPath("live-meeting-refining-500.png"), fullPage: true });
  await page.evaluate(() => window.qa.setRefinement("cancelled", 42, "Refinement cancelled; live text retained."));
  await page.getByText("Live transcript kept", { exact: true }).waitFor();
  await page.getByText("Preview från mig", { exact: true }).waitFor();
  await page.evaluate(() => {
    window.qa.setPhase("failed", "Synthetic partial stop failure");
  });

  await page.getByText("Live transcript kept", { exact: true }).waitFor();
  await page.getByText("Synthetic partial stop failure", { exact: true }).waitFor();

  const saveText = page.getByRole("button", { name: "Save text", exact: true });
  const saveAudio = page.getByRole("button", { name: "Save audio", exact: true });
  await saveText.click();
  await page.getByRole("alert").getByText("Synthetic text export failed; temporary text remains available.", { exact: true }).waitFor();
  assert.equal(await saveAudio.isEnabled(), true, "audio export remains available after text export failure");
  await saveText.click();
  await page.waitForFunction(() => window.qa.calls.filter((call) => call.action === "save-text").length === 2);
  await saveAudio.click();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.action === "save-audio"));
  await page.screenshot({ path: outputPath("live-meeting-failed-save-retry.png"), fullPage: true });

  await page.evaluate(() => window.qa.setRefinement("completed", 100));
  await page.getByText("Final transcript", { exact: true }).waitFor();
  await page.getByText("Efterbearbetad text", { exact: true }).waitFor();
  assert.equal(await page.getByText("Preview från mig", { exact: true }).count(), 0);
  await page.screenshot({ path: outputPath("live-meeting-refined-500.png"), fullPage: true });

  await page.getByRole("button", { name: "Discard", exact: true }).click();
  await page.getByRole("group", { name: "Confirm discard", exact: true }).waitFor();
  await page.getByRole("button", { name: "Keep", exact: true }).click();
  assert.equal(await page.getByRole("group", { name: "Confirm discard", exact: true }).count(), 0);
  await page.getByRole("button", { name: "Discard", exact: true }).click();
  await page.getByRole("button", { name: "Yes, discard", exact: true }).click();
  await page.getByRole("button", { name: "Start live meeting", exact: true }).waitFor();
  await page.screenshot({ path: outputPath("live-meeting-discarded-500.png"), fullPage: true });

  assert.deepEqual(errors, []);
  console.log(
    "PASS: isolated live meeting browser QA, running previews/finals, bounded history, partial stop, export retry, and discard confirmation.",
  );
} catch (error) {
  await writeFailureDiagnostics(error);
  throw error;
} finally {
  await context.close();
  await browser.close();
}
