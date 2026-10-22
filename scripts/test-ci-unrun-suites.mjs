import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { commandOf, gatesFor } from "./ci/cargo-gates.mjs";
import { actionInput, commandLines, loadAction, loadWorkflow, readRepoFile } from "./lib/ci-workflow.mjs";

// #357: suites that used to run in no PR lane (or only in the release gate) are now part of PR CI.
// These assertions keep them there. The gate list (scripts/ci/cargo-gates.mjs) says which cargo
// commands each OS runs; the workflow says which lane runs the list, and when.

const workflow = loadWorkflow();
const action = loadAction("setup");
const devOverrides = readRepoFile("src-tauri/crates/sagascript-core/src/transcription/dev_overrides.rs");
const workspaceManifest = readRepoFile("src-tauri/Cargo.toml");
const ortManifest = readRepoFile("src-tauri/engine-host/ort/Cargo.toml");
const contributing = readRepoFile("CONTRIBUTING.md");

const gateLines = (os) => gatesFor({ os }).map((gate) => `${gate.cwd}: ${commandOf(gate)}`);
const stepIndex = (jobId, predicate) => workflow.job(jobId).steps.findIndex(predicate);
const runs = (command) => (step) => commandLines(step).includes(command);
const gatesStep = (os) => (step) => commandLines(step).includes(`node scripts/ci/cargo-gates.mjs --os ${os}`);
// The step that runs `npm ci`: the setup action when it is asked to, otherwise an explicit step.
function npmInstall(jobId) {
  return stepIndex(
    jobId,
    (step) => (step.uses === "./.github/actions/setup" && actionInput(action, step, "npm-ci") === "true") || runs("npm ci")(step),
  );
}
function assertBlocking(jobId, index, what) {
  const step = workflow.job(jobId).steps[index];
  assert.ok(step, `${jobId} must have ${what}`);
  assert.equal(step.condition, undefined, `${what} must be unconditional`);
  assert.equal(step.continueOnError, undefined, `${what} must be blocking`);
}

test("every lane runs the engine-protocol wire-format tests through the workspace tests", () => {
  const members = /^members = \[(.*)\]$/m.exec(workspaceManifest)?.[1] ?? "";
  assert.ok(members.includes('"crates/*"'), "the workspace takes every crate under crates/");
  assert.ok(
    readdirSync(fileURLToPath(new URL("../src-tauri/crates/", import.meta.url))).includes("sagascript-engine-protocol"),
    "the protocol crate is one of them",
  );
  for (const os of ["macos", "linux", "windows"]) {
    assert.ok(
      gateLines(os).includes("src-tauri: cargo test --workspace"),
      `${os} runs cargo test --workspace, which reaches the protocol crate`,
    );
  }
  for (const [jobId, os] of [
    ["test-macos", "macos"],
    ["test-linux", "linux"],
    ["test-windows", "windows"],
  ]) {
    assertBlocking(jobId, stepIndex(jobId, gatesStep(os)), "the cargo gates");
  }
});

test("test-linux and test-windows test and lint the standalone ORT engine host workspace", () => {
  assert.match(ortManifest, /^\[workspace\]$/m, "the ORT host is its own Cargo workspace");
  for (const os of ["linux", "windows"]) {
    const lines = gateLines(os);
    assert.ok(lines.includes("src-tauri/engine-host/ort: cargo test --locked"), `${os} tests the ORT host`);
    assert.ok(lines.includes("src-tauri/engine-host/ort: cargo clippy --all-targets --locked -- -D warnings"), `${os} lints the ORT host`);
  }
  assert.ok(
    !gateLines("macos").some((line) => line.startsWith("src-tauri/engine-host/ort")),
    "macOS ships the Core ML host, not the ORT host",
  );
  assert.ok(
    gateLines("linux").includes("src-tauri: cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check"),
    "test-linux must keep its rustfmt gate for the ORT host",
  );
  for (const jobId of ["test-linux", "test-windows"]) {
    const setup = workflow.job(jobId).steps.find((step) => step.uses === "./.github/actions/setup");
    assert.deepEqual(
      actionInput(action, setup, "cache-workspaces")
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean),
      ["src-tauri", "src-tauri/engine-host/ort"],
      `${jobId} must cache both Cargo workspaces`,
    );
  }
});

test("test-linux runs the release-only dev-override guard in the release profile", () => {
  // The test exists only without debug assertions and without the feature...
  assert.match(
    devOverrides,
    /#\[cfg\(not\(any\(debug_assertions, feature = "dev-overrides"\)\)\)\]\n    #\[test\]\n    fn release_build_ignores_the_real_environment\(\)/,
  );
  // ...and its doc comment names the command CI must run.
  const command = "cargo test --release -p sagascript-core --lib dev_overrides";
  assert.ok(devOverrides.includes(`(\`${command}\`)`), "the test's doc comment must name the CI command");
  const guards = gatesFor({ os: "linux" }).filter((gate) => commandOf(gate) === command);
  assert.equal(guards.length, 1, "test-linux must run the release guard exactly once");
  const [guard] = guards;
  assert.equal(guard.cwd, "src-tauri");
  // cargo test exits 0 when the filter matches nothing; the gate must prove the guard test itself ran.
  assert.ok(
    guard.requireOutput?.pattern.test("release_build_ignores_the_real_environment ... ok"),
    "the gate must require the guard's own pass line",
  );
  assert.ok(!guard.requireOutput.pattern.test("running 0 tests"));
  assert.ok(
    ["macos", "windows"].every((os) => !gateLines(os).some((line) => line.includes("dev_overrides"))),
    "the guard compiles core in the release profile once, on Linux",
  );

  // A release profile with debug assertions would compile the guard out and
  // leave the step green with zero tests.
  const release = /^\[profile\.release\]\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(workspaceManifest)?.[1];
  assert.ok(release !== undefined, "src-tauri/Cargo.toml must define [profile.release]");
  assert.doesNotMatch(release, /^debug-assertions\s*=\s*true/m);
  const environments = [
    workflow.doc.env,
    action.doc.runs.env,
    ...workflow.jobs.flatMap((job) => [job.env, ...job.steps.map((step) => step.env)]),
    ...action.steps.map((step) => step.env),
  ];
  for (const environment of environments) {
    assert.ok(
      !Object.keys(environment ?? {}).includes("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"),
      "no lane may switch debug assertions on in release",
    );
  }
  for (const name of readdirSync(fileURLToPath(new URL("../.github/workflows/", import.meta.url)))) {
    assert.ok(
      !readFileSync(new URL(`../.github/workflows/${name}`, import.meta.url), "utf8").includes("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"),
      name,
    );
  }
});

test("Cargo dev/test profiles keep only line-table debug info by default", () => {
  const manifests = [
    ["main workspace", workspaceManifest],
    ["ORT host workspace", ortManifest],
  ];
  for (const [name, manifest] of manifests) {
    for (const profileName of ["dev", "test"]) {
      const block = new RegExp(`^\\[profile\\.${profileName}\\]\\n([\\s\\S]*?)(?=^\\[|(?![\\s\\S]))`, "m").exec(manifest)?.[1];
      assert.ok(block !== undefined, `${name} must define [profile.${profileName}]`);
      assert.match(block, /^debug\s*=\s*["']line-tables-only["']\s*$/m);
      assert.doesNotMatch(
        block,
        /^(debug-assertions|overflow-checks|opt-level|panic)\s*=/m,
        `${name} must retain Cargo defaults outside debug-info selection`,
      );
    }
  }

  const release = /^\[profile\.release\]\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(workspaceManifest)?.[1];
  assert.ok(release !== undefined, "main workspace must define [profile.release]");
  assert.match(release, /^opt-level\s*=\s*3\s*$/m);
  assert.match(release, /^lto\s*=\s*["']thin["']\s*$/m);
  assert.match(release, /^codegen-units\s*=\s*1\s*$/m);
  assert.doesNotMatch(release, /^(debug-assertions|overflow-checks|panic)\s*=/m);

  assert.match(contributing, /CARGO_PROFILE_DEV_DEBUG=full/);
  assert.match(contributing, /CARGO_PROFILE_TEST_DEBUG=full/);
});

test("test-windows runs the full frontend suite after npm ci and before Cargo work", () => {
  const suite = stepIndex("test-windows", runs("npm run test:frontend"));
  const install = npmInstall("test-windows");
  assert.ok(install >= 0, "test-windows must install npm dependencies");
  assert.ok(suite > install, "the suite needs node_modules");
  assert.ok(suite < stepIndex("test-windows", gatesStep("windows")), "the suite must fail before Cargo work");
  assertBlocking("test-windows", suite, "`npm run test:frontend`");
  const identity = stepIndex("test-windows", (step) =>
    commandLines(step).some((line) => line.startsWith("node --test scripts/test-ci-build-identity.mjs")),
  );
  assert.ok(identity >= 0 && identity < install, "the early build identity gates stay ahead of npm ci");
});

test("test-macos runs ESLint after npm ci and before Cargo work", () => {
  const lint = stepIndex("test-macos", runs("npm run lint"));
  assert.ok(lint > npmInstall("test-macos"), "`npm run lint` needs node_modules");
  assert.ok(lint < stepIndex("test-macos", gatesStep("macos")), "`npm run lint` must fail before Cargo work");
  assertBlocking("test-macos", lint, "`npm run lint`");
});

test("test-macos and test-windows run the Vitest unit tests and the Prettier check after npm ci", () => {
  for (const [jobId, os] of [
    ["test-macos", "macos"],
    ["test-windows", "windows"],
  ]) {
    const install = npmInstall(jobId);
    assert.ok(install >= 0, `${jobId} must install npm dependencies`);
    for (const command of ["npm run test:unit", "npm run format:check"]) {
      const index = stepIndex(jobId, runs(command));
      assert.ok(index > install, `${jobId}: \`${command}\` needs node_modules`);
      assert.ok(index < stepIndex(jobId, gatesStep(os)), `${jobId}: \`${command}\` must fail before Cargo work`);
      assertBlocking(jobId, index, `\`${command}\``);
    }
  }
});
