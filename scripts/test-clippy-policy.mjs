// Clippy policy baseline (#356): the comparison logic on synthetic cargo
// JSON (no cargo run), plus the repository wiring that keeps it enforced.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  POLICY_LINTS,
  RUNS,
  clippyArgs,
  compareCounts,
  countFindings,
  packageName,
  parseCargoMessages,
  planUpdate,
  readBaseline,
  unmeasuredCrates,
} from "./clippy-policy.mjs";

const script = fileURLToPath(new URL("./clippy-policy.mjs", import.meta.url));
const read = (path) => readFileSync(fileURLToPath(new URL(`../${path}`, import.meta.url)), "utf8").replace(/\r\n?/g, "\n");

// ---------------------------------------------------------------- fixtures

const ROOT = "path+file:///work/src-tauri";
const PACKAGES = {
  app: `${ROOT}#sagascript@1.0.0`,
  core: `${ROOT}/crates/sagascript-core#1.0.0`,
  cli: `${ROOT}/crates/sagascript-cli#1.0.0`,
  protocol: `${ROOT}/crates/sagascript-engine-protocol#1.0.0`,
  serde: "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228",
};

// The package directory and a target's root source file, as cargo reports them.
function targetFields(packageId, kind, source) {
  const directory = /^path\+file:\/\/([^#]+)#/.exec(packageId)?.[1] ?? "/registry/crate";
  const root = source ?? (kind === "bin" ? "src/main.rs" : kind === "custom-build" ? "build.rs" : "src/lib.rs");
  return { manifest_path: `${directory}/Cargo.toml`, target: { kind: [kind], src_path: `${directory}/${root}` } };
}

function artifact(packageId, kind = "lib", source) {
  return JSON.stringify({ reason: "compiler-artifact", package_id: packageId, ...targetFields(packageId, kind, source) });
}

function finding(packageId, lint, file, line, { kind = "lib", column = 9, level = "warning", source } = {}) {
  return JSON.stringify({
    reason: "compiler-message",
    package_id: packageId,
    ...targetFields(packageId, kind, source),
    message: {
      level,
      code: { code: lint.includes("::") ? lint : `clippy::${lint}` },
      message: `${lint} finding`,
      rendered: `${level}: ${lint}\n  --> ${file}:${line}:${column}\n`,
      spans: [
        { file_name: "src/macro_site.rs", line_start: 1, column_start: 1, line_end: 1, column_end: 2, is_primary: false },
        { file_name: file, line_start: line, column_start: column, line_end: line, column_end: column + 8, is_primary: true },
      ],
    },
  });
}

const jsonl = (...lines) => `${lines.join("\n")}\n`;

// The core crate with two unwraps and one expect; the cli with one print.
const BASE_RUN = jsonl(
  "   Compiling sagascript-core v1.0.0 (not JSON)",
  artifact(PACKAGES.core),
  artifact(PACKAGES.cli),
  artifact(PACKAGES.cli, "bin"),
  artifact(PACKAGES.protocol),
  finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 10),
  finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 20),
  finding(PACKAGES.core, "expect_used", "crates/sagascript-core/src/b.rs", 5),
  finding(PACKAGES.cli, "print_stdout", "crates/sagascript-cli/src/main.rs", 3, { kind: "bin" }),
);

function tempDir() {
  return mkdtempSync(join(tmpdir(), "clippy-policy-"));
}

function runScript(args, { summary = "" } = {}) {
  const result = spawnSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    env: { ...process.env, GITHUB_STEP_SUMMARY: summary },
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

// Writes the inputs and a baseline measured from `baselineRun`.
function workspace(baselineRun, ...runs) {
  const dir = tempDir();
  const baseline = join(dir, "baseline.json");
  const { findings, crates } = parseCargoMessages(baselineRun);
  writeFileSync(baseline, planUpdate({}, countFindings(findings, crates), { allowIncrease: true }).text);
  const inputs = runs.map((run, index) => {
    const path = join(dir, `run-${index}.jsonl`);
    writeFileSync(path, run);
    return path;
  });
  return { dir, baseline, inputArgs: inputs.flatMap((path) => ["--input", path]) };
}

// ------------------------------------------------------------------ parsing

test("package names come from both cargo package-id formats", () => {
  assert.equal(packageName(PACKAGES.app), "sagascript");
  assert.equal(packageName(PACKAGES.core), "sagascript-core");
  assert.equal(packageName("sagascript-cli 1.4.4 (path+file:///work/src-tauri/crates/sagascript-cli)"), "sagascript-cli");
  assert.equal(packageName("path+file:///C:/work/src-tauri/crates/my%20crate#0.1.0"), "my crate");
});

test("only policy lints in production targets of workspace crates are counted", () => {
  const { findings, crates } = parseCargoMessages(
    jsonl(
      artifact(PACKAGES.core),
      artifact(PACKAGES.serde),
      artifact(PACKAGES.app, "custom-build"),
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 1),
      finding(PACKAGES.core, "clippy::needless_return", "crates/sagascript-core/src/a.rs", 2),
      finding(PACKAGES.core, "unused_variables", "crates/sagascript-core/src/a.rs", 3),
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/tests/it.rs", 4, { kind: "test" }),
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/examples/e.rs", 5, { kind: "example" }),
      finding(PACKAGES.app, "panic", "build.rs", 6, { kind: "custom-build" }),
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/tests/support/fake.rs", 6, {
        kind: "bin",
        source: "tests/support/fake.rs",
      }),
      finding(PACKAGES.serde, "unwrap_used", "/registry/serde/src/lib.rs", 7),
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 8, { level: "note" }),
      finding(PACKAGES.app, "expect_used", "src/main.rs", 9, { kind: "bin", level: "error" }),
      "{not json",
    ),
  );
  assert.deepEqual(
    findings.map(({ crate, lint, file, line }) => `${crate} ${lint} ${file}:${line}`),
    ["sagascript-core unwrap_used crates/sagascript-core/src/a.rs:1", "sagascript expect_used src/main.rs:9"],
  );
  assert.deepEqual([...crates].sort(), ["sagascript", "sagascript-core"]);
});

test("a span reported by several targets or runs counts once", () => {
  const span = finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 10);
  const { findings, crates } = parseCargoMessages(
    jsonl(
      artifact(PACKAGES.core),
      span,
      span, // e.g. the lib and the lean CLI's build of the same lib
      finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 10, { column: 30 }),
      finding(PACKAGES.core, "expect_used", "crates/sagascript-core/src/a.rs", 10),
    ),
  );
  const counts = countFindings(findings, crates);
  assert.equal(counts.unwrap_used["sagascript-core"], 2, "same line, different column is a second finding");
  assert.equal(counts.expect_used["sagascript-core"], 1, "the same span under another lint counts for that lint");
});

test("counts list every policy lint and every compiled crate, zeros included", () => {
  const { findings, crates } = parseCargoMessages(BASE_RUN);
  const counts = countFindings(findings, crates);
  assert.deepEqual(Object.keys(counts), POLICY_LINTS);
  assert.deepEqual(counts.unwrap_used, { "sagascript-cli": 0, "sagascript-core": 2, "sagascript-engine-protocol": 0 });
  assert.deepEqual(counts.dbg_macro, { "sagascript-cli": 0, "sagascript-core": 0, "sagascript-engine-protocol": 0 });
});

// --------------------------------------------------------------- comparison

test("an increase, a new crate and a new lint are all reported as increases", () => {
  const baseline = { unwrap_used: { "sagascript-core": 2 } };
  const current = {
    unwrap_used: { "sagascript-core": 3, "sagascript-cli": 1 },
    dbg_macro: { "sagascript-core": 1 },
  };
  const { increased, decreased } = compareCounts(baseline, current);
  assert.deepEqual(
    increased.map(({ lint, crate, baseline: before, now }) => `${lint}/${crate} ${before}->${now}`).sort(),
    ["dbg_macro/sagascript-core 0->1", "unwrap_used/sagascript-cli 0->1", "unwrap_used/sagascript-core 2->3"],
  );
  assert.deepEqual(decreased, []);
});

test("a decrease is not an increase and reports cells that reached zero", () => {
  const { increased, decreased, reachedZero } = compareCounts(
    { unwrap_used: { a: 2, b: 1 }, panic: { a: 1 } },
    { unwrap_used: { a: 1, b: 0 }, panic: { a: 1 } },
  );
  assert.deepEqual(increased, []);
  assert.deepEqual(decreased.map((row) => `${row.lint}/${row.crate}`), ["unwrap_used/a", "unwrap_used/b"]);
  assert.deepEqual(reachedZero.map((row) => `${row.lint}/${row.crate}`), ["unwrap_used/b"]);
});

test("--update refuses to raise a count unless explicitly allowed", () => {
  const baseline = { unwrap_used: { a: 1 }, expect_used: { a: 5 } };
  const current = { unwrap_used: { a: 2 }, expect_used: { a: 4 } };
  const refused = planUpdate(baseline, current);
  assert.equal(refused.ok, false);
  assert.deepEqual(refused.increased.map((row) => `${row.lint}/${row.crate}`), ["unwrap_used/a"]);

  const allowed = planUpdate(baseline, current, { allowIncrease: true });
  assert.equal(allowed.ok, true);
  const written = JSON.parse(allowed.text).counts;
  assert.equal(written.unwrap_used.a, 2);
  assert.equal(written.expect_used.a, 4);
});

test("a baseline crate missing from the measurement is reported, not read as zero", () => {
  const baseline = { unwrap_used: { kept: 1, gone: 3 }, panic: { kept: 0 } };
  assert.deepEqual(unmeasuredCrates(baseline, new Set(["kept"])), ["gone"]);
  assert.deepEqual(unmeasuredCrates(baseline, new Set(["kept", "gone", "new"])), []);
});

// ---------------------------------------------------------------------- CLI

test("CLI: an increase fails and points at the current locations", () => {
  const more = BASE_RUN + finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/c.rs", 42) + "\n";
  const { baseline, inputArgs } = workspace(BASE_RUN, more);
  const result = runScript([...inputArgs, "--baseline", baseline]);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /^unwrap_used\s+sagascript-core\s+2\s+3\s+\+1$/m);
  assert.match(result.stderr, /unwrap_used in sagascript-core: 2 -> 3 \(\+1\)/);
  assert.match(result.stderr, /src-tauri\/crates\/sagascript-core\/src\/c\.rs:42:9/);
});

test("CLI: a lint or crate missing from the baseline counts from zero", () => {
  const extra = jsonl(BASE_RUN, artifact(PACKAGES.app, "bin"), finding(PACKAGES.app, "dbg_macro", "src/main.rs", 7, { kind: "bin" }));
  const { baseline, inputArgs } = workspace(BASE_RUN, extra);
  const result = runScript([...inputArgs, "--baseline", baseline]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /dbg_macro in sagascript: 0 -> 1 \(\+1\)/);
});

test("CLI: findings split over two runs are merged and deduplicated", () => {
  const lean = jsonl(
    artifact(PACKAGES.core),
    finding(PACKAGES.core, "unwrap_used", "crates/sagascript-core/src/a.rs", 10), // also in BASE_RUN
  );
  const { baseline, inputArgs } = workspace(BASE_RUN, BASE_RUN, lean);
  const result = runScript([...inputArgs, "--baseline", baseline]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /No policy count went up/);
});

test("CLI: a decrease passes and suggests --update", () => {
  const fewer = BASE_RUN.replace(/^.*"line_start":20.*\n/m, "");
  const { baseline, inputArgs } = workspace(BASE_RUN, fewer);
  const result = runScript([...inputArgs, "--baseline", baseline]);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^unwrap_used\s+sagascript-core\s+2\s+1\s+-1$/m);
  assert.match(result.stdout, /node scripts\/clippy-policy\.mjs --update/);
});

test("CLI: --update lowers the baseline but refuses an increase without --allow-increase", () => {
  const fewer = BASE_RUN.replace(/^.*"line_start":20.*\n/m, "");
  const more = BASE_RUN + finding(PACKAGES.cli, "panic", "crates/sagascript-cli/src/lib.rs", 1) + "\n";
  const { baseline, inputArgs } = workspace(BASE_RUN, fewer, more);
  const [fewerArgs, moreArgs] = [inputArgs.slice(0, 2), inputArgs.slice(2)];
  const before = readFileSync(baseline, "utf8");

  const refused = runScript([...moreArgs, "--baseline", baseline, "--update"]);
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /Refusing to raise the baseline without --allow-increase/);
  assert.match(refused.stderr, /panic in sagascript-cli: 0 -> 1/);
  assert.equal(readFileSync(baseline, "utf8"), before, "a refused update must not touch the file");

  const lowered = runScript([...fewerArgs, "--baseline", baseline, "--update"]);
  assert.equal(lowered.status, 0, lowered.stderr);
  assert.equal(readBaseline(baseline).unwrap_used["sagascript-core"], 1);

  const raised = runScript([...moreArgs, "--baseline", baseline, "--update", "--allow-increase"]);
  assert.equal(raised.status, 0, raised.stderr);
  assert.equal(readBaseline(baseline).panic["sagascript-cli"], 1);
  assert.equal(readBaseline(baseline).unwrap_used["sagascript-core"], 2);
});

test("CLI: usage errors and an empty measurement fail loudly", () => {
  const { dir, baseline, inputArgs } = workspace(BASE_RUN, BASE_RUN);
  assert.equal(runScript([...inputArgs, "--baseline", baseline, "--allow-increase"]).status, 2);
  assert.equal(runScript(["--bogus"]).status, 2);
  const empty = join(dir, "empty.jsonl");
  writeFileSync(empty, "");
  const result = runScript(["--input", empty, "--baseline", baseline]);
  assert.equal(result.status, 2);
  assert.match(result.stderr, /no workspace crate was compiled/);
  assert.equal(runScript([...inputArgs, "--baseline", join(dir, "missing.json")]).status, 2);

  // A run that did not compile a baseline crate must not pass or lower it.
  const partial = join(dir, "partial.jsonl");
  writeFileSync(partial, BASE_RUN.split("\n").filter((line) => !line.includes("sagascript-cli")).join("\n"));
  for (const extra of [[], ["--update"]]) {
    const partialRun = runScript(["--input", partial, "--baseline", baseline, ...extra]);
    assert.equal(partialRun.status, 2);
    assert.match(partialRun.stderr, /did not compile: sagascript-cli/);
  }
});

test("CLI: CI gets a per-lint, per-crate summary table", () => {
  const { dir, baseline, inputArgs } = workspace(BASE_RUN, BASE_RUN);
  const summary = join(dir, "summary.md");
  writeFileSync(summary, "");
  assert.equal(runScript([...inputArgs, "--baseline", baseline], { summary }).status, 0);
  const table = readFileSync(summary, "utf8");
  assert.match(table, /^\| lint \| sagascript-cli \| sagascript-core \| sagascript-engine-protocol \|$/m);
  assert.match(table, /^\| `unwrap_used` \| 0 \| 2 \| 0 \|$/m);
});

// ------------------------------------------------------------------- wiring

test("the policy runs measure production code in both CLI feature sets", () => {
  for (const run of RUNS) {
    const args = clippyArgs(run);
    assert.ok(!args.includes("--all-targets"), "test builds of a lib are indistinguishable in cargo's JSON");
    const lintArgs = args.slice(args.indexOf("--") + 1);
    assert.deepEqual(lintArgs.slice(0, 2), ["-A", "clippy::all"]);
    assert.deepEqual(lintArgs.slice(2), POLICY_LINTS.flatMap((lint) => ["-W", `clippy::${lint}`]));
  }
  assert.ok(RUNS.some((run) => run.args.includes("--workspace")));
  assert.ok(RUNS.some((run) => run.args.join(" ") === "-p sagascript-cli --no-default-features"));
});

test("the checked-in baseline covers exactly the policy lints", () => {
  const baseline = readBaseline(fileURLToPath(new URL("../src-tauri/clippy-policy-baseline.json", import.meta.url)));
  assert.deepEqual(Object.keys(baseline), POLICY_LINTS);
  for (const crate of ["sagascript", "sagascript-cli", "sagascript-core", "sagascript-engine-protocol"]) {
    for (const lint of POLICY_LINTS) assert.ok(Number.isInteger(baseline[lint][crate]), `${lint}/${crate}`);
  }
});

test("engine-protocol is at zero and denies every policy lint", () => {
  const baseline = readBaseline(fileURLToPath(new URL("../src-tauri/clippy-policy-baseline.json", import.meta.url)));
  for (const lint of POLICY_LINTS) assert.equal(baseline[lint]["sagascript-engine-protocol"], 0, lint);
  const lib = read("src-tauri/crates/sagascript-engine-protocol/src/lib.rs");
  const deny = /^#!\[deny\(([^\]]*)\)\]$/m.exec(lib)?.[1] ?? "";
  const denied = deny.split(",").map((item) => item.trim()).filter(Boolean);
  assert.deepEqual(denied, POLICY_LINTS.map((lint) => `clippy::${lint}`));
});

test("clippy.toml exempts tests and sets the size thresholds", () => {
  const config = read("src-tauri/clippy.toml");
  for (const line of [
    "allow-unwrap-in-tests = true",
    "allow-expect-in-tests = true",
    "allow-panic-in-tests = true",
    "allow-print-in-tests = true",
    "too-many-lines-threshold = 150",
    "cognitive-complexity-threshold = 30",
  ]) {
    assert.match(config, new RegExp(`^${line.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "m"));
  }
  assert.doesNotMatch(config, /^msrv\s*=/m, "clippy must keep reading the MSRV from Cargo.toml");
});

test("test-linux runs the policy check after the clippy gates", () => {
  const ci = read(".github/workflows/ci.yml");
  const start = ci.indexOf("\n  test-linux:\n");
  const end = ci.indexOf("\n  msrv-linux:\n");
  assert.ok(start >= 0 && end > start, "ci.yml must define test-linux before msrv-linux");
  const job = ci.slice(start, end);
  const steps = job.split(/\n(?=      - name: )/).slice(1);
  const names = steps.map((step) => /^      - name: (.*)$/m.exec(step)[1]);
  const index = steps.findIndex((step) => /^        run: node scripts\/clippy-policy\.mjs$/m.test(step));
  assert.ok(index >= 0, "test-linux must run `node scripts/clippy-policy.mjs` as its own step");
  assert.doesNotMatch(steps[index], /^        (if|continue-on-error|working-directory):/m);
  assert.ok(index > names.indexOf("Install Node.js"), "the check needs Node.js");
  assert.ok(index > names.indexOf("Cargo clippy (GUI app)"), "the regular clippy gate runs first");
  assert.ok(index > names.indexOf("Cargo clippy (headless CLI, batch-only)"), "the regular clippy gate runs first");
});
