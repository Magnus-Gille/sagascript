import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";

const workflow = await readFile(
  new URL("../.github/workflows/windows-package.yml", import.meta.url),
  "utf8",
);
const releaseGuide = await readFile(
  new URL("../docs/windows-release.md", import.meta.url),
  "utf8",
);

test("Windows candidate workflow stays non-publishing and explicitly unsigned", () => {
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /permissions:\s*\n\s*contents: read/);
  assert.match(workflow, /tauri build --ci --bundles nsis,msi --no-sign/);
  assert.match(workflow, /SignaturePolicy Internal/);
  assert.match(workflow, /windows-x64-unsigned-candidate/);
  assert.match(workflow, /accept-windows-candidate\.ps1/);
  assert.match(workflow, /norwegian-short-3s\.mp3/);
  assert.doesNotMatch(workflow, /action-gh-release|gh release|contents: write/);
});

test("Windows candidate makes real transcription a blocking gate", () => {
  const gateStart = workflow.indexOf("name: Gate real Windows transcription");
  const buildStart = workflow.indexOf("name: Build unsigned internal installers");
  assert.ok(gateStart >= 0 && buildStart > gateStart);
  const gate = workflow.slice(gateStart, buildStart);
  assert.match(gate, /download-model nb-whisper-tiny/);
  assert.match(gate, /verify-json-cli-streams\.py/);
  assert.doesNotMatch(gate, /continue-on-error/);
});

test("Windows release guide forbids publishing unsigned candidates", () => {
  assert.match(releaseGuide, /Do not publish those artifacts/);
  assert.match(releaseGuide, /Microsoft Store MSIX/);
  assert.match(releaseGuide, /Release[^\n]*requires every executable artifact/i);
});

test("third-party notice comparison accepts Windows checkout line endings", () => {
  const output = execFileSync(
    process.execPath,
    [
      fileURLToPath(new URL("./generate-third-party-notices.mjs", import.meta.url)),
      "--test-newline-normalization",
    ],
    { encoding: "utf8" },
  );
  assert.match(output, /newline normalization passed/);
});
