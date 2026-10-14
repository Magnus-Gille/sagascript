import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import test from "node:test";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { allSteps, commandLines, gateInvocation, loadWorkflow } from "./lib/ci-workflow.mjs";
import { GATES, OSES, PROFILES, RELEASE_GUARD_PROOF, REPO_ROOT, commandOf, gatesFor, runGates } from "./ci/cargo-gates.mjs";

// The list of cargo gates (scripts/ci/cargo-gates.mjs) is the one source of the cargo commands every
// lane of ci.yml runs (#358). These tests hold the list, its wiring into the lanes and its runner.

const SCRIPT = fileURLToPath(new URL("./ci/cargo-gates.mjs", import.meta.url));
const lines = (os, profile) => gatesFor({ os, profile }).map((gate) => `${gate.cwd}: ${commandOf(gate)}`);

const COMMON = [
  "src-tauri: cargo clippy --workspace --all-targets -- -D warnings",
  "src-tauri: cargo test --workspace",
  "src-tauri: cargo clippy -p sagascript-core --all-targets -- -D warnings",
  "src-tauri: cargo test -p sagascript-core",
  "src-tauri: cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings",
  "src-tauri: cargo test -p sagascript-cli --no-default-features",
];
const ORT = [
  "src-tauri/engine-host/ort: cargo clippy --all-targets --locked -- -D warnings",
  "src-tauri/engine-host/ort: cargo test --locked",
];
const DENY = [
  "src-tauri: cargo deny --locked --workspace check",
  "src-tauri: cargo deny --locked --manifest-path engine-host/ort/Cargo.toml --workspace check -A license-not-encountered -A license-exception-not-encountered -A unmatched-source",
];

// The commands each lane ran before the list existed, plus the ones the list added on purpose,
// spelled out so that dropping or adding a gate has to change this fixture too.
const EXPECTED = {
  macos: COMMON,
  windows: [...COMMON, ...ORT],
  linux: [
    ...DENY,
    "src-tauri: cargo fmt --all -- --check",
    "src-tauri: cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check",
    ...COMMON,
    "src-tauri: cargo clippy -p sagascript-core --features diarization --all-targets -- -D warnings",
    "src-tauri: cargo test -p sagascript-core --features diarization",
    "src-tauri: cargo test -p sagascript-core --features record",
    ...ORT,
    "src-tauri: cargo build -p sagascript-cli --no-default-features",
    "src-tauri: cargo tree -p sagascript-cli --no-default-features -e normal",
    "src-tauri: cargo test --release -p sagascript-core --lib dev_overrides",
  ],
  msrv: [
    "src-tauri: cargo check --workspace --locked",
    "src-tauri: cargo check -p sagascript-cli --no-default-features --locked",
    "src-tauri/engine-host/ort: cargo check --locked",
  ],
};

test("every gate is well formed, and every per-OS exclusion states its reason", () => {
  assert.equal(new Set(GATES.map((gate) => gate.id)).size, GATES.length, "gate ids are unique");
  for (const gate of GATES) {
    assert.ok(PROFILES.includes(gate.profile), `${gate.id}: profile`);
    assert.ok(existsSync(join(REPO_ROOT, gate.cwd)), `${gate.id}: ${gate.cwd} exists`);
    assert.ok(gate.args.length > 0 && gate.args.every((argument) => typeof argument === "string" && argument !== ""), `${gate.id}: args`);
    if (gate.os === undefined) {
      assert.equal(gate.why, undefined, `${gate.id} runs everywhere and needs no exclusion reason`);
    } else {
      assert.ok(gate.os.length > 0 && gate.os.every((os) => OSES.includes(os)), `${gate.id}: os`);
      assert.ok(gate.os.length < OSES.length, `${gate.id} lists every OS; drop the restriction`);
      assert.ok(typeof gate.why === "string" && gate.why.length >= 20, `${gate.id} excludes an OS and must say why`);
    }
  }
});

test("each lane runs exactly the expected cargo commands, in order", () => {
  for (const os of OSES) assert.deepEqual(lines(os), EXPECTED[os], `${os} gates`);
  assert.deepEqual(lines("linux", "msrv"), EXPECTED.msrv, "msrv gates");
});

test("the core gates run on every OS; every other difference between the OS lists is an exclusion in the list", () => {
  const everywhere = OSES.map((os) => new Set(lines(os)));
  for (const command of COMMON)
    assert.ok(
      everywhere.every((set) => set.has(command)),
      `${command} must run on every OS`,
    );
  for (const gate of GATES.filter((entry) => entry.profile === "main")) {
    const runsOn = OSES.filter((os) => lines(os).includes(`${gate.cwd}: ${commandOf(gate)}`));
    assert.deepEqual(
      runsOn,
      OSES.filter((os) => gate.os === undefined || gate.os.includes(os)),
      `${gate.id} runs where it says`,
    );
  }
});

test("the release guard proves its test ran and the dependency-tree gate forbids cpal and ALSA", () => {
  const guard = GATES.find((gate) => gate.id === "release-guard");
  assert.ok(guard.requireOutput.pattern.test("test dev_overrides::tests::release_build_ignores_the_real_environment ... ok"));
  assert.ok(!guard.requireOutput.pattern.test("running 0 tests"));
  assert.equal(guard.requireOutput.pattern, RELEASE_GUARD_PROOF);
  const tree = GATES.find((gate) => gate.id === "no-alsa");
  assert.ok(tree.forbidOutput.pattern.test("├── cpal v0.15.3") && tree.forbidOutput.pattern.test("alsa-sys"));
  assert.ok(!tree.forbidOutput.pattern.test("sagascript-core v1.4.4"));
});

test("cargo-deny gates run first, before any gate that compiles", () => {
  const linux = gatesFor({ os: "linux" });
  assert.deepEqual(
    linux.slice(0, 2).map((gate) => gate.args[0]),
    ["deny", "deny"],
  );
  assert.ok(linux.slice(2).every((gate) => gate.args[0] !== "deny"));
});

test("every OS lane runs the gate script once with its own OS, and the MSRV lane its profile", () => {
  const workflow = loadWorkflow();
  const osOf = (runsOn) => (/^macos/.test(runsOn) ? "macos" : /^windows/.test(runsOn) ? "windows" : "linux");
  const lanes = { "test-macos": "macos", "test-linux": "linux", "test-windows": "windows" };
  for (const [jobId, os] of Object.entries(lanes)) {
    const job = workflow.job(jobId);
    assert.equal(osOf(job.runsOn), os, `${jobId} runs on ${os}`);
    const invocations = job.steps.map(gateInvocation).filter(Boolean);
    assert.equal(invocations.length, 1, `${jobId} must run the gate script exactly once`);
    assert.deepEqual([invocations[0].os, invocations[0].profile], [os, "main"]);
    const step = job.steps.find((candidate) => gateInvocation(candidate));
    assert.equal(step.condition, undefined, `${jobId}: the gates are unconditional`);
    assert.equal(step.continueOnError, undefined, `${jobId}: the gates are blocking`);
    assert.equal(step.workingDirectory, undefined, "the script resolves each gate's directory itself");
  }
  const msrv = workflow.job("msrv-linux");
  const [invocation] = msrv.steps.map(gateInvocation).filter(Boolean);
  assert.deepEqual([invocation.os, invocation.profile], [undefined, "msrv"]);
  for (const { job } of allSteps(workflow).filter(({ step }) => gateInvocation(step))) {
    assert.ok([...Object.keys(lanes), "msrv-linux"].includes(job.id), `${job.id} must not run the cargo gates`);
  }
});

test("ci.yml writes no cargo gate command of its own: the only cargo it runs is the cargo-deny install", () => {
  const workflow = loadWorkflow();
  const cargo = allSteps(workflow).flatMap(({ job, step }) =>
    commandLines(step)
      .filter((line) => /^cargo\s/.test(line))
      .map((line) => `${job.id}: ${line}`),
  );
  assert.deepEqual(cargo, ["test-linux: cargo install --locked cargo-deny --version 0.20.2"]);
});

test("windows-package.yml keeps the workspace commands the gates run", () => {
  const workflow = loadWorkflow("windows-package.yml");
  const step = workflow.jobs[0].steps.find((candidate) => candidate.name === "Test and lint Rust workspace");
  const commands = commandLines(step);
  for (const id of ["test-workspace", "clippy-workspace"]) {
    const gate = GATES.find((candidate) => candidate.id === id);
    assert.ok(commands.includes(commandOf(gate)), `${commandOf(gate)} must stay in step with the gate ${id}`);
  }
});

// -- the runner ----------------------------------------------------------------------------------

const fakeGate = (id, extra = {}) => ({ id, cwd: "src-tauri", args: ["test", id], profile: "main", ...extra });
const executeWith = (outcomes) => async (gate) => outcomes[gate.id] ?? { code: 0, output: "" };

test("the runner stops at the first failing gate unless it keeps going", async () => {
  const gates = [fakeGate("a"), fakeGate("b"), fakeGate("c")];
  const execute = executeWith({ b: { code: 101, output: "" } });
  const stopped = await runGates(gates, { execute });
  assert.deepEqual(
    stopped.map((entry) => [entry.gate.id, entry.failure]),
    [
      ["a", null],
      ["b", "exited with status 101"],
    ],
  );
  const all = await runGates(gates, { execute, keepGoing: true });
  assert.deepEqual(
    all.map((entry) => entry.gate.id),
    ["a", "b", "c"],
  );
  assert.equal(all.filter((entry) => entry.failure).length, 1);
});

test("a gate also fails on output it must not contain, or lacks the output it must contain", async () => {
  const forbid = fakeGate("tree", { forbidOutput: { pattern: /alsa/, message: "alsa leaked" }, quiet: true });
  const require = fakeGate("guard", { requireOutput: { pattern: /ran ok/, message: "guard did not run" } });
  const logged = [];
  const log = (line) => logged.push(line);
  const [leak] = await runGates([forbid], { execute: executeWith({ tree: { code: 0, output: "alsa-sys" } }), log });
  assert.equal(leak.failure, "alsa leaked");
  assert.ok(logged.includes("alsa-sys"), "a quiet gate shows its output when it fails");
  logged.length = 0;
  const [clean] = await runGates([forbid], { execute: executeWith({ tree: { code: 0, output: "cpal-free" } }), log });
  assert.equal(clean.failure, null);
  assert.ok(!logged.includes("cpal-free"), "a quiet gate stays quiet when it passes");
  const [missing] = await runGates([require], { execute: executeWith({ guard: { code: 0, output: "running 0 tests" } }) });
  assert.equal(missing.failure, "guard did not run");
  const [proved] = await runGates([require], { execute: executeWith({ guard: { code: 0, output: "it ran ok" } }) });
  assert.equal(proved.failure, null);
  const [failed] = await runGates([require], { execute: executeWith({ guard: { code: 1, output: "ran ok" } }) });
  assert.equal(failed.failure, "exited with status 1");
});

// -- the command line ----------------------------------------------------------------------------

function cli(args, env = {}) {
  const inherited = { ...process.env };
  delete inherited.RUSTUP_TOOLCHAIN;
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", env: { ...inherited, ...env } });
}

test("--list prints a lane's commands and runs nothing", () => {
  for (const os of OSES) {
    const result = cli(["--os", os, "--list"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.trim().split("\n"), EXPECTED[os]);
  }
  assert.deepEqual(cli(["--profile", "msrv", "--list"]).stdout.trim().split("\n"), EXPECTED.msrv);
});

test("the script refuses to run another OS's gates, an unknown option and an MSRV run without a toolchain", () => {
  const foreign = OSES.find((os) => os !== { darwin: "macos", linux: "linux", win32: "windows" }[process.platform]);
  const wrongOs = cli(["--os", foreign]);
  assert.equal(wrongOs.status, 2);
  assert.match(wrongOs.stderr, /refusing to run the \w+ gates/);
  assert.equal(cli(["--nonsense"]).status, 2);
  assert.equal(cli(["--os", "plan9", "--list"]).status, 2);
  const noToolchain = cli(["--profile", "msrv"]);
  assert.equal(noToolchain.status, 2);
  assert.match(noToolchain.stderr, /RUSTUP_TOOLCHAIN/);
});
