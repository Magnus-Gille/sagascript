// SAGA-551: a pull request builds the Windows packages only when it is relevant; the release path never skips.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { REPO_ROOT, loadWorkflow } from "./lib/ci-workflow.mjs";
import { LABEL, WINDOWS_PACKAGE_PATHS, decide, pathMatches } from "./ci/windows-package-gate.mjs";

const workflow = loadWorkflow("windows-package.yml");
const GATE = "${{ !cancelled() && (github.event_name != 'pull_request' || needs.decide.outputs.run == 'true') }}";

test("workflow_call and workflow_dispatch stay triggers, and pull_request also fires when a label is added", () => {
  const on = workflow.doc.on;
  assert.ok("workflow_call" in on && "workflow_dispatch" in on);
  assert.deepEqual(on.pull_request.types, ["opened", "synchronize", "reopened", "labeled"]);
  assert.equal(on.pull_request.paths, undefined, "the decision job owns path filtering; a paths: filter would also hide label runs");
});

test("only the pull_request event can skip the package jobs; every other event runs them", () => {
  const build = workflow.job("build-windows-candidate");
  assert.deepEqual(build.needs, ["decide"]);
  assert.equal(build.condition, GATE);
  // The gate is false only for a pull request whose decision said no, whatever the called/dispatched context.
  // run "" is a failed or skipped `decide`: !cancelled() (not success()) keeps a release build running then.
  assert.match(GATE, /^\$\{\{ !cancelled\(\) && \(/, "a failed decide job must not skip the release path");
  const gate = (event, run) => event !== "pull_request" || run === "true";
  for (const event of ["workflow_call", "push", "workflow_dispatch"]) {
    for (const run of ["true", "false", ""]) assert.ok(gate(event, run), `${event} with run=${run} builds`);
  }
  assert.ok(!gate("pull_request", ""), "a pull request whose decide failed is skipped, not built");
  assert.ok(gate("pull_request", "true"));
  assert.ok(!gate("pull_request", "false"));
  assert.ok(!gate("pull_request", ""));
  // The only other job is the decision job itself, which is never skipped.
  assert.deepEqual(workflow.jobIds, ["decide", "build-windows-candidate"]);
  assert.equal(workflow.job("decide").condition, undefined);
});

test("the decision job is small, secret-free and read-only", () => {
  const decideJob = workflow.job("decide");
  assert.match(decideJob.runsOn, /^ubuntu-/);
  assert.ok(decideJob.timeoutMinutes <= 5);
  assert.deepEqual(decideJob.outputs, { run: "${{ steps.gate.outputs.run }}" });
  assert.equal(workflow.doc.permissions.contents, "read");
  assert.equal(Object.keys(workflow.doc.permissions).length, 1);
  assert.doesNotMatch(JSON.stringify(decideJob), /secrets\./);
  for (const step of decideJob.steps.filter((s) => s.uses)) assert.match(step.uses, /^actions\/checkout@[0-9a-f]{40}$/);
  const run = decideJob.steps.find((s) => s.id === "gate");
  assert.equal(run.run, "node scripts/ci/windows-package-gate.mjs");
  assert.equal(run.env.EVENT_NAME, "${{ github.event_name }}");
  assert.match(run.env.PR_LABELS, /github\.event\.pull_request\.labels\.\*\.name/);
});

test("an unrelated label cannot cancel a package build that is running", () => {
  const group = workflow.doc.concurrency.group;
  assert.match(group, /github\.event\.action == 'labeled' && github\.event\.label\.name != 'windows-package'/);
  assert.equal(workflow.doc.concurrency["cancel-in-progress"], true);
});

test("the decision covers the label, a version change and the packaging paths", () => {
  const none = { changedFiles: ["src/App.svelte"], versions: [{ file: "package.json", before: "1.0.0", after: "1.0.0" }] };
  assert.equal(decide({ ...none, labels: ["bug"] }).run, false);
  assert.equal(decide({ ...none, labels: ["bug", LABEL] }).run, true);
  assert.equal(LABEL, "windows-package");
  const bump = { before: "1.0.0", after: "1.1.0" };
  for (const file of ["package.json", "src-tauri/tauri.conf.json"]) {
    assert.equal(decide({ ...none, versions: [{ file, ...bump }] }).run, true, file);
  }
  for (const path of WINDOWS_PACKAGE_PATHS) {
    const file = path.endsWith("/") ? `${path}nested/file` : path;
    assert.equal(decide({ ...none, changedFiles: ["README.md", file] }).run, true, file);
  }
  // Dependency changes can change the bundler or what is packaged (Dependabot's groups do).
  for (const file of ["package-lock.json", "src-tauri/Cargo.lock", "src-tauri/Cargo.toml"]) {
    assert.ok(pathMatches(file), file);
  }
  for (const file of ["src-tauri/src/lib.rs", "docs/ci-performance.md", "src-tauri/engine-host/coreml/x.swift", "scripts/cmake-notes.md"]) {
    assert.ok(!pathMatches(file), file);
  }
  // No change set (no base commit) fails open.
  assert.equal(decide({ labels: [] }).run, true);
});

test("every Windows-only script and config the workflow reads is in the path list", () => {
  const text = readFileSync(join(REPO_ROOT, ".github/workflows/windows-package.yml"), "utf8");
  const read = new Set(
    text.match(
      /scripts\/(?:cmake\/)?[A-Za-z0-9._-]*(?:windows|ci-build-identity|json-cli-streams|tauri-ci-prebuilt)[A-Za-z0-9._-]*\.(?:ps1|mjs|json|cmake|py)/g,
    ),
  );
  assert.ok(read.size >= 8, `found ${[...read]}`);
  for (const file of read) {
    if (file.startsWith("scripts/test-")) continue; // tests run in the Windows job but package nothing
    assert.ok(pathMatches(file), `${file} is read by windows-package.yml but not in WINDOWS_PACKAGE_PATHS`);
  }
});

test("the gate script reads a real merge commit: a docs-only change skips, a packaging change and a version bump run", () => {
  const script = join(dirname(fileURLToPath(import.meta.url)), "ci/windows-package-gate.mjs");
  const dir = mkdtempSync(join(tmpdir(), "wpg-"));
  const vcs = (...args) =>
    execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", ...args], { cwd: dir, stdio: "pipe" });
  const write = (file, text) => {
    mkdirSync(dirname(join(dir, file)), { recursive: true });
    writeFileSync(join(dir, file), text);
  };
  const outcome = (labels = []) => {
    const out = join(tmpdir(), `wpg-out-${process.pid}-${Math.random()}`);
    writeFileSync(out, "");
    const env = { PATH: process.env.PATH, EVENT_NAME: "pull_request", PR_LABELS: JSON.stringify(labels), GITHUB_OUTPUT: out };
    execFileSync("node", [script], { cwd: dir, stdio: "pipe", env });
    const text = readFileSync(out, "utf8").trim();
    rmSync(out);
    return text;
  };
  // Builds the merge commit GitHub checks out for a pull request: main plus one branch, merged with --no-ff.
  const prMerge = (branch, change) => {
    vcs("switch", "-q", "--detach", "main");
    vcs("switch", "-q", "-c", branch);
    change();
    vcs("add", "-A");
    vcs("commit", "-qm", branch);
    vcs("switch", "-q", "--detach", "main");
    vcs("merge", "-q", "--no-ff", "-m", "merge", branch);
  };
  try {
    vcs("init", "-q", "-b", "main");
    write("package.json", '{"version":"1.0.0"}');
    write("src-tauri/tauri.conf.json", '{"version":"1.0.0"}');
    write("README.md", "a");
    vcs("add", "-A");
    vcs("commit", "-qm", "base");
    prMerge("docs", () => write("README.md", "b"));
    assert.equal(outcome(), "run=false");
    assert.equal(outcome([LABEL]), "run=true");
    prMerge("pkg", () => write("scripts/cmake/windows-arm64-native.cmake", "x"));
    assert.equal(outcome(), "run=true");
    prMerge("bump", () => write("src-tauri/tauri.conf.json", '{"version":"1.1.0"}'));
    assert.equal(outcome(), "run=true");
    prMerge("bump2", () => write("package.json", '{"version":"1.2.0"}'));
    assert.equal(outcome(), "run=true");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
