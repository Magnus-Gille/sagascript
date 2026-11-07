import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { actionNames, allSteps, loadAction, loadWorkflow, readRepoFile, workflowFiles } from "./lib/ci-workflow.mjs";

const rustCacheRef = "Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6";

const [testBuildWorkflow, windowsWorkflow] = await Promise.all([
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
  const cacheUses = (steps) => steps.map((step) => step.uses).filter((uses) => uses?.startsWith("Swatinem/rust-cache@"));
  // The cache is part of the setup action (#358), the one place that names the revision; ci.yml's
  // six lanes (test-macos, build-macos, test-linux, msrv-linux #353, test-windows, build-windows)
  // reach it through the action, which scripts/test-ci-parallel-gates.mjs checks lane by lane.
  assert.deepEqual(cacheUses(loadAction("setup").steps), [rustCacheRef]);
  assert.deepEqual(
    cacheUses(allSteps(loadWorkflow()).map(({ step }) => step)),
    [],
    "a ci.yml lane that names its own rust-cache would bypass the shared setup",
  );
});

test("signed test builds use the shared dependency cache without raw target dumps", () => {
  const cache = section(testBuildWorkflow, "name: Cache Rust dependencies", "name: Install npm dependencies");
  assert.match(cache, new RegExp(`uses: ${rustCacheRef.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
  assert.match(cache, /workspaces:\s+src-tauri/);
  assert.match(cache, /shared-key:\s+sagascript-test-macos-arm64/);
  assert.doesNotMatch(cache, /actions\/cache|src-tauri\/target/);
  assert.ok(
    testBuildWorkflow.indexOf("name: Import Developer ID certificate") > testBuildWorkflow.indexOf("name: Cache Rust dependencies"),
    "signing must remain after the dependency cache",
  );
});

test("Windows native cache is isolated by runner image and toolchain namespace", () => {
  assert.match(
    windowsWorkflow,
    /concurrency:\s*\n(?:\s*#[^\n]*\n)*\s*group:\s+\$\{\{ github\.workflow \}\}-\$\{\{ github\.event\.pull_request\.number \|\| github\.run_id \}\}[^\n]*\n\s*cancel-in-progress:\s+true/,
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
  assert.match(cache, /workspaces:\s*\|\s*\n\s+src-tauri\s*\n\s+src-tauri\/engine-host\/ort/);
  assert.match(
    cache,
    /shared-key:\s+windows-package-\$\{\{ matrix\.architecture \}\}-\$\{\{ steps\.windows-image\.outputs\.image_os \}\}-\$\{\{ steps\.windows-image\.outputs\.image_version \}\}-\$\{\{ hashFiles\('\.github\/workflows\/windows-package\.yml', 'scripts\/cmake\/windows-x64-portable\.cmake', 'scripts\/cmake\/windows-arm64-native\.cmake', 'scripts\/verify-windows-x64-cpu-policy\.ps1'\) \}\}/,
  );
  assert.ok(
    windowsWorkflow.indexOf("name: Remove cached installer bundles") > cacheStart,
    "installer cleanup must remain after the native build gates",
  );
});

test("Windows candidates cache only verified models with a candidate primary key", () => {
  const modelCacheStart = windowsWorkflow.indexOf("name: Cache downloaded models");
  const identityStart = windowsWorkflow.indexOf("name: Pin validated Windows build identity");
  const rustCacheStart = windowsWorkflow.indexOf("name: Cache Rust dependencies");
  assert.ok(modelCacheStart > rustCacheStart && identityStart > modelCacheStart);

  const cache = section(windowsWorkflow, "name: Cache downloaded models", "name: Pin validated Windows build identity");
  // SHA-pinned (#354); the trailing comment names the v5 release it resolves to. Restore only (SAGA-546):
  // the last step of the job saves the cache, on main.
  assert.match(cache, /uses: actions\/cache\/restore@[0-9a-f]{40} # v5\.\d+\.\d+$/m);
  assert.match(cache, /id: models-cache/);
  assert.match(cache, /path: ~\\AppData\\Roaming\\Sagascript\\Models/);
  assert.equal((cache.match(/^\s*path:/gm) ?? []).length, 1, "cache must contain one exact model path");
  assert.equal(
    cache.match(/^\s*path:\s*(.+)$/m)?.[1],
    "~\\AppData\\Roaming\\Sagascript\\Models",
    "cache must exclude settings and logs from the model path",
  );
  assert.match(
    cache,
    /key: windows-models-package-\$\{\{ matrix\.architecture \}\}-\$\{\{ hashFiles\('src-tauri\/crates\/sagascript-core\/src\/models\/sources\.rs', 'src-tauri\/crates\/sagascript-core\/src\/transcription\/pianissimo_model\.rs', 'src-tauri\/crates\/sagascript-core\/src\/transcription\/model\.rs', 'src-tauri\/crates\/sagascript-core\/src\/diarization\/model\.rs'\) \}\}/,
  );
  assert.match(cache, /restore-keys:[\s\S]*windows-models-package-\$\{\{ matrix\.architecture \}\}-[\s\S]*windows-models-/);
  const saveStart = windowsWorkflow.indexOf("name: Save downloaded models cache");
  const save = windowsWorkflow.slice(saveStart);
  assert.ok(saveStart > windowsWorkflow.indexOf("name: Upload internal Windows candidate"), "the models save must be the last step");
  assert.match(save, /uses: actions\/cache\/save@[0-9a-f]{40} # v5\.\d+\.\d+$/m);
  assert.match(save, /key: \$\{\{ steps\.models-cache\.outputs\.cache-primary-key \}\}/);
  assert.equal(save.match(/^\s*path:\s*(.+)$/m)?.[1], "~\\AppData\\Roaming\\Sagascript\\Models");
  assert.match(save, /if: success\(\) && github\.ref == 'refs\/heads\/main' && steps\.models-cache\.outputs\.cache-hit != 'true'/);
  const nativeGate = windowsWorkflow.indexOf("name: Gate real Windows transcription");
  const englishGate = windowsWorkflow.indexOf("name: Gate repeated English dictation");
  assert.ok(nativeGate > modelCacheStart && englishGate > nativeGate);
  assert.ok(
    windowsWorkflow.indexOf("& $binary download-model nb-whisper-tiny", nativeGate) > modelCacheStart,
    "native smoke must verify restored models after cache restore",
  );
  assert.ok(
    windowsWorkflow.indexOf("& $binary download-model base.en", englishGate) > modelCacheStart,
    "English benchmark must verify restored base.en after cache restore",
  );
});

// SAGA-546: a cache saved from refs/pull/N/merge or refs/tags/vX is scoped to that ref and can never be
// read by another run, yet it counts against the repository's 10 GB limit and evicts what main saved.
test("only main saves Actions caches (SAGA-546)", () => {
  const mainOnly = "github.ref == 'refs/heads/main'";
  // The single combined actions/cache step: ~23 MB, saves only when the model manifest hash changes.
  const combinedAllowlist = { ".github/actions/setup": "Cache downloaded models" };
  const units = [
    ...workflowFiles().map((file) => ({
      label: `.github/workflows/${file}`,
      steps: allSteps(loadWorkflow(file)).map(({ step }) => step),
    })),
    ...actionNames().map((name) => ({ label: `.github/actions/${name}`, steps: loadAction(name).steps })),
  ];
  let rustCaches = 0;
  let saves = 0;
  for (const { label, steps } of units) {
    for (const step of steps) {
      const where = `${label} step "${step.name}"`;
      if (step.uses?.startsWith("Swatinem/rust-cache@")) {
        rustCaches += 1;
        assert.equal(step.with["save-if"], `\${{ ${mainOnly} }}`, `${where} must save the Rust cache only on main`);
      }
      if (step.uses?.startsWith("actions/cache/save@")) {
        saves += 1;
        assert.ok(step.condition?.includes(mainOnly), `${where} must save only on main, got: ${step.condition}`);
      }
      if (step.uses?.startsWith("actions/cache@") && combinedAllowlist[label] !== step.name) {
        assert.fail(`${where} uses the combined actions/cache, which saves on every ref; split it into restore + save`);
      }
    }
  }
  assert.ok(rustCaches >= 5 && saves >= 4, `the scan must find the cache steps (rust ${rustCaches}, save ${saves})`);
});

// SAGA-546: the Linux apt step restores cached .deb files, and only main saves them.
test("the setup action caches apt packages on Linux, restore-only off main (SAGA-546)", () => {
  const steps = loadAction("setup").steps;
  const at = (name) => steps.findIndex((step) => step.name === name);
  const keyStep = steps[at("Compute the apt cache key")];
  const restore = steps[at("Restore apt packages cache")];
  const install = steps[at("Install Tauri system dependencies")];
  const save = steps[at("Save apt packages cache")];
  assert.ok(keyStep && restore && install && save, "the apt cache steps exist");
  assert.ok(keyStep.index < restore.index && restore.index < install.index && install.index < save.index);
  for (const step of [keyStep, restore, install, save]) {
    assert.match(step.condition, /inputs\.linux-deps == 'true'/, `${step.name} runs only for linux-deps`);
  }
  assert.match(restore.uses, /^actions\/cache\/restore@[0-9a-f]{40}$/);
  assert.equal(restore.continueOnError, true, "a failed restore must fall back to downloading");
  assert.match(save.uses, /^actions\/cache\/save@[0-9a-f]{40}$/);
  assert.equal(save.continueOnError, true);
  for (const part of ["success()", "github.ref == 'refs/heads/main'", "steps.apt-cache.outputs.cache-hit != 'true'"]) {
    assert.ok(save.condition.includes(part), `save condition must include ${part}: ${save.condition}`);
  }
  assert.equal(restore.with.key, save.with.key);
  assert.equal(restore.with.path, save.with.path);
  assert.match(keyStep.run, /\$\{RUNNER_OS\}/);
  assert.match(keyStep.run, /\$\{ImageOS:-unknown\}/);
  assert.match(keyStep.run, /\$\{ImageVersion:-unknown\}/);
  assert.match(keyStep.run, /\$\{PACKAGES_HASH\}/);
  assert.equal(keyStep.env.PACKAGES_HASH, "${{ hashFiles('scripts/ci/apt-install-tauri-deps.sh') }}");
  assert.equal(install.env.APT_DEB_CACHE_DIR, "${{ steps.apt-key.outputs.dir }}");

  const script = readRepoFile("scripts/ci/apt-install-tauri-deps.sh");
  assert.doesNotMatch(script, /allow-unauthenticated|AllowInsecure|--force-yes|allow-insecure|trusted=yes|--no-check/i);
  assert.match(script, /sudo apt-get "\$\{APT_OPTS\[@\]\}" install -y --no-download/, "the retry/timeout structure stays");
  assert.match(script, /timeout "\$ATTEMPT_SECONDS" sudo apt-get/);
});
