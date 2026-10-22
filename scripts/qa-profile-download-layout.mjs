// Rendered Chromium check for the fixed profile-engine action and its neighbor.
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

async function measure() {
  return page.evaluate(() => {
    const button = document.querySelector(".profile-engine-action");
    const row = button?.closest(".profile-engine");
    const neighbor = row?.querySelector(":scope > span");
    if (!button || !row || !neighbor) throw new Error("Missing profile engine action or adjacent model label");
    const rect = (element) => {
      const { x, y, width, height } = element.getBoundingClientRect();
      return Object.fromEntries(Object.entries({ x, y, width, height }).map(([key, value]) => [key, Number(value.toFixed(2))]));
    };
    const textRange = document.createRange();
    textRange.selectNodeContents(button);
    const style = getComputedStyle(button);
    return {
      label: button.textContent.trim(),
      button: rect(button),
      row: rect(row),
      neighbor: rect(neighbor),
      computed: {
        whiteSpace: style.whiteSpace,
        overflowX: style.overflowX,
        textOverflow: style.textOverflow,
      },
      content: {
        clientWidth: button.clientWidth,
        scrollWidth: button.scrollWidth,
        clientHeight: button.clientHeight,
        scrollHeight: button.scrollHeight,
        textLineCount: textRange.getClientRects().length,
      },
    };
  });
}

function assertStableLayout(baseline, current) {
  for (const region of ["button", "row", "neighbor"]) {
    for (const dimension of ["x", "y", "width", "height"]) {
      assert.ok(
        Math.abs(current[region][dimension] - baseline[region][dimension]) <= 0.5,
        `${region}.${dimension} moved: ${baseline[region][dimension]} -> ${current[region][dimension]} (${current.label})`,
      );
    }
  }
  assert.equal(current.computed.whiteSpace, "nowrap", `${current.label} must stay on one line`);
  assert.equal(current.computed.overflowX, "hidden", `${current.label} must stay inside the action bounds`);
  assert.equal(current.content.textLineCount, 1, `${current.label} wrapped onto multiple lines`);
  assert.ok(current.content.scrollWidth <= current.content.clientWidth, `${current.label} is clipped horizontally`);
  assert.ok(current.content.scrollHeight <= current.content.clientHeight, `${current.label} is clipped vertically`);
}

try {
  let url = process.env.QA_URL;
  if (!url) {
    const { createServer } = await import("vite");
    server = await createServer({ server: { host: "127.0.0.1", port: 0, strictPort: false } });
    await server.listen();
    const address = server.httpServer?.address();
    if (!address || typeof address === "string") throw new Error("Vite did not expose a TCP port.");
    url = `http://127.0.0.1:${address.port}/?tab=dictate`;
  } else {
    const parsed = new URL(url);
    parsed.pathname = "/";
    parsed.search = "?tab=dictate";
    url = parsed.toString();
  }

  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 900, height: 960 } });
  page = await context.newPage();
  page.on("pageerror", (error) => errors.push(error.message));
  const navigation = new URL(url);
  const routePrefix = `${navigation.origin}${navigation.pathname}`.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  let fixtureServed = false;
  await page.route(new RegExp(`^${routePrefix}\\?tab=dictate$`), async (route) => {
    fixtureServed = true;
    await route.fulfill({
      contentType: "text/html",
      body: `<html><head><title>Profile download layout QA</title><link rel="stylesheet" href="/src/app.css"></head><body><div id="app"></div><script type="module">${fixture}</script></body></html>`,
    });
  });

  await page.goto(url);
  assert.ok(fixtureServed, `QA fixture was not served for ${url}`);
  const downloadButton = page.getByRole("button", { name: "Download speech engine", exact: true });
  await downloadButton.waitFor({ state: "visible" });
  await page.evaluate(() => {
    // Keep the fixture's model-ready event from replacing the missing state
    // while its existing held download promise is pending.
    const originalInvoke = window.__TAURI_INTERNALS__.invoke;
    window.__TAURI_INTERNALS__.invoke = (command, args, options) => {
      if (window.qaHoldModelReady && command === "plugin:event|emit" && args.event === "model-ready") {
        return Promise.resolve(null);
      }
      return originalInvoke(command, args, options);
    };
  });
  const baseline = await measure();
  assert.equal(baseline.button.width, 152, "The missing-engine action retains its 152px contract");
  await page.evaluate(() => {
    window.qa.holdNextPianissimoDownload();
    window.qaHoldModelReady = true;
  });
  await downloadButton.click();
  await page.waitForFunction(() => window.qa.calls.some((call) => call.cmd === "download_pianissimo_model"));

  const snapshots = [baseline];
  for (const progress of [1, 99]) {
    await page.evaluate(async (value) => {
      const { emit } = await import("/node_modules/@tauri-apps/api/event.js");
      await emit("model-download-progress", { progress: value });
    }, progress);
    const progressButton = page.getByRole("button", { name: `Downloading ${progress}%`, exact: true });
    await progressButton.waitFor({ state: "visible" });
    const current = await measure();
    assertStableLayout(baseline, current);
    snapshots.push(current);
  }

  const result = { snapshots, pageErrors: errors };
  assert.deepEqual(errors, [], "Rendered page should not emit errors");
  await page.screenshot({ path: outputPath("profile-download-layout.png"), fullPage: true });
  await writeFile(outputPath("profile-download-layout-result.json"), `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify(result, null, 2));
  await page.evaluate(() => {
    window.qaHoldModelReady = false;
    window.qa.releasePianissimo();
  });
} catch (error) {
  if (page) {
    const body = await page
      .locator("body")
      .innerText()
      .catch(() => "<body unavailable>");
    await Promise.allSettled([
      page.screenshot({ path: outputPath("profile-download-layout-failure.png"), fullPage: true, timeout: 5000 }),
      writeFile(
        outputPath("profile-download-layout-failure.json"),
        `${JSON.stringify({ error: error instanceof Error ? error.stack : String(error), body, errors }, null, 2)}\n`,
      ),
    ]);
  }
  throw error;
} finally {
  await browser?.close();
  await server?.close();
}
