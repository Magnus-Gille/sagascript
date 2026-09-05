import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const rustCacheRef =
  "Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6";

const [ciWorkflow, testBuildWorkflow, windowsWorkflow] = await Promise.all([
  readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"),
  readFile(new URL("../.github/workflows/test-build.yml", import.meta.url), "utf8"),
  readFile(new URL("../.github/workflows/windows-package.yml", import.meta.url), "utf8"),
]);

function section(content, startMarker, endMarker) {
  const start = content.indexOf(startMarker);
  const end = content.indexOf(endMarker, start + startMarker.length);
  assert.ok(start >= 0 && end > start, `section exists: ${startMarker}`);
  return content.slice(start, end);
}

test("CI rust caches use the reviewed immutable action revision", () => {
  const refs = ciWorkflow.match(/uses: Swatinem\/rust-cache@[^\s]+/g) ?? [];
  assert.equal(refs.length, 3);
  assert.deepEqual(refs, [`uses: ${rustCacheRef}`, `uses: ${rustCacheRef}`, `uses: ${rustCacheRef}`]);
});

test("signed test builds use the shared dependency cache without raw target dumps", () => {
  const cache = section(testBuildWorkflow, "name: Cache Rust dependencies", "name: Install npm dependencies");
  assert.match(cache, new RegExp(`uses: ${rustCacheRef.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
  assert.match(cache, /workspaces:\s+src-tauri/);
  assert.match(cache, /shared-key:\s+sagascript-test-macos-arm64/);
  assert.doesNotMatch(cache, /actions\/cache|src-tauri\/target/);
  assert.ok(
    testBuildWorkflow.indexOf("name: Import Developer ID certificate") >
      testBuildWorkflow.indexOf("name: Cache Rust dependencies"),
    "signing must remain after the dependency cache",
  );
});

test("Windows native cache is isolated by runner image and toolchain namespace", () => {
  assert.match(
    windowsWorkflow,
    /concurrency:\s*\n\s*group:\s+\$\{\{ github\.workflow \}\}-\$\{\{ github\.event\.pull_request\.number \|\| github\.run_id \}\}\s*\n\s*cancel-in-progress:\s+true/,
  );

  const imageStart = windowsWorkflow.indexOf("name: Identify Windows runner image");
  const cacheStart = windowsWorkflow.indexOf("name: Cache Rust dependencies");
  const nodeStart = windowsWorkflow.indexOf("name: Install Node.js");
  const armSetupStart = windowsWorkflow.indexOf("name: Configure native ARM64 C and C++ toolchain");
  assert.ok(imageStart > armSetupStart && cacheStart > imageStart);
  assert.ok(nodeStart > cacheStart, "cache must stay before Node setup and after native toolchain setup");

  const imageStep = section(windowsWorkflow, "name: Identify Windows runner image", "name: Cache Rust dependencies");
  assert.match(imageStep, /\$env:ImageOS/);
  assert.match(imageStep, /\$env:ImageVersion/);
  assert.match(imageStep, /IsNullOrWhiteSpace\(\$imageOs\)/);
  assert.match(imageStep, /IsNullOrWhiteSpace\(\$imageVersion\)/);
  assert.match(imageStep, /GITHUB_OUTPUT/);

  const cache = section(windowsWorkflow, "name: Cache Rust dependencies", "name: Install npm dependencies");
  assert.match(cache, new RegExp(`uses: ${rustCacheRef.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
  assert.match(cache, /workspaces:\s+src-tauri/);
  assert.match(
    cache,
    /shared-key:\s+windows-package-\$\{\{ matrix\.architecture \}\}-\$\{\{ steps\.windows-image\.outputs\.image_os \}\}-\$\{\{ steps\.windows-image\.outputs\.image_version \}\}-\$\{\{ hashFiles\('\.github\/workflows\/windows-package\.yml'\) \}\}/,
  );
  assert.ok(
    windowsWorkflow.indexOf("name: Remove cached installer bundles") > cacheStart,
    "installer cleanup must remain after the native build gates",
  );
});
