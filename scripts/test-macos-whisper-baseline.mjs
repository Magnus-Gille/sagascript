// macOS builds compile whisper.cpp for Apple silicon's apple-m1 baseline, not for the CI runner's CPU
// (#546). GGML_NATIVE broke the build on the macos-15 runners and would tie a shipped binary to the
// runner's CPU; scripts/cmake/macos-arm64-portable.cmake turns it off, and every macOS job that
// compiles Rust must load that hook before its Rust cache, whose key includes CMAKE_* variables.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { loadAction, loadWorkflow, readRepoFile, REPO_ROOT, workflowFiles } from "./lib/ci-workflow.mjs";

const SCRIPT = "bash scripts/ci/use-portable-whisper-macos.sh";
const HOOK = "scripts/cmake/macos-arm64-portable.cmake";
const appliesBaseline = (step) => step.run === SCRIPT;
const isRustCache = (step) => step.uses?.startsWith("Swatinem/rust-cache@") ?? false;
const isSetup = (step) => step.uses === "./.github/actions/setup";

// Jobs that compile the workspace without a Rust cache step of their own, by the first step that compiles
// Rust. The release, signed test and package builds keep no Rust cache (SAGA-546, test-pipeline-cache.mjs).
const UNCACHED_COMPILERS = {
  "code-health.yml": { collect: "Collect static measurements" },
  "release-build-macos.yml": { "build-macos": "Build, sign, notarize, and staple Apple Silicon app" },
  "release-quality-gate.yml": { "quality-gate": "Test Rust workspace" },
  "test-build.yml": { "build-macos-test": "Build, sign, notarize, and staple Apple Silicon app" },
};

test("the setup action applies the baseline on macOS before its Rust cache", () => {
  const { steps } = loadAction("setup");
  const baseline = steps.findIndex(appliesBaseline);
  assert.ok(baseline >= 0, "the setup action must run the whisper.cpp baseline script");
  assert.equal(steps[baseline].condition, "${{ runner.os == 'macOS' }}");
  assert.ok(baseline < steps.findIndex(isRustCache), "the baseline must be set before the Rust cache keys on CMAKE_*");
});

test("every macOS job that compiles Rust applies the whisper.cpp baseline first", () => {
  const checked = [];
  for (const file of workflowFiles()) {
    for (const job of loadWorkflow(file).jobs) {
      if (!String(job.runsOn ?? "").startsWith("macos-")) continue;
      const where = `${file} job ${job.id}`;
      const cache = job.steps.findIndex(isRustCache);
      const uncached = UNCACHED_COMPILERS[file]?.[job.id];
      if (job.steps.some(isSetup)) {
        assert.equal(cache, -1, `${where} must take its Rust cache from the setup action`);
        checked.push(where);
        continue;
      }
      if (cache < 0 && !uncached) continue;
      const baseline = job.steps.findIndex(appliesBaseline);
      assert.ok(baseline >= 0, `${where} compiles Rust on macOS and must run ${SCRIPT}`);
      const before = cache >= 0 ? cache : job.steps.findIndex((step) => step.name === uncached);
      assert.ok(before >= 0 && baseline < before, `${where} must apply the baseline before it compiles or restores Rust`);
      checked.push(where);
    }
  }
  for (const where of [
    "ci.yml job test-macos",
    "ci.yml job build-macos",
    "code-health.yml job collect",
    "release-build-macos.yml job build-macos",
    "release-quality-gate.yml job quality-gate",
    "test-build.yml job build-macos-test",
  ]) {
    assert.ok(checked.includes(where), `expected to check ${where}; checked: ${checked.join(", ")}`);
  }
});

test("the CMake hook turns off GGML_NATIVE for whisper.cpp on Apple silicon only", () => {
  const hook = readRepoFile(HOOK);
  const code = hook
    .split("\n")
    .filter((line) => !line.trimStart().startsWith("#"))
    .join("\n");
  assert.match(code, /^if\(NOT PROJECT_NAME STREQUAL "whisper\.cpp"\)\n {2}return\(\)\nendif\(\)/, "scoped to whisper.cpp first");
  assert.match(code, /if\(NOT CMAKE_SYSTEM_NAME STREQUAL "Darwin"\)\n {2}message\(FATAL_ERROR/);
  assert.match(code, /STREQUAL "arm64"/);
  assert.match(code, /^set\(GGML_NATIVE OFF CACHE BOOL "[^"]+" FORCE\)$/m);
  // No explicit architecture: Apple clang's arm64 macOS default (apple-m1) is the baseline.
  assert.doesNotMatch(code, /GGML_CPU_ARM_ARCH|-mcpu|-march/);
  assert.doesNotMatch(code, /^set\(GGML_\w+ ON\b/m, "the hook must not switch any GGML instruction set on");
});

test("the baseline script exports the hook through CMAKE_PROJECT_INCLUDE", { skip: process.platform === "win32" && "bash script" }, () => {
  const dir = mkdtempSync(join(tmpdir(), "whisper-baseline-"));
  try {
    const envFile = join(dir, "github-env");
    writeFileSync(envFile, "");
    const workspace = REPO_ROOT.replace(/\/$/, "");
    execFileSync("bash", ["scripts/ci/use-portable-whisper-macos.sh"], {
      cwd: REPO_ROOT,
      env: { PATH: process.env.PATH, GITHUB_ENV: envFile, GITHUB_WORKSPACE: workspace },
      stdio: "pipe",
    });
    assert.equal(readFileSync(envFile, "utf8"), `CMAKE_PROJECT_INCLUDE=${workspace}/${HOOK}\n`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
