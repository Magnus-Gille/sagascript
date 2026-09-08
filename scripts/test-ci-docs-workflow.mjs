import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

const workflow = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8");
function normalizeWorkflow(source) {
  return source.replace(/\r\n?/g, "\n");
}
function job(id, source = workflow) {
  const normalized = normalizeWorkflow(source);
  return normalized.split(`\n  ${id}:\n`)[1].split(/\n  [a-z][a-z0-9-]*:\n/)[0];
}
function script(id, source = workflow) {
  return job(id, source).split("        run: |\n")[1].split(/\n      - /)[0]
    .split("\n").map(line => line.replace(/^          /, "")).join("\n");
}
function shell(command, env, cwd) {
  return spawnSync("bash", ["-euo", "pipefail", "-c", command], {
    cwd, env: { ...process.env, ...env }, encoding: "utf8",
  });
}

test("required checks accept only successful full lanes or explicitly classified skipped docs lanes", () => {
  const results = ["success", "failure", "cancelled", "skipped", ""];
  for (const id of ["check-macos", "check-windows", "check-linux"]) {
    const run = script(id);
    assert.match(job(id), /if: \$\{\{ always\(\) \}\}/);
    for (const docsOnly of ["true", "false", ""]) {
      for (const first of results) {
        for (const second of id === "check-linux" ? [first] : results) {
          const env = {
            SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: docsOnly,
            TEST_RESULT: first, BUILD_RESULT: second, LINUX_RESULT: first,
          };
          const expected = docsOnly === "true"
            ? first === "skipped" && second === "skipped"
            : docsOnly === "false" && first === "success" && second === "success";
          assert.equal(shell(run, env).status === 0, expected, `${id}: ${JSON.stringify(env)}`);
        }
      }
    }
    for (const result of results.filter(value => value !== "success")) {
      assert.notEqual(shell(run, {
        SCOPE_RESULT: result, SCOPE_TRUSTED: "true", DOCS_ONLY: "true",
        TEST_RESULT: "skipped", BUILD_RESULT: "skipped", LINUX_RESULT: "skipped",
      }).status, 0, `${id} rejects unsuccessful scope`);
    }
    for (const trusted of ["false", ""]) {
      assert.notEqual(shell(run, {
        SCOPE_RESULT: "success", SCOPE_TRUSTED: trusted, DOCS_ONLY: "true",
        TEST_RESULT: "skipped", BUILD_RESULT: "skipped", LINUX_RESULT: "skipped",
      }).status, 0);
    }
  }
});

test("scope bootstrap and unavailable commits select full CI; known docs and pushes select the intended mode", t => {
  const cwd = mkdtempSync(join(tmpdir(), "sagascript-docs-workflow-"));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  const env = {
    GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_AUTHOR_NAME: "CI fixture", GIT_COMMITTER_NAME: "CI fixture",
    GIT_AUTHOR_EMAIL: "ci@example.invalid", GIT_COMMITTER_EMAIL: "ci@example.invalid",
  };
  const git = (...args) => execFileSync("git", ["-c", "core.hooksPath=/dev/null", ...args], {
    cwd, env: { ...process.env, ...env }, encoding: "utf8",
  }).trim();
  git("init", "-q");
  writeFileSync(join(cwd, "README.md"), "before\n");
  git("add", "."); git("commit", "-qm", "base");
  const base = git("rev-parse", "HEAD");
  writeFileSync(join(cwd, "README.md"), "after\n");
  git("add", "."); git("commit", "-qm", "docs");
  const head = git("rev-parse", "HEAD");
  const output = join(cwd, "output");
  const classify = (overrides = {}) => {
    writeFileSync(output, "");
    const result = shell(script("scope"), {
      ...env, EVENT_NAME: "pull_request", PR_BASE_SHA: base, PR_HEAD_SHA: head,
      GITHUB_OUTPUT: output, ...overrides,
    }, cwd);
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
  for (const overrides of [
    { PR_HEAD_SHA: "f".repeat(40) },
    { PR_HEAD_SHA: "bad; touch unsafe" },
    { EVENT_NAME: "push" },
  ]) {
    result = classify(overrides);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.output, /docs_only=false/);
  }
  result = classify({ PR_HEAD_SHA: base }); // Empty comparison cannot pass docs gates.
  assert.notEqual(result.status, 0);
  assert.doesNotMatch(result.output, /docs_only=true/);
});

test("CRLF workflow text still parses and executes scope and aggregate scripts", () => {
  const crlfWorkflow = normalizeWorkflow(workflow).replaceAll("\n", "\r\n");
  const scope = script("scope", crlfWorkflow);
  const aggregateEnvironment = {
    SCOPE_RESULT: "success", SCOPE_TRUSTED: "true", DOCS_ONLY: "false",
    TEST_RESULT: "success", BUILD_RESULT: "success", LINUX_RESULT: "success",
  };

  const scopeResult = shell(scope, { EVENT_NAME: "push", GITHUB_OUTPUT: "" });
  assert.equal(scopeResult.status, 0, scopeResult.stderr);
  assert.match(scopeResult.stdout, /push-event-requires-full-ci/);

  for (const id of ["check-macos", "check-linux", "check-windows"]) {
    const result = shell(script(id, crlfWorkflow), aggregateEnvironment);
    assert.equal(result.status, 0, `${id}: ${result.stderr}`);
  }
});
