import assert from "node:assert/strict";
import test from "node:test";
import { gateInvocation, loadWorkflow, readRepoFile } from "./lib/ci-workflow.mjs";
import { GATES, OSES, commandOf, gatesFor } from "./ci/cargo-gates.mjs";

// How CI covers the Cargo features of the CLI and of core (#81, #358). Structural: the gate list
// that every lane runs, not the text of ci.yml.

const manifest = (crate) => readRepoFile(`src-tauri/crates/${crate}/Cargo.toml`);
const workflow = loadWorkflow();
const lanes = { "test-macos": "macos", "test-linux": "linux", "test-windows": "windows" };

// The gates the lane really runs: found through the lane's own invocation of the gate script.
function gatesOfLane(jobId) {
  const invocation = workflow.job(jobId).steps.map(gateInvocation).find(Boolean);
  assert.ok(invocation, `${jobId} runs the gate script`);
  return gatesFor({ os: invocation.os, profile: invocation.profile });
}

test("the CLI keeps diarization in its default features and core keeps none", () => {
  assert.match(
    manifest("sagascript-cli"),
    /default\s*=\s*\[\s*"record"\s*,\s*"diarization"\s*\]/,
    "CLI default features must include diarization",
  );
  assert.match(manifest("sagascript-core"), /default\s*=\s*\[\s*\]/, "core must retain its no-feature default");
});

test("every OS lint and tests the workspace, which builds the CLI with those defaults, with no package excluded", () => {
  for (const [jobId, os] of Object.entries(lanes)) {
    const gates = gatesOfLane(jobId);
    for (const verb of ["clippy", "test"]) {
      const gate = gates.find((candidate) => candidate.args[0] === verb && candidate.args.includes("--workspace"));
      assert.ok(gate, `${os}: ${verb} --workspace`);
      assert.ok(
        !gate.args.some((argument) => ["--exclude", "--features", "--no-default-features", "-p"].includes(argument)),
        `${os}: ${commandOf(gate)}`,
      );
    }
    assert.deepEqual(
      gates.filter((gate) => gate.args.includes("--workspace") && ["clippy", "test"].includes(gate.args[0])).map((gate) => gate.args[0]),
      ["clippy", "test"],
    );
    assert.ok(OSES.includes(os));
  }
});

test("feature sets stay separate invocations: no gate names two packages, and the CLI never gets `--features diarization`", () => {
  for (const jobId of Object.keys(lanes)) {
    for (const gate of gatesOfLane(jobId)) {
      const packages = gate.args.filter((argument, index) => gate.args[index - 1] === "-p");
      assert.ok(packages.length <= 1, `${commandOf(gate)} combines packages, which would unify their features`);
      assert.ok(
        !(packages.includes("sagascript-cli") && gate.args.includes("--features")),
        `${commandOf(gate)}: the CLI's features come from its defaults, not a flag`,
      );
    }
  }
});

test("core is gated explicitly with and without diarization, and the lean CLI without default features", () => {
  const all = Object.keys(lanes).flatMap((jobId) => gatesOfLane(jobId));
  const matching = (predicate) => all.filter(predicate).map((gate) => gate.args.join(" "));
  const coreAlone = (gate) =>
    gate.args.includes("sagascript-core") && !gate.args.includes("--features") && !gate.args.includes("--release");
  const coreDiarization = (gate) => gate.args.includes("sagascript-core") && gate.args.join(" ").includes("--features diarization");
  // Core with no features, on every OS: clippy over all targets and the tests.
  for (const jobId of Object.keys(lanes)) {
    const gates = gatesOfLane(jobId);
    assert.deepEqual(
      gates.filter(coreAlone).map((gate) => gate.args[0]),
      ["clippy", "test"],
      `${jobId}: core without features`,
    );
  }
  // Core with diarization: a clippy over all targets and a test run, separate from the above. `cargo
  // check` is not a third gate: clippy --all-targets compiles everything it would.
  assert.deepEqual(
    new Set(matching(coreDiarization)),
    new Set([
      "clippy -p sagascript-core --features diarization --all-targets -- -D warnings",
      "test -p sagascript-core --features diarization",
    ]),
  );
  assert.ok(
    GATES.every((gate) => gate.args[0] !== "check" || ["msrv", "msrv-live"].includes(gate.profile)),
    "no plain cargo check outside the MSRV profiles",
  );
  // The lean CLI, which strips diarization with record, is linted and tested on every OS.
  for (const jobId of Object.keys(lanes)) {
    const lean = gatesOfLane(jobId).filter(
      (gate) =>
        ["clippy", "test"].includes(gate.args[0]) && gate.args.includes("sagascript-cli") && gate.args.includes("--no-default-features"),
    );
    assert.deepEqual(
      lean.map((gate) => gate.args[0]),
      ["clippy", "test"],
      `${jobId}: lean CLI`,
    );
  }
});
