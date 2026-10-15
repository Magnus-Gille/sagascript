import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { actionInput, commandLines, gateInvocation, loadAction, loadWorkflow, parseWorkflow } from "./lib/ci-workflow.mjs";
import { gatesFor } from "./ci/cargo-gates.mjs";
import { laneProblems } from "./ci/assert-lanes.mjs";

// The parallel lanes of ci.yml (#358): what each one needs, how the shared setup action and the
// aggregators are wired, and that the aggregators fail closed. The workflow is parsed, not matched.

const workflow = loadWorkflow();
const ASSERT_LANES = fileURLToPath(new URL("./ci/assert-lanes.mjs", import.meta.url));
const SCOPE_CONDITION = "${{ needs.scope.result == 'success' && needs.scope.outputs.docs_only != 'true' }}";
const LANES = ["test-macos", "build-macos", "test-linux", "msrv-linux", "test-windows", "build-windows"];
const CHECKS = { "check-macos": "macos", "check-linux": "linux", "check-windows": "windows" };
const setupStep = (job) => job.steps.find((step) => step.uses === "./.github/actions/setup");

test("the job set is the scope job, the six lanes and the three aggregators", () => {
  assert.deepEqual([...workflow.jobIds].sort(), ["scope", ...LANES, ...Object.keys(CHECKS)].sort());
});

test("every lane waits only for the scope gate and is skipped together with every other lane on a trusted docs-only change", () => {
  for (const id of LANES) {
    const job = workflow.job(id);
    assert.deepEqual(job.needs, ["scope"], `${id} must wait only for the scope gate`);
    assert.equal(job.condition, SCOPE_CONDITION, `${id} must run only on a successful, non-docs-only scope`);
  }
});

test("check-macos and check-windows keep their names, need every lane of their OS and run whatever the lanes did", () => {
  for (const [id, os] of Object.entries(CHECKS)) {
    const job = workflow.job(id);
    assert.equal(workflow.doc.jobs[id].name, undefined, `${id}: the check-run name is the job id; a name: would rename the required check`);
    const lanes = LANES.filter((lane) => lane.endsWith(`-${os}`));
    assert.ok(lanes.length >= 2, `${os} has a test lane and a second lane`);
    assert.deepEqual([...job.needs].sort(), ["scope", ...lanes].sort(), `${id} must wait for scope and every ${os} lane`);
    assert.equal(job.condition, "${{ always() }}", `${id} must report even when a lane failed or was skipped`);
    assert.equal(job.env.NEEDS, "${{ toJSON(needs) }}");
    assert.equal(job.timeoutMinutes, 20);
    assert.equal(job.steps[0].uses?.split("@")[0], "actions/checkout");
    assert.deepEqual(
      job.steps.slice(1).map((step) => step.run),
      ["node scripts/ci/assert-lanes.mjs"],
    );
    assert.ok(
      job.steps.every((step) => step.condition === undefined && step.continueOnError === undefined),
      `${id}'s steps are unconditional`,
    );
  }
});

// The `needs` context a check job would see: its own needs, with the results given.
function needsContext(id, { scope = "success", trusted = "true", docsOnly = "false", lane = "success", overrides = {} }) {
  const needs = { scope: { result: scope, outputs: { docs_only: docsOnly, scope_trusted: trusted, scope_classification: "x" } } };
  for (const name of workflow.job(id).needs.filter((need) => need !== "scope")) {
    needs[name] = { result: overrides[name] ?? lane, outputs: {} };
  }
  return needs;
}

for (const id of Object.keys(CHECKS)) {
  test(`${id} accepts only trusted docs-only skips or successful full lanes`, () => {
    const accepts = (options) => laneProblems(needsContext(id, options)).length === 0;
    assert.ok(accepts({ docsOnly: "true", lane: "skipped" }));
    assert.ok(accepts({ docsOnly: "false", lane: "success" }));
    for (const scope of ["failure", "cancelled", "skipped"]) {
      for (const docsOnly of ["true", "false"]) {
        assert.ok(
          !accepts({ scope, docsOnly, lane: docsOnly === "true" ? "skipped" : "success" }),
          `scope ${scope}, docs_only ${docsOnly}`,
        );
      }
    }
    for (const trusted of ["false", "", "untrusted"]) {
      assert.ok(!accepts({ trusted, docsOnly: "true", lane: "skipped" }), `scope_trusted ${trusted}`);
    }
    for (const docsOnly of ["", "unknown"]) assert.ok(!accepts({ docsOnly }), `docs_only ${JSON.stringify(docsOnly)}`);
    // Every lane the job waits for, one by one, with every result that is not the expected one.
    for (const lane of workflow.job(id).needs.filter((need) => need !== "scope")) {
      for (const result of ["failure", "cancelled", "skipped", ""]) {
        assert.ok(!accepts({ overrides: { [lane]: result } }), `${lane} ${JSON.stringify(result)} on a full run`);
      }
      for (const result of ["success", "failure", "cancelled", ""]) {
        assert.ok(
          !accepts({ docsOnly: "true", lane: "skipped", overrides: { [lane]: result } }),
          `${lane} ${JSON.stringify(result)} on a docs-only run`,
        );
      }
    }
  });
}

test("an aggregator that waits for no lane, or gets no usable needs context, fails", () => {
  assert.notEqual(laneProblems({ scope: { result: "success", outputs: { docs_only: "false", scope_trusted: "true" } } }).length, 0);
  for (const bad of [null, [], "success", 1]) assert.notEqual(laneProblems(bad).length, 0);
  const run = (env) => spawnSync(process.execPath, [ASSERT_LANES], { encoding: "utf8", env: { PATH: process.env.PATH, ...env } });
  assert.equal(run({ NEEDS: JSON.stringify(needsContext("check-macos", {})) }).status, 0);
  assert.equal(run({ NEEDS: JSON.stringify(needsContext("check-macos", { overrides: { "build-macos": "failure" } })) }).status, 1);
  assert.equal(run({}).status, 1, "no NEEDS");
  assert.equal(run({ NEEDS: "not json" }).status, 1);
  assert.match(run({ NEEDS: JSON.stringify(needsContext("check-macos", { lane: "failure" })) }).stderr, /test-macos: result is "failure"/);
});

test("the scope job publishes the outputs the aggregators read", () => {
  const scope = workflow.job("scope");
  assert.deepEqual(Object.keys(scope.outputs).sort(), ["docs_only", "scope_classification", "scope_trusted"]);
  for (const name of ["docs_only", "scope_trusted"]) assert.equal(scope.outputs[name], `\${{ steps.classify.outputs.${name} }}`);
});

test("the setup action is the only toolchain preamble: every lane uses it with declared inputs and the cache its gates need", () => {
  const action = loadAction("setup");
  const KEYS = {
    "test-macos": "sagascript-ci-checks",
    "test-windows": "sagascript-ci-checks",
    "build-macos": "sagascript-ci-native",
    "build-windows": "sagascript-ci-native",
    "test-linux": "sagascript-ci",
    "msrv-linux": "sagascript-msrv",
  };
  for (const id of LANES) {
    const job = workflow.job(id);
    const setups = job.steps.filter((step) => step.uses === "./.github/actions/setup");
    assert.equal(setups.length, 1, `${id} sets up once, through the action`);
    const [setup] = setups;
    for (const name of Object.keys(setup.with)) assert.ok(Object.hasOwn(action.inputs, name), `${id} passes an undeclared input: ${name}`);
    for (const [name, input] of Object.entries(action.inputs)) if (input.required) assert.ok(name in setup.with, `${id} must pass ${name}`);
    assert.equal(setup.with["cache-key"], KEYS[id]);
    assert.equal(job.steps[0].uses?.split("@")[0], "actions/checkout", `${id}: checkout stays in the job, ahead of the local action`);
    assert.equal(job.steps.indexOf(setup), 1, `${id}: the setup follows the checkout directly`);
    for (const step of job.steps) {
      assert.ok(
        !/^(actions\/setup-(python|node)|dtolnay\/rust-toolchain|Swatinem\/rust-cache)@/.test(step.uses ?? ""),
        `${id} repeats a step of the setup action`,
      );
    }

    // Which cargo gates the lane runs decides which Rust components and workspaces it needs.
    const invocation = job.steps.map(gateInvocation).find(Boolean);
    const gates = invocation ? gatesFor({ os: invocation.os ?? "linux", profile: invocation.profile }) : [];
    const components = actionInput(action, setup, "rust-components")
      .split(",")
      .map((part) => part.trim());
    if (gates.some((gate) => gate.args[0] === "clippy")) assert.ok(components.includes("clippy"), `${id} runs clippy`);
    if (gates.some((gate) => gate.args[0] === "fmt")) assert.ok(components.includes("rustfmt"), `${id} runs rustfmt`);
    const workspaces = actionInput(action, setup, "cache-workspaces")
      .split("\n")
      .map((line) => line.trim());
    assert.ok(workspaces.includes("src-tauri"));
    if (gates.some((gate) => gate.cwd === "src-tauri/engine-host/ort")) {
      assert.ok(workspaces.includes("src-tauri/engine-host/ort"), `${id} runs ORT host gates and must cache its workspace`);
    }
  }
  const msrv = setupStep(workflow.job("msrv-linux"));
  assert.equal(actionInput(action, msrv, "rust"), "msrv");
  assert.equal(actionInput(action, msrv, "python"), "false");
});

test("every lane that runs Python selects the supported runtime first", () => {
  // Moved here from scripts/dictation_eval/test_evaluate_cli.py: the workflow is parsed, not read as text.
  const action = loadAction("setup");
  const python = action.steps.find((step) => step.uses?.startsWith("actions/setup-python@"));
  assert.equal(python.with["python-version"], "3.12");
  assert.equal(python.condition, "${{ inputs.python == 'true' }}");
  assert.equal(action.inputs.python.default, "true");
  for (const id of ["test-macos", "test-linux", "test-windows"]) {
    const steps = workflow.job(id).steps;
    const setup = steps.findIndex((step) => step.uses === "./.github/actions/setup");
    const suites = steps.findIndex((step) =>
      commandLines(step).some((line) => line.startsWith("python -m unittest discover -s scripts/dictation_eval")),
    );
    assert.equal(actionInput(action, steps[setup], "python"), "true", `${id} selects Python`);
    assert.ok(setup >= 0 && suites > setup, `${id} selects Python before the evaluator suites`);
  }
  // The native build lanes run the JSON stream verifier of the smoke tests with it; the MSRV lane runs no Python.
  for (const id of ["build-macos", "build-windows"]) assert.equal(actionInput(action, setupStep(workflow.job(id)), "python"), "true");
  assert.equal(actionInput(action, setupStep(workflow.job("msrv-linux")), "python"), "false");
});

test("the Rust cache key follows the gate list: rust-cache saves only under a new key, so new gates need one to get cached", () => {
  const cache = loadAction("setup").steps.find((step) => step.uses?.startsWith("Swatinem/rust-cache@"));
  assert.equal(cache.with["shared-key"], "${{ inputs.cache-key }}-${{ hashFiles('scripts/ci/cargo-gates.mjs') }}");
});

test("only the native build lanes cache downloaded models", () => {
  const action = loadAction("setup");
  for (const id of LANES) {
    assert.equal(
      actionInput(action, setupStep(workflow.job(id)), "models"),
      id.startsWith("build-") ? "true" : "false",
      `${id}: model cache`,
    );
  }
  const step = action.steps.find((candidate) => candidate.name === "Cache downloaded models");
  assert.equal(step.condition, "${{ inputs.models == 'true' }}");
  for (const source of ["settings/manager.rs", "transcription/model.rs", "diarization/model.rs"]) {
    assert.ok(step.with.key.includes(`sagascript-core/src/${source}`), `the model cache key hashes ${source}`);
  }
});

test("the setup action's steps run in the order the lanes depend on", () => {
  const action = loadAction("setup");
  const order = action.steps.map((step) => step.name);
  const before = (first, second) =>
    assert.ok(order.indexOf(first) >= 0 && order.indexOf(first) < order.indexOf(second), `${first} before ${second}`);
  before("Install Tauri system dependencies", "Select evaluation Python runtime");
  before("Install Rust", "Cache Rust dependencies");
  before("Install the declared MSRV toolchain", "Cache Rust dependencies");
  before("Install Node.js", "Install npm dependencies");
  before("Cache Rust dependencies", "Install npm dependencies");
  // The MSRV toolchain is exported before anything later in the lane looks at rustc.
  const lines = commandLines(action.steps.find((step) => step.name === "Install the declared MSRV toolchain"));
  assert.ok(lines.includes('echo "RUSTUP_TOOLCHAIN=$msrv" >> "$GITHUB_ENV"'));
  assert.ok(
    lines.some((line) => line.includes("src-tauri/Cargo.toml")) && lines.some((line) => line.includes("engine-host/ort/Cargo.toml")),
  );
});

test("test-windows runs the build identity gates before npm ci and any Cargo work, from sources that need no npm packages", () => {
  const steps = workflow.job("test-windows").steps;
  const find = (predicate) => steps.findIndex((step) => commandLines(step).some(predicate));
  const identityCommand =
    "node --test scripts/test-ci-build-identity.mjs scripts/test-windows-release-identity.mjs scripts/test-windows-package-workflow.mjs";
  const identity = find((line) => line.startsWith(identityCommand));
  const setup = steps.findIndex((step) => step.uses === "./.github/actions/setup");
  const install = find((line) => line === "npm ci");
  const gates = find((line) => line.startsWith("node scripts/ci/cargo-gates.mjs"));
  assert.ok(setup >= 0 && identity > setup, "after Node is installed");
  assert.equal(
    actionInput(loadAction("setup"), steps[setup], "npm-ci"),
    "false",
    "the action must not run npm ci ahead of the identity gates",
  );
  assert.ok(install > identity && gates > install, "npm ci follows the identity gates; Cargo work follows npm ci");
  // They run before node_modules exists, so they and what they import must not need it.
  for (const file of [
    "test-ci-build-identity",
    "test-windows-release-identity",
    "test-windows-package-workflow",
    "test-release-promotion",
  ]) {
    const source = readFileSync(new URL(`./${file}.mjs`, import.meta.url), "utf8");
    assert.ok(!/from "yaml"|ci-workflow\.mjs/.test(source), `${file}.mjs runs before npm ci and must not import the YAML reader`);
  }
});

test("windows-package.yml leaves the Rust tests of a pull request to test-windows and keeps them for releases and manual runs", () => {
  const windows = loadWorkflow("windows-package.yml").jobs[0];
  const names = windows.steps.map((step) => step.name);
  const tests = windows.steps[names.indexOf("Test and lint Rust workspace")];
  assert.equal(tests.condition, "github.event_name != 'pull_request'");
  // The debug lean CLI that the CPU policy checks read stays built: inside the step above where it runs,
  // in a step of its own exactly where that step is skipped.
  assert.ok(commandLines(tests).includes("cargo build -p sagascript-cli --no-default-features"));
  const build = windows.steps[names.indexOf("Build lean CLI (debug)")];
  assert.ok(build, "the debug build has a step of its own for the skipped case");
  assert.equal(build.condition, "github.event_name == 'pull_request'");
  assert.deepEqual(commandLines(build), ["cargo build -p sagascript-cli --no-default-features"]);
  assert.ok(names.indexOf(build.name) > names.indexOf(tests.name));
  assert.ok(names.indexOf(build.name) < names.indexOf("Verify debug x64 inference CPU policy"));
  assert.ok(names.indexOf(build.name) < names.indexOf("Verify debug ARM64 native CPU flags"));
  // test-windows runs the tests this one no longer repeats on a pull request.
  assert.ok(gatesFor({ os: "windows" }).some((gate) => gate.args.join(" ") === "test --workspace"));
  assert.ok(gatesFor({ os: "windows" }).some((gate) => gate.args.join(" ") === "clippy --workspace --all-targets -- -D warnings"));
});

test("the workflow parses the same with CRLF line endings, as on a Windows checkout", () => {
  const source = readFileSync(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8").replace(/\r\n?/g, "\n");
  const lf = parseWorkflow(source);
  const crlf = parseWorkflow(source.replaceAll("\n", "\r\n"));
  assert.deepEqual(crlf.jobs, lf.jobs);
  const classify = crlf.job("scope").steps.find((step) => step.id === "classify");
  assert.ok(!classify.run.includes("\r"), "scripts stay free of carriage returns");
});

test("a multi-command step of a Windows lane runs under bash: pwsh reports only the last native command's exit code", () => {
  // `npm run test:frontend` failing in front of a passing `npm run format:check` went green in a first draft of this file.
  const windowsJobs = workflow.jobs.filter((job) => /^windows/.test(job.runsOn));
  assert.deepEqual(windowsJobs.map((job) => job.id).sort(), ["build-windows", "test-windows"]);
  for (const job of windowsJobs) {
    for (const step of job.steps.filter((candidate) => candidate.run !== "")) {
      if (commandLines(step).length > 1) assert.equal(step.shell, "bash", `${job.id}: "${step.name}" runs several commands`);
    }
  }
  for (const step of loadAction("setup").steps.filter((candidate) => candidate.run !== "")) {
    assert.equal(step.shell, "bash", `the setup action's "${step.name}" must name its shell (and runs on Windows)`);
  }
});
