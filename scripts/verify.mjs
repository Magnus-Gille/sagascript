// `npm run verify`: the local quality checks, defined once (#350).
//
// CHECKS below is the source of truth for what "verify my change" means:
// CONTRIBUTING.md documents it, `--list` prints it, and
// scripts/test-verify-ci-parity.mjs keeps it in step with the test lanes of
// .github/workflows/ci.yml. Before anything runs, a preflight checks the
// tools and packages the selected checks need and reports everything that is
// missing in one error; it never installs anything. Every check has a time
// budget; on timeout, Ctrl-C or SIGTERM its whole process tree is stopped.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  formatBudget,
  formatDuration,
  runProcessTree,
  signalExitCode,
  trapTerminationSignals,
} from "./lib/process-tree.mjs";
import { findPackageDirectory } from "./lib/node-packages.mjs";

export const REPO_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));

const ALL = ["darwin", "linux", "win32"];
const MACOS = ["darwin"];
const POSIX = ["darwin", "linux"];
const MINUTE = 60_000;
const PLATFORM_NAMES = { darwin: "macOS", linux: "Linux", win32: "Windows" };


const step = (run, cwd, extra = {}) => ({ run, cwd, ...extra });
const budgetMs = (check) => check.timeoutMs ?? check.timeoutMinutes * MINUTE;
const ci = (run, cwd) => ({ run, cwd });
const nodeTests = (group) =>
  step(`npm run test:${group}`, undefined, { argv: ["node", "scripts/run-frontend-tests.mjs", "--group", group] });

/**
 * Each check:
 *   id, area      names for --only/--skip (an area selects all its checks)
 *   steps         commands run in order:
 *                 { run, cwd, argv?, env?, forbidOutput?, requireOutput? }.
 *                 `run` is what a human would type (and what --list prints);
 *                 `argv` overrides how it is spawned (e.g. no npm/npx shims).
 *   sets          "default" (npm run verify), "fast" (npm run verify:fast);
 *                 none = opt-in, run only through --only
 *   platforms     where it can run; elsewhere it is reported as skipped
 *   requires      prerequisites (PREREQUISITES keys, or "npm:<package>")
 *   timeoutMinutes  budget for all steps together
 *   ci            CI test-lane commands this check runs or subsumes, as
 *                 { run, cwd } exactly as ci.yml writes them
 *   lanes         the ci.yml jobs (or other workflows) that run them
 */
export const CHECKS = [
  {
    id: "fmt",
    area: "fmt",
    title: "rustfmt, main workspace",
    steps: [step("cargo fmt --all -- --check", "src-tauri")],
    sets: ["default", "fast"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 5,
    ci: [ci("cargo fmt --all -- --check", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "fmt-ort",
    area: "fmt",
    title: "rustfmt, ORT engine host workspace",
    steps: [step("cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check", "src-tauri")],
    sets: ["default", "fast"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 5,
    ci: [ci("cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "svelte-check",
    area: "svelte-check",
    title: "Svelte and TypeScript types",
    steps: [step("npx svelte-check --tsconfig ./tsconfig.json")],
    sets: ["default", "fast"],
    platforms: ALL,
    requires: ["npm:svelte-check", "npm:svelte", "npm:typescript"],
    timeoutMinutes: 10,
    ci: [ci("npx svelte-check --tsconfig ./tsconfig.json")],
    lanes: ["test-macos"],
  },
  {
    id: "node-ui",
    area: "node",
    title: "Node tests, ui group (frontend logic)",
    steps: [nodeTests("ui")],
    sets: ["default", "fast"],
    platforms: ALL,
    requires: ["npm:typescript"],
    timeoutMinutes: 15,
    ci: [ci("npm run test:frontend")],
    lanes: ["test-macos", "test-windows"],
  },
  {
    id: "node-ci",
    area: "node",
    title: "Node tests, ci group (workflows and tooling)",
    steps: [nodeTests("ci")],
    sets: ["default"],
    platforms: ALL,
    requires: [],
    timeoutMinutes: 15,
    ci: [ci("npm run test:frontend")],
    lanes: ["test-macos", "test-windows"],
  },
  {
    id: "node-release",
    area: "node",
    title: "Node tests, release group (release, updater, packaging)",
    steps: [nodeTests("release")],
    sets: ["default"],
    platforms: ALL,
    requires: [],
    timeoutMinutes: 15,
    ci: [ci("npm run test:frontend")],
    lanes: ["test-macos", "test-windows"],
  },
  {
    id: "code-health",
    area: "code-health",
    title: "Code-health contracts (codeHealth group and tests/scripts)",
    steps: [step("npm run test:code-health")],
    sets: ["default"],
    platforms: ALL,
    requires: [],
    timeoutMinutes: 10,
    ci: [ci("npm run test:frontend")],
    lanes: ["test-macos", "test-windows", "code-health.yml"],
  },
  {
    id: "python-dictation-eval",
    area: "python",
    title: "Python, offline dictation evaluation tooling",
    steps: [step("python -m unittest discover -s scripts/dictation_eval -p 'test_*.py'")],
    sets: ["default"],
    platforms: ALL,
    requires: ["python"],
    timeoutMinutes: 10,
    ci: [ci("python -m unittest discover -s scripts/dictation_eval -p 'test_*.py'")],
    lanes: ["test-macos", "test-linux", "test-windows"],
  },
  {
    id: "python-diarization-reference",
    area: "python",
    title: "Python, diarization reference contracts (no extra packages)",
    steps: [step("python -m unittest discover -s scripts/diarization_sv -p 'test_reference_dataset.py'")],
    sets: ["default"],
    platforms: POSIX,
    requires: ["python"],
    timeoutMinutes: 10,
    ci: [ci("python -m unittest discover -s scripts/diarization_sv -p 'test_reference_dataset.py'")],
    lanes: ["test-macos"],
  },
  {
    id: "clippy-workspace",
    area: "clippy",
    title: "Clippy, whole workspace (app, core, CLI, protocol)",
    steps: [step("cargo clippy --workspace --all-targets -- -D warnings", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust", "linux-libs", "frontend-dist"],
    timeoutMinutes: 45,
    ci: [
      ci("cargo clippy --all-targets -- -D warnings", "src-tauri"),
      ci("cargo clippy -p sagascript-cli --all-targets -- -D warnings", "src-tauri"),
      ci("cargo check", "src-tauri"),
      ci("cargo check -p sagascript-cli", "src-tauri"),
    ],
    lanes: ["test-macos", "test-linux", "test-windows"],
  },
  {
    id: "clippy-app",
    area: "clippy",
    title: "Clippy, app crate only (inner loop; the workspace check covers it)",
    steps: [step("cargo clippy --all-targets -- -D warnings", "src-tauri")],
    sets: ["fast"],
    platforms: ALL,
    requires: ["rust", "linux-libs", "frontend-dist"],
    timeoutMinutes: 45,
    ci: [ci("cargo clippy --all-targets -- -D warnings", "src-tauri")],
    lanes: ["test-macos", "test-linux", "test-windows"],
  },
  {
    id: "clippy-cli-lean",
    area: "clippy",
    title: "Clippy, lean batch CLI (no default features)",
    steps: [step("cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [
      ci("cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings", "src-tauri"),
      ci("cargo check -p sagascript-cli --no-default-features", "src-tauri"),
    ],
    lanes: ["test-linux"],
  },
  {
    id: "cli-lean-deps",
    area: "rust-deps",
    title: "Lean batch CLI pulls in no cpal/ALSA (#66)",
    steps: [
      step("cargo tree -p sagascript-cli --no-default-features -e normal", "src-tauri", {
        forbidOutput: { pattern: /alsa|cpal/i, message: "cpal/ALSA leaked into the batch-only dependency tree" },
      }),
    ],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 5,
    ci: [ci("cargo tree -p sagascript-cli --no-default-features -e normal", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "core-clippy",
    area: "core-features",
    title: "Clippy, sagascript-core without features",
    steps: [step("cargo clippy -p sagascript-core --all-targets -- -D warnings", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo clippy -p sagascript-core --all-targets -- -D warnings", "src-tauri")],
    lanes: ["test-macos", "test-windows"],
  },
  {
    id: "core-clippy-diarization",
    area: "core-features",
    title: "Clippy, sagascript-core with diarization",
    steps: [step("cargo clippy -p sagascript-core --features diarization --all-targets -- -D warnings", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo clippy -p sagascript-core --features diarization --all-targets -- -D warnings", "src-tauri")],
    lanes: ["test-macos"],
  },
  {
    id: "test-workspace",
    area: "rust-test",
    title: "Rust tests, whole workspace",
    steps: [step("cargo test --workspace", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust", "linux-libs", "frontend-dist"],
    timeoutMinutes: 60,
    ci: [ci("cargo test", "src-tauri"), ci("cargo test -p sagascript-cli", "src-tauri")],
    lanes: ["test-macos", "test-windows"],
  },
  {
    id: "test-cli-lean",
    area: "rust-test",
    title: "Rust tests, lean batch CLI (no default features)",
    steps: [step("cargo test -p sagascript-cli --no-default-features", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo test -p sagascript-cli --no-default-features", "src-tauri")],
    lanes: ["test-macos", "test-linux"],
  },
  {
    id: "engine-protocol",
    area: "engine-protocol",
    title: "Engine-host wire format (sagascript-engine-protocol)",
    steps: [step("cargo test -p sagascript-engine-protocol", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 15,
    ci: [ci("cargo test -p sagascript-engine-protocol", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "core-test",
    area: "core-features",
    title: "Rust tests, sagascript-core without features",
    steps: [step("cargo test -p sagascript-core", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo test -p sagascript-core", "src-tauri")],
    lanes: ["test-macos", "test-linux", "test-windows"],
  },
  {
    id: "core-test-record",
    area: "core-features",
    title: "Rust tests, sagascript-core with record",
    steps: [step("cargo test -p sagascript-core --features record", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust", "linux-libs"],
    timeoutMinutes: 30,
    ci: [ci("cargo test -p sagascript-core --features record", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "core-test-diarization",
    area: "core-features",
    title: "Rust tests, sagascript-core with diarization",
    steps: [step("cargo test -p sagascript-core --features diarization", "src-tauri")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [
      ci("cargo test -p sagascript-core --features diarization", "src-tauri"),
      ci("cargo check -p sagascript-core --features diarization", "src-tauri"),
    ],
    lanes: ["test-macos"],
  },
  {
    id: "ort-host-clippy",
    area: "ort-host",
    title: "Clippy, ONNX Runtime engine host (separate workspace)",
    steps: [step("cargo clippy --all-targets --locked -- -D warnings", "src-tauri/engine-host/ort")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo clippy --all-targets --locked -- -D warnings", "src-tauri/engine-host/ort")],
    lanes: ["test-linux"],
  },
  {
    id: "ort-host-test",
    area: "ort-host",
    title: "Rust tests, ONNX Runtime engine host",
    steps: [step("cargo test --locked", "src-tauri/engine-host/ort")],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 30,
    ci: [ci("cargo test --locked", "src-tauri/engine-host/ort")],
    lanes: ["test-linux"],
  },
  {
    id: "release-guard",
    area: "release-guard",
    title: "Release-profile guard: release builds ignore dev overrides",
    steps: [
      step("cargo test --release -p sagascript-core --lib dev_overrides", "src-tauri", {
        // cargo test passes when the filter matches nothing; CI greps for this too.
        requireOutput: {
          pattern: /release_build_ignores_the_real_environment \.\.\. ok/,
          message: "the release-profile guard test did not run",
        },
      }),
    ],
    sets: ["default"],
    platforms: ALL,
    requires: ["rust"],
    timeoutMinutes: 45,
    ci: [ci("cargo test --release -p sagascript-core --lib dev_overrides", "src-tauri")],
    lanes: ["test-linux"],
  },
  {
    id: "swift",
    area: "swift",
    title: "Swift Core ML engine host (generates BuildInfo.swift first)",
    steps: [step("./scripts/build-engine-host.sh"), step("swift test", "src-tauri/engine-host/coreml")],
    sets: ["default"],
    platforms: MACOS,
    requires: ["swift"],
    timeoutMinutes: 30,
    ci: [ci("swift test", "src-tauri/engine-host/coreml")],
    lanes: ["test-macos"],
  },
  {
    id: "python-diarization",
    area: "python",
    title: "Python, full diarization suite with native CLI parity (needs evaluation packages)",
    steps: [
      step("cargo build -p sagascript-cli --no-default-features", "src-tauri"),
      step("python -m unittest discover -s scripts/diarization_sv -p 'test_*.py'", undefined, {
        env: { SAGASCRIPT_REFERENCE_CLI: "{cargoTargetDir}/debug/sagascript" },
      }),
    ],
    sets: [],
    platforms: POSIX,
    requires: ["rust", "python", "python-eval-packages"],
    timeoutMinutes: 30,
    ci: [
      ci("cargo build -p sagascript-cli --no-default-features", "src-tauri"),
      ci("python -m unittest discover -s scripts/diarization_sv -p 'test_*.py'"),
    ],
    lanes: ["test-linux"],
  },
  {
    id: "msrv",
    area: "msrv",
    title: "Compile on the declared minimum Rust version (rust-version)",
    steps: [
      step("cargo check --workspace --locked", "src-tauri", { env: { RUSTUP_TOOLCHAIN: "{msrvToolchain}" } }),
      step("cargo check -p sagascript-cli --no-default-features --locked", "src-tauri", {
        env: { RUSTUP_TOOLCHAIN: "{msrvToolchain}" },
      }),
      step("cargo check --locked", "src-tauri/engine-host/ort", { env: { RUSTUP_TOOLCHAIN: "{msrvToolchain}" } }),
    ],
    sets: [],
    platforms: ALL,
    requires: ["msrv-toolchain", "linux-libs", "frontend-dist"],
    timeoutMinutes: 60,
    ci: [
      ci("cargo check --workspace --locked", "src-tauri"),
      ci("cargo check -p sagascript-cli --no-default-features --locked", "src-tauri"),
      ci("cargo check --locked", "src-tauri/engine-host/ort"),
    ],
    lanes: ["msrv-linux"],
  },
  {
    id: "browser",
    area: "browser",
    title: "Playwright UI flows against the mocked IPC (Transcribe tab, update recovery)",
    steps: [step("npm run test:browser", undefined, { argv: ["node", "scripts/run-transcription-browser-tests.mjs"] })],
    sets: [],
    platforms: ALL,
    requires: ["npm:vite", "npm:playwright", "playwright-chromium"],
    timeoutMinutes: 15,
    ci: [ci("npm run test:browser")],
    lanes: ["test-macos"],
  },
];

// ---------------------------------------------------------------------------
// Prerequisites. A probe returns { ok: true, note? } or
// { ok: false, missing, fix }. Probes only look; they never install.

function quiet(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    windowsHide: true,
    timeout: 30_000,
    ...options,
    env: { ...process.env, RUSTUP_AUTO_INSTALL: "0", ...options.env },
  });
  return { ok: !result.error && result.status === 0, stdout: result.stdout ?? "", stderr: result.stderr ?? "", error: result.error };
}

function readText(root, path) {
  try {
    return readFileSync(join(root, path), "utf8");
  } catch {
    return "";
  }
}

function rustupToolchains(ctx) {
  if (ctx.rustupToolchains === undefined) {
    const listed = quiet("rustup", ["toolchain", "list"], { cwd: ctx.root });
    ctx.rustupToolchains = listed.ok
      ? listed.stdout
          .split(/\r?\n/)
          .map((line) => line.trim().split(/\s+/)[0])
          .filter(Boolean)
      : null;
  }
  return ctx.rustupToolchains;
}

function installedToolchain(toolchains, channel) {
  return toolchains.find((name) => name === channel || name.startsWith(`${channel}-`)) ?? null;
}

function probeRust(ctx) {
  const pinned = /^\s*channel\s*=\s*"([^"]+)"/m.exec(readText(ctx.root, "rust-toolchain.toml"))?.[1];
  const channel = process.env.RUSTUP_TOOLCHAIN || pinned;
  const toolchains = rustupToolchains(ctx);
  if (toolchains === null) {
    if (quiet("cargo", ["--version"]).ok) {
      return {
        ok: true,
        note: `rustup not found: using the cargo on PATH instead of the toolchain pinned in rust-toolchain.toml${pinned ? ` (${pinned})` : ""}`,
      };
    }
    return {
      ok: false,
      missing: "Rust (rustup and cargo)",
      fix: "install rustup from https://rustup.rs; rust-toolchain.toml then selects the toolchain",
    };
  }
  if (!channel) return { ok: true };
  const toolchain = installedToolchain(toolchains, channel);
  if (!toolchain) {
    return {
      ok: false,
      missing: `Rust toolchain ${channel} (rust-toolchain.toml)`,
      fix: `rustup toolchain install ${channel} --profile minimal --component clippy,rustfmt`,
    };
  }
  const components = quiet("rustup", ["component", "list", "--installed", "--toolchain", toolchain]);
  const missing = ["clippy", "rustfmt"].filter(
    (name) => !components.stdout.split(/\r?\n/).some((line) => line.startsWith(`${name}-`) || line === name),
  );
  if (components.ok && missing.length > 0) {
    return {
      ok: false,
      missing: `Rust components ${missing.join(", ")} for ${channel}`,
      fix: `rustup component add ${missing.join(" ")} --toolchain ${channel}`,
    };
  }
  return { ok: true };
}

function probeMsrvToolchain(ctx) {
  const msrv = /^rust-version\s*=\s*"([^"]+)"/m.exec(readText(ctx.root, "src-tauri/Cargo.toml"))?.[1];
  if (!msrv) return { ok: false, missing: "rust-version in src-tauri/Cargo.toml", fix: "restore the rust-version key" };
  const toolchains = rustupToolchains(ctx);
  if (toolchains === null) {
    return { ok: false, missing: `rustup (to select Rust ${msrv})`, fix: "install rustup from https://rustup.rs" };
  }
  const toolchain = toolchains.find((name) => name === msrv || name.startsWith(`${msrv}-`) || name.startsWith(`${msrv}.`));
  if (!toolchain) {
    return {
      ok: false,
      missing: `Rust ${msrv} (rust-version, the MSRV)`,
      fix: `rustup toolchain install ${msrv} --profile minimal`,
    };
  }
  ctx.values.msrvToolchain = toolchain;
  return { ok: true };
}

function probeLinuxLibs(ctx) {
  if (ctx.platform !== "linux") return { ok: true };
  const fix =
    "sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev " +
    "libasound2-dev build-essential pkg-config file (see CONTRIBUTING.md for other distributions)";
  const pkgConfig = quiet("pkg-config", ["--exists", "webkit2gtk-4.1", "alsa"]);
  if (pkgConfig.ok) return { ok: true };
  return {
    ok: false,
    missing: pkgConfig.error
      ? "pkg-config and the Tauri/ALSA development libraries"
      : "Tauri/ALSA development libraries (pkg-config cannot find webkit2gtk-4.1 or alsa)",
    fix,
  };
}

// tauri::generate_context! embeds ../dist, so every cargo command that
// compiles the app crate fails without it. verify never builds it itself.
function probeFrontendDist(ctx) {
  if (existsSync(join(ctx.root, "dist", "index.html"))) return { ok: true };
  return {
    ok: false,
    missing: "built frontend dist/index.html (the app crate embeds it via tauri::generate_context!)",
    fix: "npm run build",
  };
}

function pythonCandidates(platform) {
  return platform === "win32" ? ["python", "python3"] : ["python3", "python"];
}

function probePython(ctx) {
  for (const candidate of pythonCandidates(ctx.platform)) {
    const result = quiet(candidate, ["-c", "import sys; print(sys.version_info[0])"]);
    if (result.ok && result.stdout.trim() === "3") {
      ctx.values.python = candidate;
      return { ok: true };
    }
  }
  return {
    ok: false,
    missing: "Python 3 (python3 or python on PATH; CI uses 3.12)",
    fix: "install Python 3.12 from https://www.python.org/downloads/ (macOS: brew install python@3.12)",
  };
}

const EVALUATION_MODULES = [
  "numpy",
  "scipy",
  "soundfile",
  "python_speech_features",
  "pyannote.core",
  "pyannote.database",
  "pyannote.metrics",
];

function probePythonEvalPackages(ctx) {
  const python = ctx.values.python ?? pythonCandidates(ctx.platform)[0];
  const script = [
    "import importlib.util, sys",
    `names = ${JSON.stringify(EVALUATION_MODULES)}`,
    "missing = []",
    "for name in names:",
    "    try:",
    "        found = importlib.util.find_spec(name) is not None",
    "    except ModuleNotFoundError:",
    "        found = False",
    "    if not found:",
    "        missing.append(name)",
    "print(' '.join(missing))",
  ].join("\n");
  const result = quiet(python, ["-c", script]);
  const missing = result.ok ? result.stdout.trim() : EVALUATION_MODULES.join(" ");
  if (result.ok && missing === "") return { ok: true };
  return {
    ok: false,
    missing: `Python evaluation packages (${missing.split(" ").join(", ")})`,
    fix: `${python} -m pip install -r scripts/diarization_sv/requirements-evaluation.txt (in a virtualenv)`,
  };
}

function probeSwift(ctx) {
  if (ctx.platform !== "darwin") return { ok: true };
  const fix = "install Xcode 16 or newer and select it: sudo xcode-select -s /Applications/Xcode.app";
  // xcode-select -p never prompts; a bare `swift` without developer tools
  // would open the Command Line Tools installer.
  if (!quiet("xcode-select", ["-p"]).ok) {
    return { ok: false, missing: "Xcode (Swift 6 for Swift Testing)", fix };
  }
  const version = quiet("swift", ["--version"]);
  const major = Number(/Swift version (\d+)/.exec(version.stdout + version.stderr)?.[1]);
  if (!version.ok || !(major >= 6)) {
    return { ok: false, missing: "Swift 6 or newer (Xcode 16+, for `import Testing`)", fix };
  }
  return { ok: true };
}

function probePlaywrightChromium(ctx) {
  const fix = "npx playwright install chromium";
  const directory = findPackageDirectory(ctx.root, "playwright");
  try {
    const playwright = directory && createRequire(import.meta.url)(directory);
    if (playwright && existsSync(playwright.chromium.executablePath())) return { ok: true };
  } catch {
    // Reported below.
  }
  return { ok: false, missing: "Playwright's Chromium browser", fix };
}

export const PREREQUISITES = {
  rust: { label: "Rust toolchain", probe: probeRust },
  "msrv-toolchain": { label: "MSRV toolchain", probe: probeMsrvToolchain },
  "linux-libs": { label: "Linux system libraries", probe: probeLinuxLibs },
  "frontend-dist": { label: "built frontend (dist/)", probe: probeFrontendDist },
  python: { label: "Python 3", probe: probePython },
  "python-eval-packages": { label: "Python evaluation packages", probe: probePythonEvalPackages },
  swift: { label: "Swift toolchain", probe: probeSwift },
  "playwright-chromium": { label: "Playwright Chromium", probe: probePlaywrightChromium },
};

function npmPackageResolvable(root, name) {
  return findPackageDirectory(root, name) !== null;
}

/**
 * Probe every prerequisite the checks need. Returns { missing: [...], notes: [...] }
 * where each missing entry is { missing, fix, neededBy }.
 */
export async function preflight(checks, { root, platform, prerequisites, values }) {
  const ctx = { root, platform, values };
  const neededBy = new Map();
  for (const check of checks) {
    for (const requirement of check.requires) {
      if (!neededBy.has(requirement)) neededBy.set(requirement, []);
      neededBy.get(requirement).push(check.id);
    }
  }

  const missing = [];
  const notes = [];
  const npmMissing = [];
  for (const [requirement, users] of neededBy) {
    if (requirement.startsWith("npm:")) {
      const name = requirement.slice(4);
      if (!npmPackageResolvable(root, name)) npmMissing.push({ name, users });
      continue;
    }
    const prerequisite = prerequisites[requirement];
    if (!prerequisite) throw new Error(`check requires unknown prerequisite "${requirement}"`);
    const result = await prerequisite.probe(ctx);
    if (result.note) notes.push(result.note);
    if (!result.ok) missing.push({ missing: result.missing ?? prerequisite.label, fix: result.fix, neededBy: users });
  }
  if (npmMissing.length > 0) {
    missing.unshift({
      missing: `npm packages ${npmMissing.map(({ name }) => name).join(", ")} (node_modules)`,
      fix: "npm ci",
      neededBy: [...new Set(npmMissing.flatMap(({ users }) => users))],
    });
  }
  return { missing, notes };
}

export function formatPreflightError(missing) {
  const lines = [
    `verify: preflight failed: ${missing.length} prerequisite${missing.length === 1 ? " is" : "s are"} missing. ` +
      "Nothing was installed and no check ran.",
    "",
  ];
  missing.forEach((item, index) => {
    lines.push(`  ${index + 1}. ${item.missing}`);
    lines.push(`     needed by: ${item.neededBy.join(", ")}`);
    lines.push(`     fix:       ${item.fix}`);
  });
  const skip = [...new Set(missing.flatMap((item) => item.neededBy))].join(",");
  lines.push("", `Fix them and run verify again, or leave those checks out with --skip ${skip}`);
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Command line.

const USAGE = `Usage: npm run verify [-- options]   (or: node scripts/verify.mjs [options])

Runs the local quality checks defined in scripts/verify.mjs, in order, after a
preflight that lists every missing prerequisite at once (it installs nothing).

  (no option)        every "default" check that applies to this OS; stops at
                     the first failure
  --fast             the inner-loop set (npm run verify:fast)
  --only a,b         just these checks or areas (also opt-in checks)
  --skip a,b         leave out these checks or areas
  --keep-going       run every selected check even after a failure
  --list             print the checks, commands, timeouts and CI lanes
  --help             this text

Exit status: 0 all selected checks passed or were skipped, 1 a check failed or
timed out, 2 usage or preflight error, 128+n interrupted by signal n.`;

class UsageError extends Error {}

function splitList(value, flag) {
  const items = value.split(",").map((item) => item.trim()).filter(Boolean);
  if (items.length === 0) throw new UsageError(`${flag} needs at least one check or area`);
  return items;
}

export function parseArgs(argv) {
  const options = { only: null, skip: [], keepGoing: false, list: false, fast: false, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const equals = arg.startsWith("--") ? arg.indexOf("=") : -1;
    const flag = equals === -1 ? arg : arg.slice(0, equals);
    const value = () => {
      const next = equals === -1 ? argv[++index] : arg.slice(equals + 1);
      if (next === undefined || next.startsWith("--")) throw new UsageError(`${flag} needs a value`);
      return next;
    };
    switch (flag) {
      case "--only":
        options.only = [...(options.only ?? []), ...splitList(value(), flag)];
        break;
      case "--skip":
        options.skip.push(...splitList(value(), flag));
        break;
      case "--keep-going":
        options.keepGoing = true;
        break;
      case "--list":
        options.list = true;
        break;
      case "--fast":
        options.fast = true;
        break;
      case "--help":
      case "-h":
        options.help = true;
        break;
      default:
        throw new UsageError(`unknown argument ${arg}`);
    }
  }
  if (options.fast && options.only) throw new UsageError("--fast and --only cannot be combined");
  return options;
}

function resolveNames(names, checks, flag) {
  const ids = new Set();
  const unknown = [];
  for (const name of names) {
    const matches = checks.filter((check) => check.id === name || check.area === name);
    if (matches.length === 0) unknown.push(name);
    for (const check of matches) ids.add(check.id);
  }
  if (unknown.length > 0) {
    const areas = [...new Set(checks.map((check) => check.area))].join(", ");
    throw new UsageError(
      `${flag}: unknown check or area ${unknown.join(", ")}.\n` +
        `Checks: ${checks.map((check) => check.id).join(", ")}\nAreas: ${areas}`,
    );
  }
  return ids;
}

/**
 * The checks a run covers, in CHECKS order, each with `skipReason` set when it
 * will not run here, plus the opt-in checks left out.
 */
export function selectChecks(checks, options, platform) {
  let chosen;
  if (options.only) {
    const ids = resolveNames(options.only, checks, "--only");
    chosen = checks.filter((check) => ids.has(check.id));
  } else {
    const set = options.fast ? "fast" : "default";
    chosen = checks.filter((check) => check.sets.includes(set));
  }
  const skipped = resolveNames(options.skip, checks, "--skip");
  const optIn = options.only || options.fast ? [] : checks.filter((check) => check.sets.length === 0);
  return {
    selected: chosen.map((check) => {
      if (skipped.has(check.id)) return { check, skipReason: "--skip" };
      if (!check.platforms.includes(platform)) {
        return { check, skipReason: `${check.platforms.map((name) => PLATFORM_NAMES[name]).join("/")} only` };
      }
      return { check, skipReason: null };
    }),
    optIn: optIn.filter((check) => !skipped.has(check.id)),
  };
}

function describeSets(check) {
  return check.sets.length > 0 ? check.sets.join(",") : "opt-in";
}

function describePlatforms(check) {
  return check.platforms.length === ALL.length ? "all" : check.platforms.map((name) => PLATFORM_NAMES[name]).join("/");
}

function stepLine(stepDefinition) {
  return `${stepDefinition.run}  [${stepDefinition.cwd ?? "."}]`;
}

export function formatList(checks, platform) {
  const width = Math.max(...checks.map((check) => check.id.length)) + 2;
  const areaWidth = Math.max(...checks.map((check) => check.area.length)) + 2;
  const lines = [
    `Checks defined in scripts/verify.mjs, in run order (this OS: ${PLATFORM_NAMES[platform] ?? platform}).`,
    '`npm run verify` runs the "default" set, `npm run verify:fast` the "fast" set; opt-in checks run with --only.',
    "",
    `${"CHECK".padEnd(width)}${"AREA".padEnd(areaWidth)}${"SET".padEnd(14)}${"OS".padEnd(14)}${"BUDGET".padEnd(8)}CI`,
  ];
  for (const check of checks) {
    lines.push(
      `${check.id.padEnd(width)}${check.area.padEnd(areaWidth)}${describeSets(check).padEnd(14)}` +
        `${describePlatforms(check).padEnd(14)}${formatBudget(budgetMs(check)).padEnd(8)}` +
        `${check.lanes.length > 0 ? check.lanes.join(", ") : "-"}`,
    );
    lines.push(`${"".padEnd(width)}${check.title}`);
    if (check.requires.length > 0) lines.push(`${"".padEnd(width)}needs: ${check.requires.join(", ")}`);
    for (const stepDefinition of check.steps) lines.push(`${"".padEnd(width)}$ ${stepLine(stepDefinition)}`);
  }
  lines.push("", `Areas (for --only/--skip): ${[...new Set(checks.map((check) => check.area))].join(", ")}`);
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Running.

function cargoTargetDir(root) {
  const result = quiet("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: join(root, "src-tauri"),
    maxBuffer: 64 * 1024 * 1024,
  });
  if (!result.ok) throw new Error(`cargo metadata failed: ${result.stderr.trim() || result.error?.message}`);
  return JSON.parse(result.stdout).target_directory;
}

function substitute(value, ctx) {
  return value.replace(/\{(\w+)\}/g, (_, name) => {
    if (name === "cargoTargetDir") {
      ctx.values.cargoTargetDir ??= cargoTargetDir(ctx.root);
      return ctx.values.cargoTargetDir;
    }
    if (ctx.values[name] === undefined) throw new Error(`no value for {${name}}`);
    return ctx.values[name];
  });
}

/** `run` as argv: whitespace-separated, with '…' and "…" quoting. */
export function splitCommand(run) {
  const argv = [];
  const pattern = /'([^']*)'|"([^"]*)"|(\S+)/g;
  let match;
  while ((match = pattern.exec(run)) !== null) argv.push(match[1] ?? match[2] ?? match[3]);
  return argv;
}

// `npx <name>` without npx: the package's own bin script, run with node, so
// a missing package can never trigger a download.
function npxBin(root, name) {
  const directory = findPackageDirectory(root, name);
  if (!directory) throw new Error(`${name} is not installed; run \`npm ci\``);
  const manifest = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.[name];
  if (!bin) throw new Error(`${name} declares no "${name}" bin`);
  return join(directory, bin);
}

// How to spawn a step, without npm/npx shims (no download, no shell) where
// possible.
function spawnPlan(stepDefinition, ctx) {
  const [program, ...rest] = (stepDefinition.argv ?? splitCommand(stepDefinition.run)).map((part) =>
    substitute(part, ctx),
  );
  if (program === "node") return { command: process.execPath, args: rest };
  if (program === "python") return { command: ctx.values.python ?? pythonCandidates(ctx.platform)[0], args: rest };
  if (program === "npx") {
    const [bin, ...args] = rest;
    return { command: process.execPath, args: [npxBin(ctx.root, bin), ...args] };
  }
  if (program === "npm") {
    const npmCli = process.env.npm_execpath;
    if (npmCli && /\.[cm]?js$/.test(npmCli)) return { command: process.execPath, args: [npmCli, ...rest] };
    if (ctx.platform === "win32") return { command: "npm", args: rest, shell: true };
    return { command: "npm", args: rest };
  }
  if (program.startsWith("./")) return { command: join(ctx.root, program), args: rest };
  return { command: program, args: rest };
}

function describeFailure(result, budget) {
  if (result.abortedBy) return `interrupted (${result.abortedBy}); process tree stopped`;
  if (result.timedOut) return `timed out after ${formatBudget(budget)}; process tree stopped`;
  if (result.error) return `could not start: ${result.error.message}`;
  if (result.code === null) return `killed by ${result.signal}`;
  return `exit code ${result.code}`;
}

async function runCheck(check, ctx, io) {
  const budget = budgetMs(check);
  const deadline = Date.now() + budget;
  const started = Date.now();
  const notes = [];
  for (const [index, stepDefinition] of check.steps.entries()) {
    const label = check.steps.length > 1 ? `step ${index + 1}/${check.steps.length} (${stepDefinition.run}): ` : "";
    if (check.steps.length > 1) io.out(`  $ ${stepLine(stepDefinition)}`);
    let plan;
    let env;
    try {
      plan = spawnPlan(stepDefinition, ctx);
      env = { ...process.env, ...ctx.env };
      for (const [key, value] of Object.entries(stepDefinition.env ?? {})) env[key] = substitute(value, ctx);
    } catch (error) {
      return { status: "fail", detail: `${label}${error.message}`, durationMs: Date.now() - started };
    }
    const result = await runProcessTree({
      ...plan,
      cwd: join(ctx.root, stepDefinition.cwd ?? "."),
      env,
      timeoutMs: Math.max(0, deadline - Date.now()),
      killGraceMs: ctx.killGraceMs,
      abortSignal: ctx.abortSignal,
      captureStdout: Boolean(stepDefinition.forbidOutput || stepDefinition.requireOutput),
    });
    if (result.leftovers) notes.push("stopped processes it left running");
    if (result.abortedBy) {
      return { status: "interrupted", detail: `${label}${describeFailure(result, budget)}`, durationMs: Date.now() - started };
    }
    if (result.timedOut || result.error || result.code !== 0) {
      return { status: "fail", detail: `${label}${describeFailure(result, budget)}`, durationMs: Date.now() - started };
    }
    if (stepDefinition.requireOutput) {
      io.out(result.stdout.trimEnd());
      if (!stepDefinition.requireOutput.pattern.test(result.stdout)) {
        return { status: "fail", detail: `${label}${stepDefinition.requireOutput.message}`, durationMs: Date.now() - started };
      }
    }
    if (stepDefinition.forbidOutput) {
      const offending = result.stdout.split(/\r?\n/).filter((line) => stepDefinition.forbidOutput.pattern.test(line));
      if (offending.length > 0) {
        io.err(`${stepDefinition.forbidOutput.message}:\n${offending.join("\n")}`);
        return { status: "fail", detail: `${label}${stepDefinition.forbidOutput.message}`, durationMs: Date.now() - started };
      }
    }
  }
  return { status: "pass", detail: notes.join("; "), durationMs: Date.now() - started };
}

const STATUS_LABELS = { pass: "pass", fail: "FAIL", skip: "skip", "not-run": "not run", interrupted: "INTERRUPTED" };

export function formatSummary(results, { platform, totalMs, optIn }) {
  const width = Math.max(5, ...results.map((result) => result.check.id.length)) + 2;
  const lines = [
    "",
    `verify summary (${PLATFORM_NAMES[platform] ?? platform}, ${results.length} checks, ${formatDuration(totalMs)})`,
    `${"CHECK".padEnd(width)}${"RESULT".padEnd(13)}${"TIME".padEnd(9)}DETAIL`,
  ];
  for (const result of results) {
    lines.push(
      `${result.check.id.padEnd(width)}${STATUS_LABELS[result.status].padEnd(13)}` +
        `${(result.durationMs === undefined ? "-" : formatDuration(result.durationMs)).padEnd(9)}${result.detail ?? ""}`.trimEnd(),
    );
  }
  const counts = ["pass", "fail", "skip", "not-run", "interrupted"]
    .map((status) => [status, results.filter((result) => result.status === status).length])
    .filter(([, count]) => count > 0)
    .map(([status, count]) => `${count} ${STATUS_LABELS[status].toLowerCase()}`);
  lines.push(counts.join(", "));
  if (optIn.length > 0) {
    lines.push(`Opt-in, not in this run: ${optIn.map((check) => check.id).join(", ")} (run with --only <id>)`);
  }
  return lines.join("\n");
}

async function loadConfig() {
  const configPath = process.env.SAGASCRIPT_VERIFY_CONFIG;
  if (!configPath) return { checks: CHECKS, prerequisites: PREREQUISITES, root: REPO_ROOT, killGraceMs: 5000 };
  // Test hook: scripts/test-verify-runner.mjs points this at synthetic checks.
  const config = await import(pathToFileURL(resolve(configPath)).href);
  return {
    checks: config.checks.map((check) => ({
      title: check.id,
      area: check.id,
      sets: ["default"],
      platforms: ALL,
      requires: [],
      ci: [],
      lanes: [],
      timeoutMinutes: 10,
      ...check,
    })),
    prerequisites: { ...PREREQUISITES, ...config.prerequisites },
    root: config.root ?? REPO_ROOT,
    killGraceMs: config.killGraceMs ?? 5000,
  };
}

export async function main(argv, io = { out: (line) => console.log(line), err: (line) => console.error(line) }) {
  let options;
  let config;
  let selection;
  const platform = process.platform;
  try {
    options = parseArgs(argv);
    if (options.help) {
      io.out(USAGE);
      return 0;
    }
    config = await loadConfig();
    if (options.list) {
      io.out(formatList(config.checks, platform));
      return 0;
    }
    selection = selectChecks(config.checks, options, platform);
  } catch (error) {
    if (!(error instanceof UsageError)) throw error;
    io.err(`verify: ${error.message}\n\n${USAGE}`);
    return 2;
  }

  const runnable = selection.selected.filter((entry) => entry.skipReason === null).map((entry) => entry.check);
  const values = {};
  const { missing, notes } = await preflight(runnable, {
    root: config.root,
    platform,
    prerequisites: config.prerequisites,
    values,
  });
  if (missing.length > 0) {
    io.err(formatPreflightError(missing));
    return 2;
  }
  for (const note of notes) io.out(`verify: note: ${note}`);

  const controller = new AbortController();
  const untrap = trapTerminationSignals((signal) => {
    if (!controller.signal.aborted) controller.abort(signal);
  });
  const ctx = {
    root: config.root,
    platform,
    values,
    killGraceMs: config.killGraceMs,
    abortSignal: controller.signal,
    // rust-toolchain.toml must never pull a toolchain or component in
    // behind the user's back; the preflight already confirmed they exist.
    env: process.env.RUSTUP_AUTO_INSTALL === undefined ? { RUSTUP_AUTO_INSTALL: "0" } : {},
  };

  const started = Date.now();
  const results = [];
  let stopReason = null;
  const total = runnable.length;
  let position = 0;
  for (const { check, skipReason } of selection.selected) {
    if (skipReason) {
      results.push({ check, status: "skip", detail: skipReason });
      continue;
    }
    if (controller.signal.aborted) {
      results.push({ check, status: "not-run", detail: `interrupted by ${controller.signal.reason}` });
      continue;
    }
    if (stopReason) {
      results.push({ check, status: "not-run", detail: stopReason });
      continue;
    }
    position += 1;
    io.out(`\n==> [${position}/${total}] ${check.id}: ${check.title} (budget ${formatBudget(budgetMs(check))})`);
    if (check.steps.length === 1) io.out(`  $ ${stepLine(check.steps[0])}`);
    const result = await runCheck(check, ctx, io);
    results.push({ check, ...result });
    if (result.status === "fail" && !options.keepGoing) {
      stopReason = `not run: stopped after ${check.id} failed (--keep-going runs the rest)`;
    }
  }
  untrap();

  io.out(formatSummary(results, { platform, totalMs: Date.now() - started, optIn: selection.optIn }));
  if (controller.signal.aborted) return signalExitCode(controller.signal.reason);
  return results.some((result) => result.status === "fail") ? 1 : 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  // No top-level await: a config or manifest module may import this module.
  main(process.argv.slice(2)).then((code) => {
    process.exitCode = code;
  });
}
