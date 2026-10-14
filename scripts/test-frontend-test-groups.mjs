import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, posix } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { isProcessAlive } from "./lib/process-tree.mjs";
import { GROUP_NPM_PACKAGES, TEST_GROUPS } from "./node-test-groups.mjs";
import { classificationProblems, discoverTestFiles } from "./run-frontend-tests.mjs";

// scripts/run-frontend-tests.mjs and the manifest scripts/node-test-groups.mjs
// (#350). Fixture repositories live in temporary directories; their tests
// only append their own name to a log.

const runner = fileURLToPath(new URL("./run-frontend-tests.mjs", import.meta.url));
const repoRoot = dirname(dirname(runner));
const isWindows = process.platform === "win32";

const passing = (name) => `
import { appendFileSync } from "node:fs";
import test from "node:test";
test(${JSON.stringify(name)}, () => appendFileSync(process.env.FIXTURE_LOG, ${JSON.stringify(`${name}\n`)}));
`;

function fixtureRepo(t, { groups, files, packages = {} }) {
  const root = mkdtempSync(join(tmpdir(), "sagascript-node-groups-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "scripts"), { recursive: true });
  writeFileSync(
    join(root, "scripts", "node-test-groups.mjs"),
    `export const TEST_GROUPS = ${JSON.stringify(groups)};\nexport const GROUP_NPM_PACKAGES = ${JSON.stringify(packages)};\n`,
  );
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), body);
  }
  return root;
}

function runTests(root, args = [], env = {}) {
  const log = join(root, "ran.log");
  return new Promise((resolveRun, reject) => {
    const started = Date.now();
    const child = spawn(process.execPath, [runner, "--root", root, ...args], {
      env: { ...process.env, FIXTURE_LOG: log, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", reject);
    child.once("close", (code) =>
      resolveRun({
        code,
        stdout,
        stderr,
        elapsedMs: Date.now() - started,
        ran: existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").sort() : [],
      }),
    );
  });
}

const threeGroups = {
  groups: {
    ui: ["scripts/test-ui.mjs"],
    ci: ["scripts/test-ci.mjs", "scripts/tools/test_tool.mjs"],
    codeHealth: ["scripts/test-health.mjs"],
  },
  files: {
    "scripts/test-ui.mjs": passing("ui"),
    "scripts/test-ci.mjs": passing("ci"),
    "scripts/tools/test_tool.mjs": passing("tool"),
    "scripts/test-health.mjs": passing("health"),
    "scripts/fixtures/test-not-a-suite.mjs": "throw new Error('fixtures are not discovered');",
    "scripts/helper.mjs": "export {};",
  },
};

test("the real manifest classifies every Node test file exactly once", () => {
  const discovered = discoverTestFiles(repoRoot);
  assert.deepEqual(classificationProblems(TEST_GROUPS, discovered, repoRoot), []);
  assert.deepEqual(Object.keys(TEST_GROUPS), ["ui", "ci", "release", "codeHealth"]);
  for (const [group, files] of Object.entries(TEST_GROUPS)) assert.ok(files.length > 0, `${group} is not empty`);
  // Runs in test:frontend now; the old ^test-.*\.mjs$ glob missed it.
  assert.ok(TEST_GROUPS.ci.includes("scripts/diarization_sv/test_reference_review.mjs"));
  assert.ok(discovered.includes("scripts/diarization_sv/test_reference_review.mjs"));
});

test("each group declares the npm packages its tests import", () => {
  // Static, side-effect and dynamic imports of a bare package specifier.
  const bareImport = /\bfrom\s*["']([@a-z][\w@./:-]*)["']|\bimport\s*\(?\s*["']([@a-z][\w@./:-]*)["']/g;
  const relativeImport = /\bfrom\s*["'](\.{1,2}\/[^"']+\.mjs)["']/g;
  // A test needs the packages of the repository modules it imports as well (scripts/lib/ci-workflow.mjs
  // reads the workflows with `yaml`, #358), so relative imports are followed.
  function collect(file, imported, seen) {
    if (seen.has(file)) return;
    seen.add(file);
    // Comments that mention imports (`import "name"`) are not imports.
    const source = readFileSync(join(repoRoot, file), "utf8").replace(/\/\*[\s\S]*?\*\/|^\s*\/\/.*$/gm, "");
    for (const match of source.matchAll(bareImport)) {
      const specifier = match[1] ?? match[2];
      if (specifier.startsWith("node:")) continue;
      imported.add(specifier.startsWith("@") ? specifier.split("/").slice(0, 2).join("/") : specifier.split("/")[0]);
    }
    for (const match of source.matchAll(relativeImport))
      collect(posix.normalize(posix.join(posix.dirname(file), match[1])), imported, seen);
  }
  for (const [group, files] of Object.entries(TEST_GROUPS)) {
    const imported = new Set();
    for (const file of files) collect(file, imported, new Set());
    assert.deepEqual([...imported].sort(), [...(GROUP_NPM_PACKAGES[group] ?? [])].sort(), `group ${group}`);
  }
});

test("npm run test:code-health runs exactly the codeHealth group from scripts/", () => {
  const script = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8")).scripts["test:code-health"];
  assert.deepEqual(script.match(/(?<![\w/])scripts\/test-[\w-]+\.mjs/g).sort(), [...TEST_GROUPS.codeHealth].sort());
});

test("package.json exposes the aggregate and one script per group", () => {
  const scripts = JSON.parse(readFileSync(join(repoRoot, "package.json"), "utf8")).scripts;
  assert.equal(scripts["test:frontend"], "node scripts/run-frontend-tests.mjs");
  assert.equal(scripts["test:ui"], "node scripts/run-frontend-tests.mjs --group ui");
  assert.equal(scripts["test:ci"], "node scripts/run-frontend-tests.mjs --group ci");
  assert.equal(scripts["test:release"], "node scripts/run-frontend-tests.mjs --group release");
});

test("without --group every group runs; --group selects, code-health names codeHealth", async (t) => {
  const root = fixtureRepo(t, threeGroups);

  const all = await runTests(root);
  assert.equal(all.code, 0, all.stdout + all.stderr);
  assert.deepEqual(all.ran, ["ci", "health", "tool", "ui"]);
  rmSync(join(root, "ran.log"));

  const ui = await runTests(root, ["--group", "ui"]);
  assert.equal(ui.code, 0, ui.stderr);
  assert.deepEqual(ui.ran, ["ui"]);
  assert.match(ui.stdout, /Node tests: ui \(1\)/);
  rmSync(join(root, "ran.log"));

  const several = await runTests(root, ["--group", "ci,code-health"]);
  assert.equal(several.code, 0, several.stderr);
  assert.deepEqual(several.ran, ["ci", "health", "tool"]);

  const unknown = await runTests(root, ["--group", "nope"]);
  assert.equal(unknown.code, 2);
  assert.match(unknown.stderr, /unknown group nope; known groups: ui, ci, code-health/);
});

test("--list prints the classification and runs nothing", async (t) => {
  const root = fixtureRepo(t, threeGroups);
  const result = await runTests(root, ["--list"]);
  assert.equal(result.code, 0, result.stderr);
  assert.match(result.stdout, /^ci \(2 files\)\n {2}scripts\/test-ci\.mjs\n {2}scripts\/tools\/test_tool\.mjs$/m);
  assert.match(result.stdout, /^code-health \(1 files\)$/m);
  assert.deepEqual(result.ran, []);
});

test("an unclassified test file is an error before anything runs", async (t) => {
  const root = fixtureRepo(t, {
    ...threeGroups,
    files: { ...threeGroups.files, "scripts/test-new.mjs": passing("new"), "scripts/tools/test_other.mjs": passing("other") },
  });
  const result = await runTests(root, ["--group", "ui"]);
  assert.equal(result.code, 2);
  assert.match(result.stderr, /scripts\/node-test-groups\.mjs does not match the test files on disk/);
  assert.match(
    result.stderr,
    /scripts\/test-new\.mjs is not classified; add it to one group in scripts\/node-test-groups\.mjs \(ui, ci, codeHealth\)/,
  );
  assert.match(result.stderr, /scripts\/tools\/test_other\.mjs is not classified/);
  assert.deepEqual(result.ran, []);
});

test("a missing or doubly listed manifest entry is an error", async (t) => {
  const root = fixtureRepo(t, {
    groups: { ui: ["scripts/test-ui.mjs", "scripts/test-gone.mjs"], ci: ["scripts/test-ui.mjs"] },
    files: { "scripts/test-ui.mjs": passing("ui") },
  });
  const result = await runTests(root);
  assert.equal(result.code, 2);
  assert.match(result.stderr, /scripts\/test-gone\.mjs \(group "ui"\) does not exist; remove it/);
  assert.match(result.stderr, /scripts\/test-ui\.mjs is listed in both "ui" and "ci"/);
  assert.deepEqual(result.ran, []);
});

test("a failing test file makes the run exit non-zero", async (t) => {
  const root = fixtureRepo(t, {
    groups: { ui: ["scripts/test-ok.mjs"], ci: ["scripts/test-bad.mjs"] },
    files: {
      "scripts/test-ok.mjs": passing("ok"),
      "scripts/test-bad.mjs": `import test from "node:test";\ntest("bad", () => { throw new Error("synthetic failure"); });\n`,
    },
  });
  assert.equal((await runTests(root, ["--group", "ui"])).code, 0);
  const failing = await runTests(root);
  assert.equal(failing.code, 1);
  assert.match(failing.stdout, /synthetic failure/);
});

test("missing npm packages are one preflight error naming npm ci, and nothing is installed", async (t) => {
  const root = fixtureRepo(t, {
    groups: { ui: ["scripts/test-ui.mjs"], ci: ["scripts/test-ci.mjs"] },
    packages: { ui: ["typescript", "@scope/absent-package"] },
    files: { "scripts/test-ui.mjs": passing("ui"), "scripts/test-ci.mjs": passing("ci") },
  });
  const installLog = join(root, "install-attempts.log");
  const shims = join(root, "bin");
  mkdirSync(shims);
  for (const name of ["npm", "npx", "pnpm", "yarn"]) {
    writeFileSync(join(shims, name), `#!/bin/sh\necho "${name} $*" >> "${installLog}"\nexit 1\n`);
    chmodSync(join(shims, name), 0o755);
  }
  // NODE_PATH must not hide the problem: ESM imports never read it.
  const env = { NODE_PATH: join(repoRoot, "node_modules"), ...(isWindows ? {} : { PATH: `${shims}${delimiter}${process.env.PATH}` }) };

  const result = await runTests(root, [], env);
  assert.equal(result.code, 2, result.stdout + result.stderr);
  assert.equal(
    result.stderr.trim(),
    `run-frontend-tests: missing npm packages typescript, @scope/absent-package (needed by group ui).\n` +
      `Run \`npm ci\` in ${root}, then run the tests again. Nothing was installed or run.`,
  );
  assert.deepEqual(result.ran, []);
  assert.ok(!existsSync(installLog), "no package manager was invoked");

  // Groups that need nothing still run without node_modules.
  const ci = await runTests(root, ["--group", "ci"], env);
  assert.equal(ci.code, 0, ci.stderr);
  assert.deepEqual(ci.ran, ["ci"]);
});

test("a hanging test is stopped at the budget together with its children", async (t) => {
  const root = fixtureRepo(t, {
    groups: { ci: ["scripts/test-hang.mjs"] },
    files: {
      "scripts/test-hang.mjs": `
import { spawn } from "node:child_process";
import { renameSync, writeFileSync } from "node:fs";
import test from "node:test";
test("hangs", async () => {
  const grandchild = spawn(process.execPath, ["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000);"], { stdio: "ignore" });
  writeFileSync(process.env.FIXTURE_LOG + ".pids.tmp", JSON.stringify([process.pid, grandchild.pid]));
  renameSync(process.env.FIXTURE_LOG + ".pids.tmp", process.env.FIXTURE_LOG + ".pids");
  setInterval(() => {}, 1000);
  await new Promise(() => {});
});
`,
    },
  });
  const result = await runTests(root, ["--timeout-ms", "1500"]);
  assert.equal(result.code, 1, result.stdout + result.stderr);
  assert.match(result.stderr, /timed out after 1\.5s; stopped the test process tree/);
  assert.ok(result.elapsedMs >= 1400, `waited for the budget (${result.elapsedMs} ms)`);
  assert.ok(result.elapsedMs < 15_000, `stopped promptly (${result.elapsedMs} ms)`);

  const pids = JSON.parse(readFileSync(join(root, "ran.log.pids"), "utf8"));
  const deadline = Date.now() + 5000;
  while (pids.some(isProcessAlive) && Date.now() < deadline) await new Promise((r) => setTimeout(r, 50));
  assert.deepEqual(pids.filter(isProcessAlive), [], "no test process survives");
});
