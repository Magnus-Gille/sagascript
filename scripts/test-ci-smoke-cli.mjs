import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { allSteps, commandLines, loadWorkflow } from "./lib/ci-workflow.mjs";
import { COMPLETION_SHELLS, DEFAULT_BINARY, TIERS, runTier } from "./ci/smoke-cli.mjs";

// One smoke script for every OS (scripts/ci/smoke-cli.mjs, #358) replaces the inline bash and
// PowerShell blocks of the two native build lanes. These tests hold what it checks, how the
// lanes call it, and its behaviour against a fake binary.

const SCRIPT = fileURLToPath(new URL("./ci/smoke-cli.mjs", import.meta.url));

// A harness that records the commands a tier runs instead of running anything. `language` plays
// the persisted setting `config set` / `config reset` change.
function recordingHarness() {
  const calls = [];
  let language = "en";
  return {
    calls,
    binary: "sagascript",
    scratch: "/scratch",
    assert(condition, message) {
      if (!condition) throw new Error(message);
    },
    checkBinary() {
      calls.push("check binary");
    },
    runProgram(program, args) {
      calls.push([program, ...args].join(" "));
      return { code: 0, stdout: "", stderr: "" };
    },
    ok(args) {
      calls.push(args.join(" "));
      if (args[0] === "config" && args[1] === "set") language = args[3];
      if (args[0] === "config" && args[1] === "reset") language = "en";
      if (args[0] === "config" && args[1] === "get") return `${language}\n`;
      if (args[0] === "transcribe") return "Stortinget har vedtatt\n";
      if (args[0] === "completions") return "# completion script\n";
      return "";
    },
    capture(label, args) {
      calls.push(args.join(" "));
      return { code: args.at(-1).endsWith("missing.wav") ? 1 : 0, stdout: `${label}.stdout`, stderr: `${label}.stderr` };
    },
    verifyStreams(mode) {
      calls.push(`verify ${mode.join(" ") || "--json"}`);
    },
    audioDirectory: () => "/scratch/jsonl-inputs",
    async readFirstLineThenClose(args) {
      calls.push(`${args.join(" ")} | head -1`);
      return { code: 0, sawLine: true };
    },
  };
}

const quiet = { log: () => {} };

test("the basic tier covers the binary, help, config, model listing, formats and completions of every shell", async () => {
  const harness = recordingHarness();
  const failed = await runTier("basic", harness, quiet);
  assert.deepEqual(failed, []);
  assert.deepEqual(harness.calls, [
    "check binary",
    "--version",
    "--help",
    "transcribe --help",
    "record --help",
    "config list",
    "config get language",
    "config set language sv",
    "config get language",
    "config reset language",
    "config get language",
    "config path",
    "list-models",
    "formats",
    ...COMPLETION_SHELLS.map((shell) => `completions ${shell}`),
  ]);
  assert.deepEqual(COMPLETION_SHELLS, ["zsh", "bash", "powershell"], "completions for every shell on every OS");
});

test("the model tier downloads, transcribes and verifies the JSON, JSONL and error streams of the output contract", async () => {
  const harness = recordingHarness();
  const failed = await runTier("model", harness, quiet);
  assert.deepEqual(failed, []);
  const model = "--language no --model nb-whisper-tiny";
  assert.deepEqual(harness.calls, [
    "download-model nb-whisper-tiny",
    `transcribe ${model} test-audio/norwegian-short-3s.mp3`,
    `transcribe ${model} --json test-audio/norwegian-short-3s.mp3`,
    "verify --json",
    `transcribe ${model} --jsonl /scratch/jsonl-inputs`,
    "verify --jsonl",
    `transcribe ${model} --jsonl /scratch/jsonl-inputs | head -1`,
    `transcribe ${model} --json /scratch/missing.wav`,
    "verify --error",
  ]);
});

test("every check of a tier runs even after one fails, and each failure is reported by name", async () => {
  const harness = recordingHarness();
  const original = harness.ok;
  harness.ok = (args) => {
    if (args[0] === "list-models") throw new Error("list-models exited with status 1");
    return original(args);
  };
  const logged = [];
  const failed = await runTier("basic", harness, { log: (line) => logged.push(line) });
  assert.deepEqual(failed, ["model listing and formats"]);
  assert.ok(harness.calls.includes("completions powershell"), "later checks still ran");
  assert.ok(logged.some((line) => line.startsWith("::error title=Smoke test failed::model listing and formats")));
});

test("the lanes run the smoke script after the native build, and only the Windows model tier is non-blocking", () => {
  const workflow = loadWorkflow();
  for (const jobId of ["build-macos", "build-windows"]) {
    const steps = workflow.job(jobId).steps;
    const index = (predicate) => steps.findIndex((step) => commandLines(step).some(predicate));
    const build = index((line) => line.startsWith("npx tauri build"));
    const basic = index((line) => line === "node scripts/ci/smoke-cli.mjs basic");
    const model = index((line) => line === "node scripts/ci/smoke-cli.mjs model");
    assert.ok(build >= 0 && basic > build && model >= basic, `${jobId}: build, then the basic tier, then the model tier`);
    assert.equal(steps[basic].continueOnError, undefined, `${jobId}: the basic tier blocks`);
    assert.equal(steps[model].continueOnError, jobId === "build-windows" ? true : undefined, `${jobId}: model tier tolerance`);
  }
  const tolerant = allSteps(workflow).filter(({ step }) => step.continueOnError !== undefined);
  assert.deepEqual(
    tolerant.map(({ job, step }) => `${job.id}:${step.id}`),
    ["build-windows:smoke_model"],
    "the Windows model tier is the only step that tolerates failure (#357)",
  );
  assert.ok(
    tolerant.every(({ job }) => job.continueOnError === undefined),
    "no job tolerates failure",
  );
});

test("a masked Windows smoke failure is reported from the step's real outcome", () => {
  const steps = loadWorkflow().job("build-windows").steps;
  const model = steps.findIndex((step) => step.id === "smoke_model");
  const report = steps.findIndex((step) => step.name === "Report masked smoke test failures");
  assert.ok(report > model, "the report follows the tolerated step");
  assert.equal(steps[report].condition, "${{ always() }}");
  assert.equal(steps[report].env.MODEL_OUTCOME, "${{ steps.smoke_model.outcome }}");
  const lines = commandLines(steps[report]).join("\n");
  assert.ok(lines.includes("GITHUB_STEP_SUMMARY") && lines.includes("::warning "), "the summary and a warning annotation");
});

// -- the script against a fake binary ----------------------------------------------------------

// A stand-in for `sagascript`: keeps `language` in a file, prints what the real CLI prints in the
// places the checks look. FAKE_FAIL names a behaviour to break.
const FAKE_BINARY = `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const state = path.join(process.env.FAKE_STATE, "language");
const args = process.argv.slice(2);
const fail = process.env.FAKE_FAIL;
const read = () => (fs.existsSync(state) ? fs.readFileSync(state, "utf8") : "en");
const [command, sub] = args;
if (command === "config" && sub === "set") fs.writeFileSync(state, args[3]);
else if (command === "config" && sub === "reset") fs.rmSync(state, { force: true });
else if (command === "config" && sub === "get") console.log(fail === "config" ? "xx" : read());
else if (command === "completions") process.stdout.write(fail === "completions" ? "" : "# " + sub + " completion\\n");
else if (command === "transcribe" && args.includes("--help")) console.log("transcribe help");
else if (command === "record" && args.includes("--help")) console.log("record help");
else if (command === "transcribe") {
  const input = args.at(-1);
  if (input.endsWith("missing.wav")) {
    console.log(JSON.stringify({ status: "error", code: "file_decode_error", error: "no such file" }));
    console.error("Error: Transcription failed");
    process.exit(1);
  } else if (args.includes("--jsonl")) {
    for (const name of ["a", "b"]) console.log(JSON.stringify({ status: "ok", source: name + ".mp3", result: { text: "Stortinget " + name } }));
  } else if (args.includes("--json")) {
    console.log(JSON.stringify({ text: "Stortinget har vedtatt", language: "no" }));
  } else {
    console.log("Stortinget har vedtatt");
  }
} else if (!["--version", "--help", "list-models", "formats", "config", "download-model"].includes(command)) {
  process.exit(2);
} else if (command === "config" && !["list", "path"].includes(sub)) {
  process.exit(2);
}
`;

const python = spawnSync(process.platform === "win32" ? "python" : "python3", ["--version"]).status === 0;

function withFakeBinary(callback) {
  const directory = mkdtempSync(join(tmpdir(), "sagascript-smoke-test-"));
  try {
    const binary = join(directory, "sagascript");
    writeFileSync(binary, FAKE_BINARY);
    chmodSync(binary, 0o755);
    return callback(binary, directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function runScript(tier, binary, directory, env = {}) {
  return spawnSync(process.execPath, [SCRIPT, tier, "--binary", binary], {
    encoding: "utf8",
    env: { ...process.env, FAKE_STATE: directory, RUNNER_TEMP: directory, ...env },
  });
}

test("both tiers pass against a fake binary that keeps the contract", { skip: process.platform === "win32" || !python }, () => {
  withFakeBinary((binary, directory) => {
    for (const tier of Object.keys(TIERS)) {
      const result = runScript(tier, binary, directory);
      assert.equal(result.status, 0, `${tier}: ${result.stdout}${result.stderr}`);
    }
  });
});

test(
  "the script exits 1 and names the check when the binary breaks the contract",
  { skip: process.platform === "win32" || !python },
  () => {
    withFakeBinary((binary, directory) => {
      for (const [fault, tier, check] of [
        ["config", "basic", "config management"],
        ["completions", "basic", "shell completions"],
      ]) {
        const result = runScript(tier, binary, directory, { FAKE_FAIL: fault });
        assert.equal(result.status, 1, `${fault}: ${result.stdout}${result.stderr}`);
        assert.match(result.stdout, new RegExp(`::error title=Smoke test failed::${check}`));
      }
      const missing = runScript("basic", join(directory, "no-such-binary"), directory);
      assert.equal(missing.status, 1);
      assert.match(missing.stdout, /the native release binary exists and is executable/);
      assert.equal(runScript("nonsense", binary, directory).status, 2);
    });
  },
);

test("the default binary is the native release build the lanes just made", () => {
  assert.equal(DEFAULT_BINARY.replaceAll("\\", "/").replace(/\.exe$/, ""), "src-tauri/target/release/sagascript");
});
