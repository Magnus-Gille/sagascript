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
const acceptanceScript = await readFile(
  new URL("./accept-windows-candidate.ps1", import.meta.url),
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
  assert.match(workflow, /name: Configure native ARM64 C and C\+\+ toolchain/);
  assert.match(workflow, /if: matrix\.architecture == 'arm64'/);
  assert.match(workflow, /CMAKE_GENERATOR=Ninja/);
  assert.match(workflow, /CMAKE_CXX_FLAGS=\/EHsc \/utf-8/);
  assert.match(workflow, /CMAKE_C_COMPILER=clang-cl/);
  assert.match(workflow, /CMAKE_CXX_COMPILER=clang-cl/);
  assert.match(workflow, /CMAKE_C_COMPILER_TARGET=aarch64-pc-windows-msvc/);
  assert.match(workflow, /CMAKE_CXX_COMPILER_TARGET=aarch64-pc-windows-msvc/);
  assert.match(
    workflow,
    /concurrency:\s*\n\s*group:\s+\$\{\{ github\.workflow \}\}-\$\{\{ github\.event\.pull_request\.number \|\| github\.run_id \}\}\s*\n\s*cancel-in-progress:\s+true/,
  );
  assert.match(workflow, /name: Identify Windows runner image/);
  assert.match(workflow, /\$env:ImageOS/);
  assert.match(workflow, /\$env:ImageVersion/);
  assert.match(workflow, /IsNullOrWhiteSpace\(\$imageOs\)/);
  assert.match(workflow, /IsNullOrWhiteSpace\(\$imageVersion\)/);
  assert.match(workflow, /GITHUB_OUTPUT/);
  const imageStart = workflow.indexOf("name: Identify Windows runner image");
  const cacheStart = workflow.indexOf("name: Cache Rust dependencies");
  const nodeStart = workflow.indexOf("name: Install Node.js");
  assert.ok(imageStart >= 0 && cacheStart > imageStart && nodeStart > cacheStart);
  const rustCache = workflow.slice(cacheStart, nodeStart);
  assert.match(
    rustCache,
    /uses: Swatinem\/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6/,
  );
  assert.match(rustCache, /workspaces:\s+src-tauri/);
  assert.match(
    rustCache,
    /shared-key:\s+windows-package-\$\{\{ matrix\.architecture \}\}-\$\{\{ steps\.windows-image\.outputs\.image_os \}\}-\$\{\{ steps\.windows-image\.outputs\.image_version \}\}-\$\{\{ hashFiles\('\.github\/workflows\/windows-package\.yml'\) \}\}/,
  );
  assert.doesNotMatch(
    rustCache,
    /actions\/cache/,
    "native cache must use the target-aware Rust cache namespace",
  );
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
  const downloadCalls = gate.match(/& \$binary download-model nb-whisper-tiny/g) ?? [];
  assert.equal(
    downloadCalls.length,
    2,
    "native gate must download once and re-verify the existing model",
  );
  const firstDownload = gate.indexOf("& $binary download-model nb-whisper-tiny");
  const secondDownload = gate.indexOf(
    "& $binary download-model nb-whisper-tiny",
    firstDownload + 1,
  );
  const transcription = gate.indexOf("& $binary transcribe");
  assert.ok(firstDownload >= 0 && secondDownload > firstDownload);
  assert.ok(transcription > secondDownload);
  assert.match(gate, /Existing model verification failed/);
  assert.match(gate, /verify-json-cli-streams\.py/);
  assert.doesNotMatch(gate, /continue-on-error/);
  assert.match(workflow, /Verify native runner architecture/);
  assert.match(workflow, /Expected native \$\{\{ matrix\.rust_target \}\} runner/);
});

test("Windows acceptance re-verifies an already-downloaded model", () => {
  const downloadCalls = acceptanceScript.match(
    /Invoke-Sagascript -Executable \$cliExePath -Arguments @\("download-model", "nb-whisper-tiny"\)/g,
  ) ?? [];
  assert.equal(
    downloadCalls.length,
    2,
    "packaged acceptance must exercise existing-model verification",
  );
  const firstDownload = acceptanceScript.indexOf(
    'Invoke-Sagascript -Executable $cliExePath -Arguments @("download-model", "nb-whisper-tiny")',
  );
  const secondDownload = acceptanceScript.indexOf(
    'Invoke-Sagascript -Executable $cliExePath -Arguments @("download-model", "nb-whisper-tiny")',
    firstDownload + 1,
  );
  const transcription = acceptanceScript.indexOf("$cliExePath transcribe");
  assert.ok(firstDownload >= 0 && secondDownload > firstDownload);
  assert.ok(transcription > secondDownload);
  assert.match(acceptanceScript, /verification_seconds/);
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
