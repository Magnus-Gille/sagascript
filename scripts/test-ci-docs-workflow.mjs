import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { REPO_ROOT, loadWorkflow, parseWorkflow } from "./lib/ci-workflow.mjs";

// The docs-only scope gate of ci.yml and what the aggregators make of it. The workflow is parsed,
// and the scripts under test are the ones its steps run.

const workflow = loadWorkflow();
const CHECKS = ["check-macos", "check-windows", "check-linux"];

function stepRun(source, jobId, predicate) {
  const step = source.job(jobId).steps.find(predicate);
  assert.ok(step, `${jobId} has the step`);
  return step.run;
}
const scopeScript = (source = workflow) => stepRun(source, "scope", (step) => step.id === "classify");
const shell = (command, env, cwd) =>
  spawnSync("bash", ["-euo", "pipefail", "-c", command], { cwd, env: { ...process.env, ...env }, encoding: "utf8" });

// A check job's own step, run as the workflow runs it, against a needs context built from the
// job's own `needs`.
function runCheck(jobId, { docsOnly, trusted = "true", scope = "success", results }) {
  const job = workflow.job(jobId);
  const needs = { scope: { result: scope, outputs: { docs_only: docsOnly, scope_trusted: trusted, scope_classification: "x" } } };
  const lanes = job.needs.filter((need) => need !== "scope");
  lanes.forEach((lane, index) => (needs[lane] = { result: results[index] ?? results[0], outputs: {} }));
  const step = job.steps.find((candidate) => candidate.run.startsWith("node scripts/ci/assert-lanes.mjs"));
  assert.equal(job.env.NEEDS, "${{ toJSON(needs) }}", "the step reads the job's needs context");
  return shell(step.run, { NEEDS: JSON.stringify(needs) }, REPO_ROOT);
}

test("required checks accept only successful full lanes or explicitly classified skipped docs lanes", () => {
  // One process per case: the matrix of every result is in test-ci-parallel-gates.mjs, in-process.
  for (const id of CHECKS) {
    for (const docsOnly of ["true", "false"]) {
      for (const first of ["success", "failure", "cancelled", "skipped"]) {
        for (const second of ["success", "skipped"]) {
          const expected = docsOnly === "true" ? first === "skipped" && second === "skipped" : first === "success" && second === "success";
          assert.equal(
            runCheck(id, { docsOnly, results: [first, second] }).status === 0,
            expected,
            `${id}: docs_only ${docsOnly}, ${first}/${second}`,
          );
        }
      }
    }
    assert.notEqual(runCheck(id, { docsOnly: "", results: ["success"] }).status, 0, `${id} rejects an empty docs_only`);
    for (const scope of ["failure", "cancelled", "skipped"]) {
      assert.notEqual(runCheck(id, { docsOnly: "true", scope, results: ["skipped"] }).status, 0, `${id} rejects scope ${scope}`);
    }
    for (const trusted of ["false", ""]) {
      assert.notEqual(
        runCheck(id, { docsOnly: "true", trusted, results: ["skipped"] }).status,
        0,
        `${id} rejects scope_trusted ${trusted}`,
      );
    }
  }
});

test("scope bootstrap and unavailable commits select full CI; known docs and pushes select the intended mode", (t) => {
  const cwd = mkdtempSync(join(tmpdir(), "sagascript-docs-workflow-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const env = {
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_AUTHOR_NAME: "CI fixture",
    GIT_COMMITTER_NAME: "CI fixture",
    GIT_AUTHOR_EMAIL: "ci@example.invalid",
    GIT_COMMITTER_EMAIL: "ci@example.invalid",
  };
  const git = (...args) =>
    execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
      cwd,
      env: { ...process.env, ...env },
      encoding: "utf8",
    }).trim();
  git("init", "-q");
  writeFileSync(join(cwd, "README.md"), "before\n");
  git("add", ".");
  git("commit", "-qm", "base");
  const base = git("rev-parse", "HEAD");
  writeFileSync(join(cwd, "README.md"), "after\n");
  git("add", ".");
  git("commit", "-qm", "docs");
  const head = git("rev-parse", "HEAD");
  const output = join(cwd, "output");
  const classify = (overrides = {}) => {
    writeFileSync(output, "");
    const result = shell(
      scopeScript(),
      {
        ...env,
        EVENT_NAME: "pull_request",
        PR_BASE_SHA: base,
        PR_HEAD_SHA: head,
        GITHUB_OUTPUT: output,
        ...overrides,
      },
      cwd,
    );
    return { ...result, output: readFileSync(output, "utf8") };
  };
  let result = classify();
  assert.equal(result.status, 0);
  assert.match(result.output, /docs_only=false/); // Adoption: base has no policy yet.
  mkdirSync(join(cwd, "scripts"));
  copyFileSync(new URL("./ci-docs-scope.mjs", import.meta.url), join(cwd, "scripts/ci-docs-scope.mjs"));
  result = classify();
  assert.equal(result.status, 0);
  assert.match(result.output, /docs_only=true/);
  for (const overrides of [{ PR_HEAD_SHA: "f".repeat(40) }, { PR_HEAD_SHA: "bad; touch unsafe" }, { EVENT_NAME: "push" }]) {
    result = classify(overrides);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /docs_only=false/);
  }
  result = classify({ PR_HEAD_SHA: base }); // Empty comparison cannot pass docs gates.
  assert.notEqual(result.status, 0);
  assert.doesNotMatch(result.output, /docs_only=true/);
});

test("the scope job needs no script file of the revision it classifies", () => {
  // It checks out the PR base, which may predate scripts/ci/ (adoption): its own script must be inline.
  const scope = workflow.job("scope");
  assert.deepEqual(scope.needs, []);
  const checkout = scope.steps[0];
  assert.equal(checkout.with.ref, "${{ github.event_name == 'pull_request' && github.event.pull_request.base.sha || github.sha }}");
  for (const step of scope.steps) assert.ok(!step.run.includes("scripts/ci/"), "scope must not depend on files added by the PR");
});

test("CRLF workflow text still parses and executes scope and aggregate scripts", () => {
  const crlfWorkflow = parseWorkflow(
    readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8")
      .replace(/\r\n?/g, "\n")
      .replaceAll("\n", "\r\n"),
  );
  const scopeResult = shell(scopeScript(crlfWorkflow), { EVENT_NAME: "push", GITHUB_OUTPUT: "" });
  assert.equal(scopeResult.status, 0, scopeResult.stderr);
  assert.match(scopeResult.stdout, /push-event-requires-full-ci/);
  for (const id of CHECKS) {
    const result = runCheck(id, { docsOnly: "false", results: ["success"] });
    assert.equal(result.status, 0, `${id}: ${result.stderr}`);
    const step = crlfWorkflow.job(id).steps.find((candidate) => candidate.run.startsWith("node scripts/ci/assert-lanes.mjs"));
    assert.equal(step.run, "node scripts/ci/assert-lanes.mjs");
  }
});
