#!/usr/bin/env node
// CLI smoke tests of the native release binary, one script for every OS (#358).
// ci.yml's build-macos and build-windows lanes run it after `tauri build --no-bundle`
// instead of carrying one inline bash and one inline PowerShell copy per check.
//
//   node scripts/ci/smoke-cli.mjs basic   tier 1, no model: the binary exists, help, config,
//                                         list-models, formats, shell completions
//   node scripts/ci/smoke-cli.mjs model   tier 2: download nb-whisper-tiny, transcribe, and the
//                                         output contract of #364 with a real model: --json,
//                                         --jsonl, a reader that stops after the first item and
//                                         the error object of a failed --json run
//   --binary <path>                       default src-tauri/target/release/sagascript[.exe]
//
// Every check of a tier runs even after a failure, and the script exits 1 when any failed,
// so one run names all of them. The model tier is the one the Windows lane tolerates
// (continue-on-error, #357); that decision lives in ci.yml, not here.
import { spawn, spawnSync } from "node:child_process";
import { accessSync, constants, copyFileSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REPO_ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const WINDOWS = process.platform === "win32";
export const DEFAULT_BINARY = join("src-tauri", "target", "release", WINDOWS ? "sagascript.exe" : "sagascript");
const MODEL = "nb-whisper-tiny";
const SHORT_AUDIO = "test-audio/norwegian-short-3s.mp3";
const MEDIUM_AUDIO = "test-audio/norwegian-medium-8s.mp3";
const TRANSCRIBE = ["transcribe", "--language", "no", "--model", MODEL];

// Completions for every shell the binary supports on every OS, not one per OS.
export const COMPLETION_SHELLS = ["zsh", "bash", "powershell"];

const text = (value) => (typeof value === "string" ? value : "");

/**
 * Each check: { name, run(harness) }. The harness is what a check may do: run the
 * binary, write scratch files, run the stream verifier, and assert.
 */
export const TIERS = {
  basic: [
    {
      name: "the native release binary exists and is executable",
      run(h) {
        h.checkBinary();
      },
    },
    {
      name: "help and version",
      run(h) {
        for (const args of [["--version"], ["--help"], ["transcribe", "--help"], ["record", "--help"]]) h.ok(args);
      },
    },
    {
      name: "config management",
      run(h) {
        h.ok(["config", "list"]);
        h.ok(["config", "get", "language"]);
        h.ok(["config", "set", "language", "sv"]);
        h.assert(h.ok(["config", "get", "language"]).includes("sv"), "config get language should print sv after set");
        h.ok(["config", "reset", "language"]);
        h.assert(h.ok(["config", "get", "language"]).includes("en"), "config get language should print en after reset");
        h.ok(["config", "path"]);
      },
    },
    {
      name: "model listing and formats",
      run(h) {
        h.ok(["list-models"]);
        h.ok(["formats"]);
      },
    },
    {
      name: "shell completions",
      run(h) {
        for (const shell of COMPLETION_SHELLS) {
          h.assert(h.ok(["completions", shell], { quiet: true }).trim() !== "", `completions ${shell} printed nothing`);
        }
      },
    },
  ],
  model: [
    {
      name: "download model and transcribe",
      run(h) {
        h.ok(["download-model", MODEL]);
        const result = h.ok([...TRANSCRIBE, SHORT_AUDIO]);
        console.log(`Transcription: ${result.trim()}`);
        h.assert(/stortinget/i.test(result), "expected 'stortinget' in the transcription");
      },
    },
    {
      name: "transcribe with JSON output",
      run(h) {
        const { stdout, stderr } = h.capture("transcription-smoke", [...TRANSCRIBE, "--json", SHORT_AUDIO]);
        h.verifyStreams([], stdout, stderr);
      },
    },
    {
      name: "JSONL output",
      run(h) {
        const { stdout, stderr } = h.capture("transcription-smoke-jsonl", [...TRANSCRIBE, "--jsonl", h.audioDirectory()]);
        h.verifyStreams(["--jsonl"], stdout, stderr);
      },
    },
    {
      // A smoke check with the real model: a panic or a failing exit on a reader that stops after the
      // first item fails the check. The deterministic check of the closed-pipe contract is
      // `run_into_closed_pipe` in src-tauri/crates/sagascript-cli/tests/output_contract.rs.
      name: "closed pipe after the first JSONL item",
      async run(h) {
        const { code, sawLine } = await h.readFirstLineThenClose([...TRANSCRIBE, "--jsonl", h.audioDirectory()]);
        h.assert(sawLine, "no JSONL item before the output ended");
        h.assert(code === 0, `exited with status ${code} after the reader closed the pipe`);
      },
    },
    {
      name: "JSON error object of a failed run",
      run(h) {
        const { code, stdout, stderr } = h.capture("transcription-error", [...TRANSCRIBE, "--json", join(h.scratch, "missing.wav")]);
        h.assert(code === 1, `a failed --json run must exit 1, got ${code}`);
        h.verifyStreams(["--error"], stdout, stderr);
      },
    },
  ],
};

function createHarness({ binary, scratch }) {
  const harness = {
    binary,
    scratch,
    assert(condition, message) {
      if (!condition) throw new Error(message);
    },
    checkBinary() {
      harness.assert(existsSync(binary), `missing native release binary: ${binary}`);
      if (!WINDOWS) {
        accessSync(binary, constants.X_OK);
        harness.runProgram("file", [binary]);
      }
    },
    // `quiet` keeps a long stdout (a completion script) out of the log; stderr always shows.
    runProgram(program, args, { quiet = false } = {}) {
      console.log(`+ ${program} ${args.join(" ")}`);
      const result = spawnSync(program, args, { cwd: REPO_ROOT, encoding: "utf8" });
      if (result.error) throw result.error;
      if (!quiet) process.stdout.write(text(result.stdout));
      process.stderr.write(text(result.stderr));
      return { code: result.status, stdout: text(result.stdout), stderr: text(result.stderr) };
    },
    // Runs the binary; the exit status must be 0. Returns stdout.
    ok(args, options) {
      const result = harness.runProgram(binary, args, options);
      harness.assert(result.code === 0, `${args.join(" ")} exited with status ${result.code}`);
      return result.stdout;
    },
    // Runs the binary with stdout and stderr in files (as a verifier needs them). Any exit status is returned.
    capture(label, args) {
      const result = harness.runProgram(binary, args);
      const stdout = join(scratch, `${label}.stdout`);
      const stderr = join(scratch, `${label}.stderr`);
      writeFileSync(stdout, result.stdout);
      writeFileSync(stderr, result.stderr);
      return { code: result.code, stdout, stderr };
    },
    verifyStreams(mode, stdout, stderr) {
      const python = WINDOWS ? "python" : "python3";
      const result = harness.runProgram(python, ["scripts/verify-json-cli-streams.py", ...mode, stdout, stderr]);
      harness.assert(result.code === 0, `verify-json-cli-streams.py ${mode.join(" ")} exited with status ${result.code}`);
    },
    // Two audio files in a directory of their own: `transcribe` takes a directory.
    audioDirectory() {
      const inputs = join(scratch, "jsonl-inputs");
      if (!existsSync(inputs)) {
        mkdirSync(inputs, { recursive: true });
        copyFileSync(join(REPO_ROOT, SHORT_AUDIO), join(inputs, "a.mp3"));
        copyFileSync(join(REPO_ROOT, MEDIUM_AUDIO), join(inputs, "b.mp3"));
      }
      return inputs;
    },
    // `binary args | head -1` under pipefail: read one line, close the pipe, expect a clean exit.
    readFirstLineThenClose(args) {
      console.log(`+ ${binary} ${args.join(" ")} | head -1`);
      return new Promise((resolve, reject) => {
        const child = spawn(binary, args, { cwd: REPO_ROOT, stdio: ["ignore", "pipe", "inherit"] });
        let sawLine = false;
        child.stdout.on("data", (chunk) => {
          if (!sawLine && chunk.includes(10)) {
            sawLine = true;
            child.stdout.destroy();
          }
        });
        child.on("error", reject);
        child.on("close", (code) => resolve({ code, sawLine }));
      });
    },
  };
  return harness;
}

/** Runs every check of a tier, reporting each; resolves to the names of the failed ones. */
export async function runTier(tier, harness, { log = console.log } = {}) {
  const failed = [];
  for (const check of TIERS[tier]) {
    log(`::group::Smoke test: ${check.name}`);
    try {
      await check.run(harness);
      log("::endgroup::");
      log(`ok: ${check.name}`);
    } catch (error) {
      log("::endgroup::");
      log(`::error title=Smoke test failed::${check.name}: ${error.message}`);
      failed.push(check.name);
    }
  }
  return failed;
}

async function main(argv) {
  const tier = argv[0];
  if (!Object.hasOwn(TIERS, tier)) throw new Error(`usage: smoke-cli.mjs <${Object.keys(TIERS).join("|")}> [--binary <path>]`);
  const binaryIndex = argv.indexOf("--binary");
  const binary = resolve(REPO_ROOT, binaryIndex >= 0 ? argv[binaryIndex + 1] : DEFAULT_BINARY);
  const scratch = mkdtempSync(join(process.env.RUNNER_TEMP || tmpdir(), "sagascript-smoke-"));
  const failed = await runTier(tier, createHarness({ binary, scratch }));
  if (failed.length > 0) {
    console.error(`${failed.length} smoke check(s) failed in tier ${tier}: ${failed.join("; ")}`);
    return 1;
  }
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).then(
    (code) => process.exit(code),
    (error) => {
      console.error(error.message);
      process.exit(2);
    },
  );
}

// Re-exported for the tests that run the script against a fake binary.
export { createHarness };
