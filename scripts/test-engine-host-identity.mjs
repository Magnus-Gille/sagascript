// The shared engine-host build identity (#367): scripts/engine-host-identity.sh computes
// version, SHA and dirty for the Core ML host; scripts/ci-build-identity.mjs and the ONNX host's
// build script (src-tauri/engine-host/ort/build_identity.rs, tested by `cargo test` there) apply
// the same list of generated files. These tests run the script and the CI gate over the same
// fixture trees so the two cannot disagree, and check that
// scripts/check-engine-host-identity.sh --release rejects a dirty host.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

const scripts = fileURLToPath(new URL(".", import.meta.url));
const posix = process.platform !== "win32";
const skip = posix ? false : "the engine-host identity scripts are bash (macOS and Linux builds)";
const created = [];

after(() => {
  for (const directory of created) rmSync(directory, { recursive: true, force: true });
});

function temporary(prefix) {
  const directory = mkdtempSync(join(tmpdir(), prefix));
  created.push(directory);
  return directory;
}

// A repository laid out like this one, with the real script and generated-file list.
function fixture() {
  const cwd = temporary("sagascript-host-identity-");
  const globalConfig = join(cwd, ".fixture-gitconfig");
  writeFileSync(globalConfig, "");
  const env = {
    ...process.env,
    GIT_CONFIG_GLOBAL: globalConfig,
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_AUTHOR_NAME: "Fixture",
    GIT_AUTHOR_EMAIL: "fixture@example.invalid",
    GIT_COMMITTER_NAME: "Fixture",
    GIT_COMMITTER_EMAIL: "fixture@example.invalid",
  };
  const git = (...args) => execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();
  git("init", "-q");
  git("config", "commit.gpgsign", "false");
  mkdirSync(join(cwd, "scripts"));
  mkdirSync(join(cwd, "src-tauri/gen/schemas"), { recursive: true });
  for (const name of ["engine-host-identity.sh", "build-identity-generated-paths.txt", "ci-build-identity.mjs"]) {
    copyFileSync(join(scripts, name), join(cwd, "scripts", name));
  }
  chmodSync(join(cwd, "scripts/engine-host-identity.sh"), 0o755);
  writeFileSync(join(cwd, ".gitignore"), ".fixture-gitconfig\nignored/\n");
  writeFileSync(join(cwd, "src-tauri/tauri.conf.json"), '{\n  "productName": "Sagascript",\n  "version": "9.8.7"\n}\n');
  writeFileSync(join(cwd, "src-tauri/gen/schemas/desktop-schema.json"), "{}\n");
  writeFileSync(join(cwd, "src-tauri/lib.rs"), "fn main() {}\n");
  git("add", ".");
  git("commit", "-qm", "fixture");
  return { cwd, env, sha: git("rev-parse", "HEAD") };
}

function identity(repo) {
  const output = execFileSync(join(repo.cwd, "scripts/engine-host-identity.sh"), { cwd: repo.cwd, env: repo.env, encoding: "utf8" });
  return Object.fromEntries(
    output
      .trim()
      .split("\n")
      .map((line) => line.split("=")),
  );
}

// What the Windows packaging gate says about the same tree.
function gateAccepts(repo) {
  const result = spawnSync(process.execPath, [join(repo.cwd, "scripts/ci-build-identity.mjs"), "--verify"], {
    cwd: repo.cwd,
    env: {
      ...repo.env,
      GITHUB_SHA: repo.sha,
      SAGASCRIPT_GIT_HASH: repo.sha,
      SAGASCRIPT_BUILD_DATE: "2026-10-12",
      SAGASCRIPT_SOURCE_REVISION_INPUT: repo.sha,
      SAGASCRIPT_SOURCE_DIRTY_INPUT: "false",
    },
    encoding: "utf8",
  });
  return result.status === 0;
}

test("a clean tree: version from tauri.conf.json, full HEAD, not dirty", { skip }, () => {
  const repo = fixture();
  assert.deepEqual(identity(repo), { version: "9.8.7", git_sha: repo.sha, dirty: "false" });
  assert.ok(gateAccepts(repo));
});

test("the script and the CI gate agree on what makes a tree dirty", { skip }, () => {
  const cases = [
    {
      name: "rewritten generated schema",
      change: (cwd) => writeFileSync(join(cwd, "src-tauri/gen/schemas/desktop-schema.json"), '{"x":1}\n'),
      dirty: false,
    },
    {
      name: "created generated schema",
      change: (cwd) => writeFileSync(join(cwd, "src-tauri/gen/schemas/windows-schema.json"), "{}\n"),
      dirty: false,
    },
    {
      name: "gitignored file",
      change: (cwd) => {
        mkdirSync(join(cwd, "ignored"));
        writeFileSync(join(cwd, "ignored/x"), "x");
      },
      dirty: false,
    },
    { name: "modified source", change: (cwd) => writeFileSync(join(cwd, "src-tauri/lib.rs"), "fn main() { }\n"), dirty: true },
    { name: "untracked source", change: (cwd) => writeFileSync(join(cwd, "src-tauri/new.rs"), "\n"), dirty: true },
    {
      name: "schema not in the list",
      change: (cwd) => writeFileSync(join(cwd, "src-tauri/gen/schemas/macOS-schema.json"), "{}\n"),
      dirty: true,
    },
    { name: "deleted source", change: (cwd) => rmSync(join(cwd, "src-tauri/lib.rs")), dirty: true },
  ];
  for (const { name, change, dirty } of cases) {
    const repo = fixture();
    change(repo.cwd);
    assert.equal(identity(repo).dirty, String(dirty), name);
    assert.equal(gateAccepts(repo), !dirty, `${name}: ci-build-identity.mjs --verify`);
  }
});

test("every consumer reads the one list of generated files", () => {
  const list = readFileSync(join(scripts, "build-identity-generated-paths.txt"), "utf8");
  for (const consumer of ["engine-host-identity.sh", "ci-build-identity.mjs", "../src-tauri/engine-host/ort/build_identity.rs"]) {
    assert.match(readFileSync(join(scripts, consumer), "utf8"), /build-identity-generated-paths\.txt/, consumer);
  }
  assert.match(readFileSync(join(scripts, "build-engine-host.sh"), "utf8"), /engine-host-identity\.sh/);
  assert.match(list, /^src-tauri\/gen\/schemas\/windows-schema\.json$/m);
});

function fakeHost(versionLine) {
  const directory = temporary("sagascript-fake-host-");
  const path = join(directory, "sagascript-engine-host");
  writeFileSync(path, `#!/bin/sh\necho '${versionLine}'\n`);
  chmodSync(path, 0o755);
  return path;
}

function check(args) {
  return spawnSync(join(scripts, "check-engine-host-identity.sh"), args, { encoding: "utf8" });
}

test("check-engine-host-identity.sh: the SHA must match, and --release also needs a clean build", { skip }, () => {
  const sha = "a".repeat(40);
  const clean = fakeHost(`sagascript-engine-host 1.4.4 (${sha}, dirty=false)`);
  const dirty = fakeHost(`sagascript-engine-host 1.4.4 (${sha}, dirty=true)`);
  assert.equal(check([clean, sha]).status, 0);
  assert.equal(check(["--release", clean, sha]).status, 0);
  assert.equal(check([dirty, sha]).status, 0, "outside release builds a dirty host is only reported");
  const rejected = check(["--release", dirty, sha]);
  assert.equal(rejected.status, 1);
  assert.match(rejected.stderr, /clean tree/);
  assert.equal(check(["--release", clean, "b".repeat(40)]).status, 1);
  assert.equal(check(["--release", clean]).status, 2);
});

test("release paths check the host in release mode", () => {
  const workflow = readFileSync(join(scripts, "../.github/workflows/release-build-macos.yml"), "utf8");
  const checks = workflow.match(/check-engine-host-identity\.sh[^\n]*/g) ?? [];
  assert.ok(checks.length >= 2, "release-build-macos.yml checks the staged and the signed host");
  for (const line of checks) assert.match(line, /^check-engine-host-identity\.sh --release /, line);
  const verifier = readFileSync(join(scripts, "verify-macos-release.sh"), "utf8");
  assert.match(
    verifier,
    /if \[\[ "\$release" == true \]\]; then\s+"\$\(dirname "\$0"\)\/check-engine-host-identity\.sh" --release "\$host"/,
  );
  // The release paths ask verify-macos-release.sh for the clean-tree check; signed test builds
  // (test-build.yml, any branch) only check the host's SHA.
  assert.match(workflow, /verify-macos-release\.sh --release "\$app"/);
  assert.match(readFileSync(join(scripts, "verify-release-draft.sh"), "utf8"), /verify-macos-release\.sh" --release "\$app"/);
  const testBuild = readFileSync(join(scripts, "../.github/workflows/test-build.yml"), "utf8");
  assert.match(testBuild, /verify-macos-release\.sh "\$app"/);
  assert.doesNotMatch(testBuild, /verify-macos-release\.sh --release/);
});
