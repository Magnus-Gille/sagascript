import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, test } from "node:test";

const helper = fileURLToPath(new URL("./ci-build-identity.mjs", import.meta.url));
const repos = [];
const envDirs = [];
const allowedOutput = "src-tauri/gen/schemas/desktop-schema.json";

function git(cwd, args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_AUTHOR_NAME: "CI Fixture",
      GIT_AUTHOR_EMAIL: "ci@example.invalid",
      GIT_COMMITTER_NAME: "CI Fixture",
      GIT_COMMITTER_EMAIL: "ci@example.invalid",
    },
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

async function fixture() {
  const cwd = await mkdtemp(join(tmpdir(), "sagascript-ci-identity-"));
  repos.push(cwd);
  git(cwd, ["init", "-q"]);
  git(cwd, ["config", "user.email", "ci@example.invalid"]);
  git(cwd, ["config", "user.name", "CI Fixture"]);
  git(cwd, ["config", "commit.gpgsign", "false"]);
  await mkdir(join(cwd, "src-tauri/capabilities"), { recursive: true });
  await mkdir(join(cwd, "src-tauri/gen/schemas"), { recursive: true });
  await writeFile(join(cwd, "src-tauri/capabilities/default.json"), "{\"permission\":true}\n");
  for (const relativePath of [
    "src-tauri/gen/schemas/acl-manifests.json",
    "src-tauri/gen/schemas/capabilities.json",
    "src-tauri/gen/schemas/desktop-schema.json",
  ]) {
    await writeFile(join(cwd, relativePath), "{}\n");
  }
  git(cwd, ["add", "."]);
  git(cwd, ["commit", "-qm", "fixture baseline"]);
  const sha = git(cwd, ["rev-parse", "HEAD"]);
  return { cwd, sha };
}

async function envPath() {
  const directory = await mkdtemp(join(tmpdir(), "sagascript-ci-env-"));
  envDirs.push(directory);
  return join(directory, "github.env");
}

function invoke(cwd, args, extraEnv = {}) {
  return spawnSync(process.execPath, [helper, ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, ...extraEnv },
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function assertRejected(result) {
  assert.notEqual(result.status, 0, "expected helper rejection");
  assert.equal(result.stdout, "");
}

async function initialized(repo) {
  const githubEnv = await envPath();
  const result = invoke(repo.cwd, ["--initialize"], {
    GITHUB_SHA: repo.sha,
    GITHUB_ENV: githubEnv,
  });
  assert.equal(result.status, 0, result.stderr);
  const envText = await readFile(githubEnv, "utf8");
  assert.match(envText, new RegExp("^SAGASCRIPT_GIT_HASH=" + repo.sha + "\\nSAGASCRIPT_BUILD_DATE=\\d{4}-\\d{2}-\\d{2}\\n$"));
  return envText.match(/SAGASCRIPT_BUILD_DATE=(\d{4}-\d{2}-\d{2})/)[1];
}

function verifyEnv(repo, date, overrides = {}) {
  return {
    GITHUB_SHA: repo.sha,
    SAGASCRIPT_GIT_HASH: repo.sha,
    SAGASCRIPT_BUILD_DATE: date,
    ...overrides,
  };
}

afterEach(async () => {
  await Promise.all(repos.splice(0).map((repo) => rm(repo, { recursive: true, force: true })));
  await Promise.all(envDirs.splice(0).map((directory) => rm(directory, { recursive: true, force: true })));
});

test("initialize emits full SHA and UTC date for a clean fixture", async () => {
  const repo = await fixture();
  const githubEnv = await envPath();
  const result = invoke(repo.cwd, ["--initialize"], { GITHUB_SHA: repo.sha, GITHUB_ENV: githubEnv });
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.equal(output.mode, "initialize");
  assert.equal(output.git_sha, repo.sha);
  assert.match(output.build_date, /^\d{4}-\d{2}-\d{2}$/);
  assert.deepEqual(output.allowed_generated_paths, [
    "src-tauri/gen/schemas/acl-manifests.json",
    "src-tauri/gen/schemas/capabilities.json",
    "src-tauri/gen/schemas/desktop-schema.json",
    "src-tauri/gen/schemas/windows-schema.json",
  ]);
  assert.equal((await readFile(githubEnv, "utf8")).split("\n").length, 3);
});

test("initialize rejects wrong or malformed expected SHA", async () => {
  for (const expected of ["0".repeat(40), "A".repeat(40), "not-a-sha"]) {
    const repo = await fixture();
    const githubEnv = await envPath();
    const result = invoke(repo.cwd, ["--initialize"], { GITHUB_SHA: expected, GITHUB_ENV: githubEnv });
    assertRejected(result);
    await assert.rejects(readFile(githubEnv, "utf8"));
  }
});

test("initialize rejects git failure and dirty tracked or untracked files", async () => {
  const outside = await mkdtemp(join(tmpdir(), "sagascript-ci-not-repo-"));
  repos.push(outside);
  const failedGit = invoke(outside, ["--initialize"], {
    GITHUB_SHA: "0".repeat(40),
    GITHUB_ENV: await envPath(),
  });
  assertRejected(failedGit);

  const repo = await fixture();
  await writeFile(join(repo.cwd, "src-tauri/capabilities/default.json"), "{\"permission\":false}\n");
  await writeFile(join(repo.cwd, "unexpected.txt"), "untracked\n");
  const result = invoke(repo.cwd, ["--initialize"], {
    GITHUB_SHA: repo.sha,
    GITHUB_ENV: await envPath(),
  });
  assertRejected(result);
});

test("initialize rejects missing GITHUB_ENV and malformed CLI arguments", async () => {
  const repo = await fixture();
  assertRejected(invoke(repo.cwd, ["--initialize"], { GITHUB_SHA: repo.sha, GITHUB_ENV: "" }));
  assertRejected(invoke(repo.cwd, [], { GITHUB_SHA: repo.sha }));
  assertRejected(invoke(repo.cwd, ["--verify", "extra"], { GITHUB_SHA: repo.sha }));
});

test("verify rejects a changed HEAD after initialization", async () => {
  const repo = await fixture();
  const date = await initialized(repo);
  await writeFile(join(repo.cwd, "new-source.txt"), "new commit\n");
  git(repo.cwd, ["add", "new-source.txt"]);
  git(repo.cwd, ["commit", "-qm", "second fixture commit"]);
  assertRejected(invoke(repo.cwd, ["--verify"], verifyEnv(repo, date)));
});

test("verify permits only regular generated schema outputs", async () => {
  const repo = await fixture();
  const date = await initialized(repo);
  await writeFile(join(repo.cwd, allowedOutput), "{\"generated\":true}\n");
  await writeFile(join(repo.cwd, "src-tauri/gen/schemas/windows-schema.json"), "{}\n");
  const result = invoke(repo.cwd, ["--verify"], verifyEnv(repo, date));
  assert.equal(result.status, 0, result.stderr);
  const output = JSON.parse(result.stdout);
  assert.deepEqual(output.observed_generated_paths, [
    "src-tauri/gen/schemas/desktop-schema.json",
    "src-tauri/gen/schemas/windows-schema.json",
  ]);
});

test("verify rejects unexpected, untracked, and capability-source changes", async () => {
  for (const change of [
    async (repo) => writeFile(join(repo.cwd, "unexpected.txt"), "nope\n"),
    async (repo) => writeFile(join(repo.cwd, "src-tauri/capabilities/default.json"), "{\"permission\":false}\n"),
  ]) {
    const repo = await fixture();
    const date = await initialized(repo);
    await change(repo);
    const result = invoke(repo.cwd, ["--verify"], verifyEnv(repo, date));
    assertRejected(result);
  }
});

test("verify rejects generated deletion and rename", async () => {
  const deleted = await fixture();
  const deletedDate = await initialized(deleted);
  await rm(join(deleted.cwd, allowedOutput));
  assertRejected(invoke(deleted.cwd, ["--verify"], verifyEnv(deleted, deletedDate)));

  const renamed = await fixture();
  const renamedDate = await initialized(renamed);
  git(renamed.cwd, ["mv", allowedOutput, allowedOutput + ".renamed"]);
  assertRejected(invoke(renamed.cwd, ["--verify"], verifyEnv(renamed, renamedDate)));
});

test("verify rejects generated symlink", async (t) => {
  const symlinked = await fixture();
  const symlinkedDate = await initialized(symlinked);
  await rm(join(symlinked.cwd, allowedOutput));
  try {
    await symlink("capabilities.json", join(symlinked.cwd, allowedOutput));
  } catch (error) {
    if (error && (error.code === "EPERM" || error.code === "EACCES")) {
      t.skip("host denied symlink creation");
      return;
    }
    throw error;
  }
  assertRejected(invoke(symlinked.cwd, ["--verify"], verifyEnv(symlinked, symlinkedDate)));
});

test("verify rejects missing, wrong, and invalid metadata", async () => {
  for (const metadata of [
    { SAGASCRIPT_GIT_HASH: "", SAGASCRIPT_BUILD_DATE: "" },
    { SAGASCRIPT_GIT_HASH: "0".repeat(40) },
    { SAGASCRIPT_GIT_HASH: "not-a-sha", SAGASCRIPT_BUILD_DATE: "2026-09-06" },
  ]) {
    const repo = await fixture();
    const result = invoke(repo.cwd, ["--verify"], verifyEnv(repo, "2026-09-06", metadata));
    assertRejected(result);
  }
});

test("verify rejects an invalid build date after SHA validation", async () => {
  const repo = await fixture();
  const result = invoke(repo.cwd, ["--verify"], verifyEnv(repo, "2026-02-30"));
  assertRejected(result);
  assert.match(result.stderr, /SAGASCRIPT_BUILD_DATE/);
});

test("status diagnostics preserve a Unicode untracked path", async () => {
  const repo = await fixture();
  await writeFile(join(repo.cwd, "naïve.txt"), "untracked\n");
  const result = invoke(repo.cwd, ["--initialize"], {
    GITHUB_SHA: repo.sha,
    GITHUB_ENV: await envPath(),
  });
  assertRejected(result);
  assert.match(result.stderr, /path="naïve\.txt" xy="\?\?"/);
});
