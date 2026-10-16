#!/usr/bin/env node
// The one list of cargo gates of CI (#358). Every lane of .github/workflows/ci.yml
// runs `node scripts/ci/cargo-gates.mjs --os <macos|linux|windows>` instead of
// writing the cargo commands out per OS, and the MSRV lane runs it with
// `--profile msrv`. scripts/test-ci-cargo-gates.mjs and
// scripts/test-verify-ci-parity.mjs read GATES from here, so the list cannot
// drift between the OS lanes, `npm run verify` (scripts/verify.mjs) and the tests.
//
// A gate runs on every OS unless it has an `os` list, and every `os` list says
// why in `why`: that is the ONE place the per-OS exclusions are documented.
// The rule: a gate whose subject is platform independent (source formatting, a
// Cargo feature subset, a dependency-graph assertion, a build-profile guard)
// runs once, on Linux, the cheapest runner. Everything that executes OS-specific
// code (the workspace tests and lints, the lean batch CLI, the ONNX host where
// it ships) runs on every OS it applies to.
//
// Why a plain `cargo clippy --workspace` / `cargo test --workspace` instead of a
// bare `cargo test` plus `-p sagascript-cli`: the workspace gate is a superset
// (app, CLI with its default features, protocol crate, test support) and, with
// resolver 2, builds core with exactly the features the app and CLI default to.
// The feature subsets below it (core alone, core with one feature, the lean CLI)
// stay separate invocations: combining package flags would unify their features.
// `cargo check` is not listed for the main profile because `cargo clippy
// --all-targets` compiles everything `cargo check` would, and more strictly.
//
//   node scripts/ci/cargo-gates.mjs --os linux            run, stop at the first failure
//   node scripts/ci/cargo-gates.mjs --os linux --keep-going   run all, fail at the end
//   node scripts/ci/cargo-gates.mjs --os windows --list   print the commands, run nothing
//   node scripts/ci/cargo-gates.mjs --profile msrv        needs RUSTUP_TOOLCHAIN (the lane sets it)
import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REPO_ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
export const OSES = ["macos", "linux", "windows"];
export const PROFILES = ["main", "msrv"];
const OS_OF_PLATFORM = { darwin: "macos", linux: "linux", win32: "windows" };

const APP = "src-tauri";
const ORT = "src-tauri/engine-host/ort";
const LINUX = ["linux"];
const gate = (id, cwd, args, extra = {}) => ({ id, cwd, args, profile: "main", ...extra });

// The release-profile guard exists only without debug assertions, and cargo
// test exits 0 when its name filter matches nothing: prove the test ran.
export const RELEASE_GUARD_PROOF = /release_build_ignores_the_real_environment \.\.\. ok/;

/**
 * Each gate: { id, cwd (repository relative), args (cargo arguments), profile,
 * os? + why? (restriction and its reason), forbidOutput?/requireOutput?
 * ({ pattern, message }), quiet? (show the output only on failure) }.
 */
export const GATES = [
  // Dependency policy (#355), before any compilation: RustSec advisories, licenses, banned or duplicated
  // crates and allowed sources, as set in src-tauri/deny.toml, for both Cargo workspaces. The advisory
  // database is fetched at run time. The lane installs cargo-deny first (ci.yml pins the version); the ORT
  // host run allows only the policy entries the main workspace alone uses.
  gate("deny", APP, ["deny", "--locked", "--workspace", "check"], {
    os: LINUX,
    why: "advisories, licenses and sources are properties of the lockfiles, not of the OS; deny.toml already judges every shipped target in one run",
  }),
  gate(
    "deny-ort",
    APP,
    [
      ...["deny", "--locked", "--manifest-path", "engine-host/ort/Cargo.toml", "--workspace", "check"],
      ...["-A", "license-not-encountered", "-A", "license-exception-not-encountered", "-A", "unmatched-source"],
    ],
    { os: LINUX, why: "same as deny, for the ORT host's own lockfile" },
  ),
  // Formatting next, so an unformatted PR fails before anything compiles.
  gate("fmt", APP, ["fmt", "--all", "--", "--check"], {
    os: LINUX,
    why: "rustfmt reads source text only; the result is identical on every OS (#352)",
  }),
  gate("fmt-ort", APP, ["fmt", "--manifest-path", "engine-host/ort/Cargo.toml", "--all", "--", "--check"], {
    os: LINUX,
    why: "rustfmt reads source text only; the standalone ORT host is its own Cargo workspace (#352)",
  }),

  gate("clippy-workspace", APP, ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]),
  gate("test-workspace", APP, ["test", "--workspace"]),

  // Feature subsets. Separate invocations preserve the no-features builds.
  gate("clippy-core", APP, ["clippy", "-p", "sagascript-core", "--all-targets", "--", "-D", "warnings"]),
  gate("test-core", APP, ["test", "-p", "sagascript-core"]),
  gate("clippy-cli-lean", APP, ["clippy", "-p", "sagascript-cli", "--all-targets", "--no-default-features", "--", "-D", "warnings"]),
  gate("test-cli-lean", APP, ["test", "-p", "sagascript-cli", "--no-default-features"]),
  gate(
    "clippy-core-diarization",
    APP,
    ["clippy", "-p", "sagascript-core", "--features", "diarization", "--all-targets", "--", "-D", "warnings"],
    {
      os: LINUX,
      why: "core's `diarization` and `record` features alone are a Cargo feature-graph property, not OS code; the workspace gates already build and test core with both on every OS",
    },
  ),
  gate("test-core-diarization", APP, ["test", "-p", "sagascript-core", "--features", "diarization"], {
    os: LINUX,
    why: "same as clippy-core-diarization: a Cargo feature subset, not OS code",
  }),
  gate("test-core-record", APP, ["test", "-p", "sagascript-core", "--features", "record"], {
    os: LINUX,
    why: "same as clippy-core-diarization (needs the ALSA headers the Linux lane installs)",
  }),

  // The ONNX Runtime engine host is a separate Cargo workspace (#357).
  gate("clippy-ort", ORT, ["clippy", "--all-targets", "--locked", "--", "-D", "warnings"], {
    os: ["linux", "windows"],
    why: "the ORT host ships for Windows and Linux; macOS ships the Core ML host (Swift tests, test-macos)",
  }),
  gate("test-ort", ORT, ["test", "--locked"], {
    os: ["linux", "windows"],
    why: "the ORT host ships for Windows and Linux; macOS ships the Core ML host (Swift tests, test-macos)",
  }),

  // The debug lean CLI the offline diarization evaluation tests drive
  // (SAGASCRIPT_REFERENCE_CLI=target/debug/sagascript). test-cli-lean built it already.
  gate("build-cli-lean", APP, ["build", "-p", "sagascript-cli", "--no-default-features"], {
    os: LINUX,
    why: "only the Linux lane runs the Python diarization suite that drives this binary",
  }),
  // Regression guard for the no-ALSA property (#66): the lean CLI's dependency tree
  // holds neither cpal nor alsa.
  gate("no-alsa", APP, ["tree", "-p", "sagascript-cli", "--no-default-features", "-e", "normal"], {
    os: LINUX,
    why: "a dependency-graph assertion; cargo resolves the same graph for cpal on every OS, ALSA itself only on Linux",
    quiet: true,
    forbidOutput: { pattern: /alsa|cpal/i, message: "cpal/ALSA leaked into the batch-only dependency tree" },
  }),
  // Release-profile guard (#357): a release build ignores SAGASCRIPT_ENGINE_HOST and
  // SAGASCRIPT_PIANISSIMO_MODEL_DIR. Compiled only without debug_assertions and
  // the `dev-overrides` feature, so every debug test run leaves it out.
  gate("release-guard", APP, ["test", "--release", "-p", "sagascript-core", "--lib", "dev_overrides"], {
    os: LINUX,
    why: "a build-profile property of core, not OS code; one release compile is enough",
    requireOutput: { pattern: RELEASE_GUARD_PROOF, message: "the release-profile guard test did not run" },
  }),

  // Minimum supported Rust version (#353): `rust-version` is a promise to people
  // building from source. The lane exports RUSTUP_TOOLCHAIN=<rust-version>.
  gate("msrv-workspace", APP, ["check", "--workspace", "--locked"], { profile: "msrv" }),
  gate("msrv-cli-lean", APP, ["check", "-p", "sagascript-cli", "--no-default-features", "--locked"], { profile: "msrv" }),
  gate("msrv-ort", ORT, ["check", "--locked"], { profile: "msrv" }),
];

export const commandOf = (item) => ["cargo", ...item.args].join(" ");

/** The gates of one lane, in run order. */
export function gatesFor({ os, profile = "main" }) {
  if (!OSES.includes(os)) throw new Error(`unknown OS ${JSON.stringify(os)}; expected one of ${OSES.join(", ")}`);
  if (!PROFILES.includes(profile)) throw new Error(`unknown profile ${JSON.stringify(profile)}; expected one of ${PROFILES.join(", ")}`);
  return GATES.filter((item) => item.profile === profile && (item.os === undefined || item.os.includes(os)));
}

/**
 * Runs the gates through `execute(gate) -> { code, output }`, stopping at the
 * first failure unless `keepGoing`. A gate also fails when the output it must
 * not contain appears, or the output it must contain is absent.
 */
export async function runGates(gates, { execute, keepGoing = false, log = () => {} }) {
  const results = [];
  for (const item of gates) {
    const started = Date.now();
    log(`::group::${commandOf(item)}  (${item.cwd})`);
    const { code, output } = await execute(item);
    let failure = code === 0 ? null : `exited with status ${code}`;
    if (failure === null && item.forbidOutput?.pattern.test(output)) failure = item.forbidOutput.message;
    if (failure === null && item.requireOutput && !item.requireOutput.pattern.test(output)) failure = item.requireOutput.message;
    if (failure !== null && item.quiet) log(output);
    log("::endgroup::");
    if (failure !== null) log(`::error title=Cargo gate failed::${commandOf(item)} (${item.cwd}) ${failure}`);
    results.push({ gate: item, failure, seconds: (Date.now() - started) / 1000 });
    if (failure !== null && !keepGoing) break;
  }
  return results;
}

function executeWithCargo(item) {
  return new Promise((resolve, reject) => {
    const watched = item.forbidOutput !== undefined || item.requireOutput !== undefined;
    const child = spawn("cargo", item.args, { cwd: join(REPO_ROOT, item.cwd), stdio: watched ? ["inherit", "pipe", "pipe"] : "inherit" });
    let output = "";
    for (const [stream, sink] of [
      [child.stdout, process.stdout],
      [child.stderr, process.stderr],
    ]) {
      stream?.on("data", (chunk) => {
        output += chunk;
        if (!item.quiet) sink.write(chunk);
      });
    }
    child.on("error", reject);
    child.on("close", (code, signal) => resolve({ code: code ?? (signal ? 128 : 1), output }));
  });
}

function parseArguments(argv) {
  const options = { os: OS_OF_PLATFORM[process.platform], profile: "main", list: false, keepGoing: false, explicitOs: false };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--os" || argument === "--profile") {
      const value = argv[(index += 1)];
      if (value === undefined) throw new Error(`${argument} needs a value`);
      if (argument === "--os") Object.assign(options, { os: value, explicitOs: true });
      else options.profile = value;
    } else if (argument === "--list") options.list = true;
    else if (argument === "--keep-going") options.keepGoing = true;
    else throw new Error(`unknown argument ${argument}`);
  }
  return options;
}

async function main(argv) {
  const options = parseArguments(argv);
  const gates = gatesFor(options);
  if (options.list) {
    for (const item of gates) console.log(`${item.cwd}: ${commandOf(item)}`);
    return 0;
  }
  // A lane that names another OS would silently run the wrong list.
  if (options.explicitOs && OS_OF_PLATFORM[process.platform] !== options.os) {
    throw new Error(`refusing to run the ${options.os} gates on a ${OS_OF_PLATFORM[process.platform] ?? process.platform} runner`);
  }
  if (options.profile === "msrv" && !process.env.RUSTUP_TOOLCHAIN) {
    throw new Error(
      "the msrv profile needs RUSTUP_TOOLCHAIN set to the declared rust-version; without it cargo would use the development toolchain",
    );
  }
  const results = await runGates(gates, { execute: executeWithCargo, keepGoing: options.keepGoing, log: console.log });
  const skipped = gates.slice(results.length);
  const lines = [
    `Cargo gates (${options.os}, ${options.profile}): ${results.length - results.filter((entry) => entry.failure).length} passed, ` +
      `${results.filter((entry) => entry.failure).length} failed, ${skipped.length} not run`,
    "",
    "| Gate | Result | Seconds |",
    "|---|---|---|",
    ...results.map(
      (entry) => `| \`${commandOf(entry.gate)}\` (${entry.gate.cwd}) | ${entry.failure ?? "passed"} | ${entry.seconds.toFixed(0)} |`,
    ),
    ...skipped.map((item) => `| \`${commandOf(item)}\` (${item.cwd}) | not run (an earlier gate failed) | |`),
  ];
  console.log(lines.join("\n"));
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${lines.join("\n")}\n`);
  return results.some((entry) => entry.failure) ? 1 : 0;
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
