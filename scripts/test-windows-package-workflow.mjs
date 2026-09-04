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
  assert.match(workflow, /runs-on: \$\{\{ matrix\.runner \}\}/);
  assert.match(workflow, /architecture: x64[\s\S]*runner: windows-latest[\s\S]*rust_target: x86_64-pc-windows-msvc/);
  assert.match(workflow, /architecture: arm64[\s\S]*runner: windows-11-arm[\s\S]*rust_target: aarch64-pc-windows-msvc/);
  assert.match(workflow, /windows-\$\{\{ matrix\.architecture \}\}-unsigned-candidate/);
  assert.match(workflow, /RuntimeInformation\]::OSArchitecture/);
  assert.match(workflow, /rustc -vV/);
  assert.match(workflow, /accept-windows-candidate\.ps1/);
  assert.match(workflow, /norwegian-short-3s\.mp3/);
  assert.match(workflow, /Sagascript-Windows-\$architecture-Portable\.exe/);
  assert.match(workflow, /Sagascript-Windows-\$architecture-CLI\.exe/);
  assert.match(workflow, /Sagascript-Windows-\$architecture-Setup\.exe/);
  assert.match(workflow, /Sagascript-Windows-\$architecture\.msi/);
  assert.match(workflow, /-CliExe "artifacts\\Sagascript-Windows-\$architecture-CLI\.exe"/);
  assert.match(workflow, /ChecksumOutput "artifacts\\SHA256SUMS-Windows-\$architecture"/);
  assert.match(workflow, /targetRoot = "src-tauri\\target\\\$\{\{ matrix\.rust_target \}\}\\release"/);
  assert.match(workflow, /name: Remove cached installer bundles/);
  assert.match(workflow, /Remove-Item -LiteralPath \$bundleDirectory -Recurse -Force/);
  assert.match(workflow, /\$nsis\.Count -ne 1/);
  assert.match(workflow, /\$msi\.Count -ne 1/);
  assert.match(workflow, /\$nsis\[0\]\.Name -notmatch \[regex\]::Escape\(\$version\)/);
  assert.match(workflow, /\$msi\[0\]\.Name -notmatch \[regex\]::Escape\(\$version\)/);
  assert.doesNotMatch(workflow, /\$version:/);
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
  assert.match(workflow, /Verify native runner architecture/);
  assert.match(workflow, /Expected native \$\{\{ matrix\.rust_target \}\} runner/);
});

test("Windows release guide forbids publishing unsigned candidates", () => {
  assert.match(releaseGuide, /Do not publish those artifacts/);
  assert.match(releaseGuide, /Microsoft Store MSIX/);
  assert.match(releaseGuide, /Release[^\n]*requires every executable artifact/i);
  assert.match(releaseGuide, /SHA256SUMS-Windows-<architecture>/);
  assert.match(releaseGuide, /Sagascript-Windows-<architecture>-Portable\.exe/);
  assert.match(releaseGuide, /Sagascript-Windows-<architecture>-CLI\.exe/);
  assert.match(releaseGuide, /Sagascript-Windows-<architecture>-Setup\.exe/);
  assert.match(releaseGuide, /Sagascript-Windows-<architecture>\.msi/);
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
