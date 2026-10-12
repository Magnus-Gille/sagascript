import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";
import { isProcessAlive } from "./lib/process-tree.mjs";
import { CHECKS, PREREQUISITES, parseArgs, preflight, selectChecks, splitCommand } from "./verify.mjs";

// scripts/verify.mjs (#350), driven through SAGASCRIPT_VERIFY_CONFIG with
// synthetic checks in temporary directories. Nothing here runs cargo, npm or
// a real check.

const verifyScript = fileURLToPath(new URL("./verify.mjs", import.meta.url));
const verifyUrl = pathToFileURL(verifyScript).href;
const isWindows = process.platform === "win32";

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "sagascript-verify-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  return directory;
}

// A program that starts a grandchild, records both pids and never exits.
// With "ignore-term" the grandchild survives SIGTERM, so only the group
// SIGKILL after the grace period can stop it.
const HANG = `
import { spawn } from "node:child_process";
import { renameSync, writeFileSync } from "node:fs";
const [pidFile, mode] = process.argv.slice(2);
const body = (mode === "ignore-term" ? "process.on('SIGTERM', () => {});" : "") + "setInterval(() => {}, 1000);";
const grandchild = spawn(process.execPath, ["-e", body], { stdio: "ignore" });
writeFileSync(pidFile + ".tmp", JSON.stringify([process.pid, grandchild.pid]));
renameSync(pidFile + ".tmp", pidFile);
setInterval(() => {}, 1000);
`;

// Exits 0 at once but leaves a background process behind in its group.
const LEAVE_BEHIND = `
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore", detached: false });
child.unref();
writeFileSync(process.argv[2], JSON.stringify([child.pid]));
`;

function writeConfig(directory, body) {
  const path = join(directory, "verify.config.mjs");
  writeFileSync(path, body);
  return path;
}

function runVerify(args, { config, env = {}, onSpawn } = {}) {
  return new Promise((resolveRun, reject) => {
    const started = Date.now();
    const child = spawn(process.execPath, [verifyScript, ...args], {
      env: { ...process.env, ...env, ...(config ? { SAGASCRIPT_VERIFY_CONFIG: config } : {}) },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.once("error", reject);
    child.once("close", (code, signal) =>
      resolveRun({ code, signal, stdout, stderr, elapsedMs: Date.now() - started }),
    );
    onSpawn?.(child);
  });
}

async function waitFor(predicate, timeoutMs, what) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  assert.fail(`timed out waiting for ${what}`);
}

async function assertAllGone(pids) {
  await waitFor(() => pids.every((pid) => !isProcessAlive(pid)), 5000, `pids ${pids.join(", ")} to exit`);
}

function summaryRow(stdout, id) {
  return stdout.split("\n").find((line) => line.startsWith(`${id} `)) ?? "";
}

test("--list prints every check with its area, set, platforms and commands, and runs nothing", async () => {
  const result = await runVerify(["--list"]);
  assert.equal(result.code, 0, result.stderr);
  for (const check of CHECKS) {
    assert.match(result.stdout, new RegExp(`^${check.id} +${check.area} `, "m"), `${check.id} is listed`);
    for (const step of check.steps) assert.ok(result.stdout.includes(`$ ${step.run}`), `${check.id}: ${step.run}`);
  }
  assert.match(result.stdout, /^swift +swift +default +macOS /m);
  assert.match(result.stdout, /^browser +browser +opt-in /m);
  assert.match(result.stdout, /^clippy-app +clippy +fast /m);
  assert.match(result.stdout, /^test-workspace .*\n.*\n +needs: rust, linux-libs, frontend-dist$/m);
  assert.match(result.stdout, /^core-test .*\n.*\n +needs: rust$/m);
  assert.match(result.stdout, /^Areas \(for --only\/--skip\): .*\bcore-features\b/m);
  assert.doesNotMatch(result.stdout, /verify summary/);
});

test("selection: default, fast, --only by check or area, --skip and platform skips", () => {
  const ids = (selection) => selection.selected.map(({ check }) => check.id);
  const skipped = (selection) =>
    Object.fromEntries(selection.selected.filter((entry) => entry.skipReason).map((entry) => [entry.check.id, entry.skipReason]));

  const linuxDefault = selectChecks(CHECKS, parseArgs([]), "linux");
  assert.ok(ids(linuxDefault).includes("test-workspace"));
  assert.ok(!ids(linuxDefault).includes("clippy-app"), "the workspace clippy covers the app in the full run");
  assert.deepEqual(
    linuxDefault.optIn.map((check) => check.id),
    CHECKS.filter((check) => check.sets.length === 0).map((check) => check.id),
  );
  assert.equal(skipped(linuxDefault).swift, "macOS only");
  assert.equal(skipped(selectChecks(CHECKS, parseArgs([]), "darwin")).swift, undefined);

  assert.deepEqual(ids(selectChecks(CHECKS, parseArgs(["--fast"]), "linux")), [
    "fmt",
    "fmt-ort",
    "svelte-check",
    "node-ui",
    "clippy-app",
  ]);
  assert.deepEqual(ids(selectChecks(CHECKS, parseArgs(["--only", "ort-host,fmt"]), "linux")), [
    "fmt",
    "fmt-ort",
    "ort-host-clippy",
    "ort-host-test",
  ]);
  assert.deepEqual(ids(selectChecks(CHECKS, parseArgs(["--only=browser"]), "linux")), ["browser"]);
  assert.equal(skipped(selectChecks(CHECKS, parseArgs(["--skip", "python"]), "linux"))["python-dictation-eval"], "--skip");

  assert.throws(() => selectChecks(CHECKS, parseArgs(["--only", "nope"]), "linux"), /unknown check or area nope/);
  assert.throws(() => parseArgs(["--fast", "--only", "fmt"]), /cannot be combined/);
  assert.throws(() => parseArgs(["--frobnicate"]), /unknown argument/);
});

test("splitCommand keeps quoted arguments together", () => {
  assert.deepEqual(splitCommand("python -m unittest discover -s scripts/x -p 'test_*.py'"), [
    "python",
    "-m",
    "unittest",
    "discover",
    "-s",
    "scripts/x",
    "-p",
    "test_*.py",
  ]);
});

test("a usage error exits 2 with the known checks and areas", async () => {
  const result = await runVerify(["--only", "does-not-exist"]);
  assert.equal(result.code, 2);
  assert.match(result.stderr, /unknown check or area does-not-exist/);
  assert.match(result.stderr, /Areas: .*ort-host/);
});

test("preflight reports every missing prerequisite in one error and installs nothing", async (t) => {
  const directory = fixture(t);
  const root = join(directory, "repo");
  mkdirSync(root);
  writeFileSync(join(root, "package.json"), "{}");
  const marker = join(directory, "check-ran");
  const installLog = join(directory, "install-attempts.log");

  // Installers on PATH that would record any call (POSIX shims).
  const shims = join(directory, "bin");
  mkdirSync(shims);
  for (const name of ["npm", "npx", "pip", "pip3", "rustup", "brew", "apt-get"]) {
    const shim = join(shims, name);
    writeFileSync(shim, `#!/bin/sh\necho "${name} $*" >> "${installLog}"\nexit 1\n`);
    chmodSync(shim, 0o755);
  }

  const config = writeConfig(
    directory,
    `
import { CHECKS } from ${JSON.stringify(verifyUrl)};
export const root = ${JSON.stringify(root)};
export const prerequisites = {
  "fake-tool": { label: "fake tool", probe: () => ({ ok: false, missing: "fake-tool 9 (synthetic)", fix: "brew install fake-tool" }) },
  "fake-ok": { label: "present tool", probe: () => ({ ok: true }) },
  // Present here so that only the repository state (node_modules, dist/) is missing.
  rust: { label: "Rust", probe: () => ({ ok: true }) },
  "linux-libs": { label: "Linux libraries", probe: () => ({ ok: true }) },
};
export const checks = [
  ...CHECKS.filter((check) => ["svelte-check", "node-ui", "clippy-workspace", "core-test"].includes(check.id)),
  { id: "synthetic", requires: ["fake-tool", "fake-ok"], steps: [{ run: "touch marker", argv: ["node", "-e", ${JSON.stringify(`require("fs").writeFileSync(${JSON.stringify(marker)}, "")`)}] }] },
];
`,
  );
  const result = await runVerify([], {
    config,
    env: isWindows ? {} : { PATH: `${shims}${delimiter}${process.env.PATH}` },
  });

  assert.equal(result.code, 2, result.stdout + result.stderr);
  assert.equal(result.stderr.match(/preflight failed/g)?.length, 1, "exactly one preflight error");
  assert.match(result.stderr, /3 prerequisites are missing/);
  assert.match(result.stderr, /npm packages svelte-check, svelte, typescript \(node_modules\)\n\s+needed by: svelte-check, node-ui\n\s+fix: +npm ci/);
  assert.match(result.stderr, /dist\/index\.html[^\n]*\n\s+needed by: clippy-workspace\n\s+fix: +npm run build/);
  assert.match(result.stderr, /fake-tool 9 \(synthetic\)\n\s+needed by: synthetic\n\s+fix: +brew install fake-tool/);
  assert.match(result.stderr, /Nothing was installed and no check ran/);
  assert.match(result.stderr, /--skip svelte-check,node-ui,clippy-workspace,synthetic/);
  assert.doesNotMatch(result.stdout, /==>/, "no check started");
  assert.ok(!existsSync(marker), "no check ran");
  assert.ok(!existsSync(installLog), `no installer was invoked: ${existsSync(installLog) ? readFileSync(installLog, "utf8") : ""}`);
});

test("dist/ is a prerequisite of exactly the checks that compile the app crate", async (t) => {
  const root = fixture(t);
  const present = { label: "present", probe: () => ({ ok: true }) };
  const prerequisites = { ...PREREQUISITES };
  for (const id of ["rust", "linux-libs", "msrv-toolchain", "python", "python-eval-packages", "swift", "playwright-chromium"]) {
    prerequisites[id] = present;
  }
  const cargoChecks = CHECKS.filter((check) => check.steps.some((step) => step.run.startsWith("cargo ")));
  const options = { root, platform: "linux", prerequisites };

  const before = await preflight(cargoChecks, { ...options, values: {} });
  assert.equal(before.missing.length, 1, JSON.stringify(before.missing));
  assert.match(before.missing[0].missing, /dist\/index\.html/);
  assert.equal(before.missing[0].fix, "npm run build");
  assert.deepEqual(before.missing[0].neededBy, ["clippy-workspace", "clippy-app", "test-workspace", "msrv"]);

  mkdirSync(join(root, "dist"));
  writeFileSync(join(root, "dist", "index.html"), "<title>Sagascript Settings</title>\n");
  const after = await preflight(cargoChecks, { ...options, values: {} });
  assert.deepEqual(after.missing, []);
});

test("exit status: stops at the first failure, --keep-going runs the rest, failures exit 1", async (t) => {
  const directory = fixture(t);
  const config = writeConfig(
    directory,
    `
export const root = ${JSON.stringify(directory)};
const exit = (code) => [{ run: "exit " + code, argv: ["node", "-e", "process.exit(" + code + ")"] }];
export const checks = [
  { id: "passes", steps: exit(0) },
  { id: "fails", steps: exit(3) },
  { id: "after", steps: exit(0) },
  { id: "elsewhere", platforms: [${JSON.stringify(process.platform === "darwin" ? "linux" : "darwin")}], steps: exit(9) },
  { id: "opt-in", sets: [], steps: exit(9) },
];
`,
  );

  const failFast = await runVerify([], { config });
  assert.equal(failFast.code, 1, failFast.stdout + failFast.stderr);
  assert.match(summaryRow(failFast.stdout, "passes"), /^passes +pass +\d/);
  assert.match(summaryRow(failFast.stdout, "fails"), /^fails +FAIL +\S+ +exit code 3$/);
  assert.match(summaryRow(failFast.stdout, "after"), /not run +- +not run: stopped after fails failed \(--keep-going runs the rest\)/);
  assert.match(summaryRow(failFast.stdout, "elsewhere"), /skip +- +(macOS|Linux) only/);
  assert.match(failFast.stdout, /Opt-in, not in this run: opt-in \(run with --only <id>\)/);

  const keepGoing = await runVerify(["--keep-going"], { config });
  assert.equal(keepGoing.code, 1);
  assert.match(summaryRow(keepGoing.stdout, "after"), /^after +pass /);
  assert.match(keepGoing.stdout, /^2 pass, 1 fail, 1 skip$/m);

  const green = await runVerify(["--skip", "fails"], { config });
  assert.equal(green.code, 0, green.stdout);
  assert.match(summaryRow(green.stdout, "fails"), /skip +- +--skip/);
  assert.match(green.stdout, /^2 pass, 2 skip$/m);

  const optIn = await runVerify(["--only", "opt-in"], { config });
  assert.equal(optIn.code, 1);
  assert.match(summaryRow(optIn.stdout, "opt-in"), /FAIL .*exit code 9/);
});

test("forbidOutput and requireOutput turn a zero exit status into a failure", async (t) => {
  const directory = fixture(t);
  const config = writeConfig(
    directory,
    `
export const root = ${JSON.stringify(directory)};
const print = (text) => ["node", "-e", "console.log(" + JSON.stringify(text) + ")"];
export const checks = [
  { id: "tree", steps: [{ run: "print tree", argv: print("ok v1\\nalsa-sys v0.3"), forbidOutput: { pattern: /alsa|cpal/i, message: "cpal/ALSA leaked" } }] },
  { id: "guard-ran", steps: [{ run: "print", argv: print("test guard ... ok"), requireOutput: { pattern: /guard \\.\\.\\. ok/, message: "guard did not run" } }] },
  { id: "guard-missing", steps: [{ run: "print", argv: print("running 0 tests"), requireOutput: { pattern: /guard \\.\\.\\. ok/, message: "guard did not run" } }] },
];
`,
  );
  const result = await runVerify(["--keep-going"], { config });
  assert.equal(result.code, 1);
  assert.match(result.stderr, /cpal\/ALSA leaked:\nalsa-sys v0\.3/);
  assert.match(summaryRow(result.stdout, "tree"), /FAIL .*cpal\/ALSA leaked$/);
  assert.match(summaryRow(result.stdout, "guard-ran"), /^guard-ran +pass /);
  assert.match(summaryRow(result.stdout, "guard-missing"), /FAIL .*guard did not run$/);
  assert.match(result.stdout, /^running 0 tests$/m, "the captured output is still shown");
});

test("a hanging check is stopped at its budget and leaves no process behind", async (t) => {
  const directory = fixture(t);
  const hang = join(directory, "hang.mjs");
  writeFileSync(hang, HANG);
  const pidFile = join(directory, "pids.json");
  const config = writeConfig(
    directory,
    `
export const root = ${JSON.stringify(directory)};
export const killGraceMs = 500;
export const checks = [
  { id: "hangs", timeoutMs: 1500, steps: [{ run: "node hang.mjs", argv: ["node", ${JSON.stringify(hang)}, ${JSON.stringify(pidFile)}, "ignore-term"] }] },
  { id: "next", steps: [{ run: "true", argv: ["node", "-e", ""] }] },
];
`,
  );

  const result = await runVerify(["--keep-going"], { config });
  assert.equal(result.code, 1, result.stdout + result.stderr);
  assert.match(summaryRow(result.stdout, "hangs"), /^hangs +FAIL +\S+ +timed out after 1\.5s; process tree stopped$/);
  assert.match(summaryRow(result.stdout, "next"), /^next +pass /);
  assert.ok(result.elapsedMs >= 1400, `waited for the budget (${result.elapsedMs} ms)`);
  assert.ok(result.elapsedMs < 15_000, `stopped promptly (${result.elapsedMs} ms)`);
  const pids = JSON.parse(readFileSync(pidFile, "utf8"));
  assert.equal(pids.length, 2);
  await assertAllGone(pids);
});

test("SIGTERM stops the running check's process tree and reports the interruption", { skip: isWindows && "POSIX signals" }, async (t) => {
  const directory = fixture(t);
  const hang = join(directory, "hang.mjs");
  writeFileSync(hang, HANG);
  const pidFile = join(directory, "pids.json");
  const config = writeConfig(
    directory,
    `
export const root = ${JSON.stringify(directory)};
export const killGraceMs = 500;
export const checks = [
  { id: "hangs", timeoutMinutes: 10, steps: [{ run: "node hang.mjs", argv: ["node", ${JSON.stringify(hang)}, ${JSON.stringify(pidFile)}] }] },
  { id: "never", steps: [{ run: "true", argv: ["node", "-e", ""] }] },
];
`,
  );

  let verify;
  const run = runVerify([], { config, onSpawn: (child) => (verify = child) });
  await waitFor(() => existsSync(pidFile), 10_000, "the hanging check to start");
  const pids = JSON.parse(readFileSync(pidFile, "utf8"));
  verify.kill("SIGTERM");
  const result = await run;

  assert.equal(result.code, 143, result.stdout + result.stderr);
  assert.match(summaryRow(result.stdout, "hangs"), /INTERRUPTED .*interrupted \(SIGTERM\); process tree stopped/);
  assert.match(summaryRow(result.stdout, "never"), /not run +- +interrupted by SIGTERM/);
  await assertAllGone(pids);
});

test("processes a passing check leaves running are stopped and noted", { skip: isWindows && "POSIX process groups" }, async (t) => {
  const directory = fixture(t);
  const script = join(directory, "leave.mjs");
  writeFileSync(script, LEAVE_BEHIND);
  const pidFile = join(directory, "pids.json");
  const config = writeConfig(
    directory,
    `
export const root = ${JSON.stringify(directory)};
export const checks = [{ id: "leaky", steps: [{ run: "node leave.mjs", argv: ["node", ${JSON.stringify(script)}, ${JSON.stringify(pidFile)}] }] }];
`,
  );
  const result = await runVerify([], { config });
  assert.equal(result.code, 0, result.stdout + result.stderr);
  assert.match(summaryRow(result.stdout, "leaky"), /pass .*stopped processes it left running/);
  await assertAllGone(JSON.parse(readFileSync(pidFile, "utf8")));
});
