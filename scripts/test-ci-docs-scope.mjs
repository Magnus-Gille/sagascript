import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { classifyRecords, isAllowedPath, parseNameStatusZ } from "./ci-docs-scope.mjs";

const SCRIPT = fileURLToPath(new URL("./ci-docs-scope.mjs", import.meta.url));
function git(cwd, args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: {
      ...process.env,
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_AUTHOR_NAME: "CI scope test",
      GIT_AUTHOR_EMAIL: "ci-scope@example.invalid",
      GIT_COMMITTER_NAME: "CI scope test",
      GIT_COMMITTER_EMAIL: "ci-scope@example.invalid",
      GIT_COMMIT_GPGSIGN: "false",
    },
  }).trim();
}

async function fixture() {
  const cwd = await mkdtemp(join(tmpdir(), "sagascript-ci-docs-scope-"));
  await mkdir(join(cwd, "docs/research"), { recursive: true });
  await mkdir(join(cwd, "src"), { recursive: true });
  await writeFile(join(cwd, "docs/research/old.md"), "old\n");
  await writeFile(join(cwd, "docs/research/rename-out.md"), "rename\n");
  await writeFile(join(cwd, "docs/research/tool.json"), "{}\n");
  await writeFile(join(cwd, "docs/other.md"), "outside the allowlist\n");
  await writeFile(join(cwd, "src/main.rs"), "fn main() {}\n");
  git(cwd, ["init", "-q"]);
  git(cwd, ["config", "core.hooksPath", "/dev/null"]);
  git(cwd, ["config", "user.name", "CI scope test"]);
  git(cwd, ["config", "user.email", "ci-scope@example.invalid"]);
  git(cwd, ["add", "."]);
  git(cwd, ["commit", "-qm", "base"]);
  return cwd;
}

function commit(cwd, message) {
  git(cwd, ["add", "-A"]);
  git(cwd, ["commit", "-qm", message]);
  return git(cwd, ["rev-parse", "HEAD"]);
}

function runClassifier(cwd, args) {
  const outputPath = join(cwd, ".git", "classifier-output");
  writeFileSync(outputPath, "");
  const result = spawnSync(process.execPath, [SCRIPT, ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, GITHUB_OUTPUT: outputPath },
  });
  return {
    ...result,
    outputPath,
    githubOutput: existsSync(outputPath) ? readFileSync(outputPath, "utf8") : "",
    output: result.stdout ? JSON.parse(result.stdout.trim()) : undefined,
  };
}

test("allowlist is narrow and rejects tooling, traversal, and other source paths", () => {
  assert.equal(isAllowedPath("README.md"), true);
  assert.equal(isAllowedPath("docs/release-readiness.md"), true);
  assert.equal(isAllowedPath("docs/research/nested/notes.md"), true);
  assert.equal(isAllowedPath("docs/research/nested/notes.json"), false);
  assert.equal(isAllowedPath("docs/other.md"), false);
  assert.equal(isAllowedPath("docs/research/../release-readiness.md"), false);
  assert.equal(isAllowedPath("docs\\research\\notes.md"), false);
});

test("PR diff classification handles add, delete, rename, mixed, unknown, full, and empty cases", async (t) => {
  const cwd = await fixture();
  t.after(() => rm(cwd, { recursive: true, force: true }));

  const base = git(cwd, ["rev-parse", "HEAD"]);

  await writeFile(join(cwd, "README.md"), "new readme\n");
  let head = commit(cwd, "add allowlisted README");
  let result = runClassifier(cwd, ["--base", base, "--head", head]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, true);
  assert.deepEqual(result.output.changed_paths, ["README.md"]);
  assert.match(result.githubOutput, /docs_only=true/);
  assert.match(result.githubOutput, /scope_trusted=true/);

  const deleteBase = head;
  await rm(join(cwd, "docs/research/old.md"));
  head = commit(cwd, "delete allowlisted research note");
  result = runClassifier(cwd, ["--base", deleteBase, "--head", head]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, true);
  assert.deepEqual(result.output.changed_paths, ["docs/research/old.md"]);

  const renameBase = head;
  git(cwd, ["mv", "docs/research/rename-out.md", "docs/research/renamed.md"]);
  head = commit(cwd, "rename allowlisted research note");
  result = runClassifier(cwd, ["--base", renameBase, "--head", head]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, true);
  assert.deepEqual(result.output.changed_paths, ["docs/research/rename-out.md", "docs/research/renamed.md"]);

  const mixedBase = head;
  await writeFile(join(cwd, "README.md"), "mixed update\n");
  await writeFile(join(cwd, "src/changed.rs"), "fn changed() {}\n");
  head = commit(cwd, "mix docs and source");
  result = runClassifier(cwd, ["--base", mixedBase, "--head", head]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, false);
  assert.equal(result.output.scope_classification, "full-required");

  const unknownBase = head;
  await writeFile(join(cwd, "docs/research/tool.json"), '{"changed":true}\n');
  head = commit(cwd, "change research tooling data");
  result = runClassifier(cwd, ["--base", unknownBase, "--head", head]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, false);

  result = runClassifier(cwd, ["--full"]);
  assert.equal(result.status, 0);
  assert.equal(result.output.docs_only, false);
  assert.equal(result.output.reason, "push-event-requires-full-ci");

  result = runClassifier(cwd, ["--base", head, "--head", head]);
  assert.notEqual(result.status, 0, "an empty diff must fail closed");
  assert.equal(result.output, undefined);
});

test("invalid SHA input cannot reach a shell and fails before git", async (t) => {
  const cwd = await fixture();
  t.after(() => rm(cwd, { recursive: true, force: true }));
  const base = git(cwd, ["rev-parse", "HEAD"]);
  const sentinel = join(cwd, "shell-injection-marker");
  const result = runClassifier(cwd, ["--base", `${base}; touch ${sentinel}`, "--head", base]);
  assert.notEqual(result.status, 0);
  assert.equal(existsSync(sentinel), false);
});

test("malformed NUL output and empty records fail closed", () => {
  assert.throws(() => classifyRecords([]), /empty diff/);
  assert.equal(classifyRecords([{ status: "T", paths: ["README.md"] }]).docs_only, false);
  assert.throws(() => parseNameStatusZ(Buffer.from("M\0README.md")), /malformed/);
  assert.throws(() => parseNameStatusZ(Buffer.from("R100\0old.md\0")), /incomplete/);
});


test("renames across the allowlist and unusual filenames remain conservative", () => {
  for (const paths of [["src/app.md", "docs/research/app.md"], ["docs/research/app.md", "src/app.md"]]) {
    assert.equal(classifyRecords([{ status: "R100", paths }]).docs_only, false);
  }
  const record = parseNameStatusZ(Buffer.from("M\0docs/research/line\nwith spaces.md\0"));
  assert.deepEqual(record[0].paths, ["docs/research/line\nwith spaces.md"]);
  assert.equal(classifyRecords([{ status: "U", paths: ["README.md"] }]).docs_only, false);
  assert.equal(classifyRecords([{ status: "MALFORMED", paths: ["README.md"] }]).docs_only, false);
});
