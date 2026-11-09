import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  REPO_ROOT,
  actionInput,
  actionNames,
  allSteps,
  loadAction,
  loadWorkflow,
  readRepoFile,
  workflowFiles,
} from "./lib/ci-workflow.mjs";

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

// SAGA-546: the release, signed test and Windows package builds saved Rust caches of 1.2 to 1.6 GB that
// no later run restored (the version bump, the runner image or eviction always came first), and saved
// after main's CI entries on the same push they made GitHub evict those. They compile cold.
test("only the setup action keeps a Rust cache: no other workflow saves or restores one (SAGA-546)", () => {
  for (const file of workflowFiles()) {
    for (const { job, step } of allSteps(loadWorkflow(file))) {
      const where = `${file} job ${job.id} step "${step.name}"`;
      assert.ok(!step.uses?.startsWith("Swatinem/rust-cache@"), `${where} must not keep a Rust cache of its own`);
      if (step.uses?.startsWith("actions/cache")) {
        assert.doesNotMatch(String(step.with.path ?? ""), /target/, `${where} must not cache a Cargo target directory`);
      }
    }
  }
  assert.doesNotMatch(windowsWorkflow, /name: Identify Windows runner image|steps\.windows-image\./);
  assert.doesNotMatch(testBuildWorkflow, /name: Cache Rust dependencies/);
  assert.ok(
    windowsWorkflow.indexOf("name: Remove cached installer bundles") > windowsWorkflow.indexOf("name: Gate real Windows transcription"),
    "installer cleanup must remain after the native build gates",
  );
});

test("Windows candidates cache only verified models with a candidate primary key", () => {
  const modelCacheStart = windowsWorkflow.indexOf("name: Cache downloaded models");
  const identityStart = windowsWorkflow.indexOf("name: Pin validated Windows build identity");
  const nodeStart = windowsWorkflow.indexOf("name: Install Node.js");
  assert.ok(nodeStart >= 0 && modelCacheStart > nodeStart && identityStart > modelCacheStart);

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
  assert.ok(rustCaches === 1 && saves >= 4, `the scan must find the cache steps (rust ${rustCaches}, save ${saves})`);
});

// SAGA-546: the repository's Actions cache holds 10 GB, and GitHub evicts the least recently used entries
// first. A cache saved after main's CI entries on the same push (the release prebuild runs on main) is
// newer than they are, so everything that saves has to fit with the CI entries, with room to spare, or the
// six entries every pull request compiles from are the ones evicted. Each step that saves has its measured
// size here (MB as `gh cache list` and the restore logs report it, 2026-11-09: `gh cache list`, and the
// restore logs of main's CI run 42484311483 for the Rust entries evicted before it). A new cache, or a lane
// that installs Rust, needs an entry; the total must stay under 85% of the limit.
const CACHE_LIMIT_MB = 9536; // 10 GB
const CACHE_BUDGET_MB = {
  // The setup action's Rust cache, one entry per ci.yml lane that installs Rust (key: cache-key and OS).
  "ci.yml job test-macos": 1110, // sagascript-ci-checks, Darwin
  "ci.yml job build-macos": 640, // sagascript-ci-native, Darwin
  "ci.yml job test-linux": 1621, // sagascript-ci, Linux (both workspaces)
  "ci.yml job msrv-linux": 662, // sagascript-msrv, Linux
  "ci.yml job test-windows": 1073, // sagascript-ci-checks, Windows
  "ci.yml job build-windows": 636, // sagascript-ci-native, Windows
  // Every other step that saves.
  '.github/actions/setup step "Save apt packages cache"': 59,
  '.github/actions/setup step "Cache downloaded models"': 46, // macos-models and windows-models, 23 MB each
  '.github/workflows/release-quality-gate.yml step "Save downloaded models cache"': 131,
  '.github/workflows/release-quality-gate.yml step "Save diarization fixture cache"': 1,
  '.github/workflows/windows-package.yml step "Save pinned LLVM cache"': 807,
  '.github/workflows/windows-package.yml step "Save downloaded models cache"': 837, // arm64 688, x64 149
};
// setup-node's npm caches (about 45 MB per OS and lockfile, saved on any ref) are not steps of their own.
const NPM_ALLOWANCE_MB = 300;

test("every cache that saves has a measured size, and together they fit the 10 GB limit with headroom (SAGA-546)", () => {
  const setup = loadAction("setup");
  const found = [];
  for (const job of loadWorkflow().jobs) {
    const step = job.steps.find((candidate) => candidate.uses === "./.github/actions/setup");
    if (step && actionInput(setup, step, "rust") !== "false") found.push(`ci.yml job ${job.id}`);
  }
  const units = [
    ...workflowFiles().map((file) => ({
      label: `.github/workflows/${file}`,
      steps: allSteps(loadWorkflow(file)).map(({ step }) => step),
    })),
    ...actionNames().map((name) => ({ label: `.github/actions/${name}`, steps: loadAction(name).steps })),
  ];
  for (const { label, steps } of units) {
    for (const step of steps) {
      if (step.uses?.startsWith("actions/cache/save@") || step.uses?.startsWith("actions/cache@")) {
        found.push(`${label} step "${step.name}"`);
      }
    }
  }
  assert.deepEqual(found.sort(), Object.keys(CACHE_BUDGET_MB).sort(), "every cache that saves needs a measured budget entry");
  const total = Object.values(CACHE_BUDGET_MB).reduce((sum, mb) => sum + mb, 0) + NPM_ALLOWANCE_MB;
  assert.ok(total <= CACHE_LIMIT_MB * 0.85, `the caches total ${total} MB, more than 85% of the ${CACHE_LIMIT_MB} MB limit`);
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

// Runs the real script with stubbed sudo/apt/dpkg: only a cached .deb whose SHA-256 the (signed) index
// lists for its package and version reaches apt's archive, and caching problems never fail the step.
test(
  "the apt script seeds only index-verified cached packages and never fails on the cache (SAGA-546)",
  { skip: process.platform === "win32" && "bash script" },
  () => {
    const root = mkdtempSync(join(tmpdir(), "apt-cache-"));
    try {
      const bin = join(root, "bin");
      const archive = join(root, "archive");
      const cache = join(root, "cache");
      for (const dir of [bin, archive, cache]) mkdirSync(dir);
      writeFileSync(join(cache, "good_1.0_amd64.deb"), "good package bytes");
      writeFileSync(join(cache, "tampered_1.0_amd64.deb"), "tampered bytes");
      symlinkSync(join(cache, "good_1.0_amd64.deb"), join(cache, "link_1.0_amd64.deb"));
      const goodSum = createHash("sha256").update("good package bytes").digest("hex");
      const stub = (name, body) => {
        writeFileSync(join(bin, name), `#!/usr/bin/env bash\n${body}\n`);
        chmodSync(join(bin, name), 0o755);
      };
      stub("sudo", 'exec "$@"');
      stub("timeout", 'shift; exec "$@"');
      stub("apt-get", "exit 0");
      // dpkg-deb -f FILE Package Version: the file name says which package the stub claims to be.
      stub("dpkg-deb", 'n=$(basename "$2"); echo "Package: ${n%%_*}"; echo "Version: 1.0"');
      // The index lists the good file's hash for every package, so the tampered one cannot match.
      stub("apt-cache", `echo "Package: $2"; echo "SHA256: ${goodSum}"`);
      stub("sha256sum", 'shasum -a 256 "$@" 2>/dev/null || command -p sha256sum "$@"');
      const run = (cacheDir) =>
        spawnSync("bash", [join(REPO_ROOT, "scripts/ci/apt-install-tauri-deps.sh")], {
          env: {
            ...process.env,
            PATH: `${bin}:${process.env.PATH}`,
            APT_ARCHIVE_DIR: archive,
            APT_DEB_CACHE_DIR: cacheDir,
            APT_SOURCES_DIR: root,
          },
          encoding: "utf8",
        });
      const seeded = run(cache);
      assert.equal(seeded.status, 0, seeded.stderr);
      assert.deepEqual(readdirSync(archive).sort(), ["good_1.0_amd64.deb"], "only the verified file is seeded");
      assert.match(seeded.stdout, /Seeded apt with 1 verified cached packages \(2 discarded\)/);
      // A cache directory that cannot be created (a file is in the way) is only a warning.
      const blocked = join(root, "blocked");
      writeFileSync(blocked, "not a directory");
      const failedExport = run(blocked);
      assert.equal(failedExport.status, 0, failedExport.stderr);
      assert.match(failedExport.stderr, /could not create the apt cache directory/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);
