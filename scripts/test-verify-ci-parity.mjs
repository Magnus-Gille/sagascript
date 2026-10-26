import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { TEST_GROUPS } from "./node-test-groups.mjs";
import { CHECKS } from "./verify.mjs";
import { GATES, commandOf, gatesFor } from "./ci/cargo-gates.mjs";
import { expandedSteps, gateInvocation, parseWorkflow } from "./lib/ci-workflow.mjs";

// Keeps scripts/verify.mjs (the local check list, #350) and the test lanes of
// .github/workflows/ci.yml from drifting apart:
// - every ci.yml job is either a test lane or a listed non-test job;
// - every command in a test lane (named or nameless steps, any program, not
//   only cargo/npm/node) is covered by a verify check, the Node test-group
//   manifest, an allowlisted difference or a listed shell word that is not a
//   check (each with its reason);
// - every CI command a verify check claims exists in a test lane.
//
// What a lane runs is read from the parsed workflow (#358), through the two
// places the lanes keep commands outside their own `run:` scripts: the cargo
// gate list (a step running scripts/ci/cargo-gates.mjs stands for that OS's
// gates, in scripts/ci/cargo-gates.mjs) and the local setup action (a step
// using it stands for the action's `run` steps its inputs enable).
//
// Limits: environment set elsewhere (job `env:`, $GITHUB_ENV, e.g. the MSRV
// lane's RUSTUP_TOOLCHAIN or test-macos's DEVELOPER_DIR), `if:` conditions,
// third-party `uses:` actions, commands inside $(…), PowerShell syntax and
// shell control flow beyond splitting at `;`, `&&`, `||` and `|` are not
// compared.
//
// SAGASCRIPT_PARITY_CI_YML=<file> checks another copy of ci.yml (used to show
// the detections on mutated copies; never set in CI).

const ciPath = process.env.SAGASCRIPT_PARITY_CI_YML ?? fileURLToPath(new URL("../.github/workflows/ci.yml", import.meta.url));
const read = (path) => readFileSync(path, "utf8").replace(/\r\n?/g, "\n");
const workflow = parseWorkflow(read(ciPath));
const packageJson = read(fileURLToPath(new URL("../package.json", import.meta.url)));

const TEST_LANES = ["test-macos", "test-linux", "msrv-linux", "msrv-macos", "test-windows"];
const NON_TEST_JOBS = {
  scope: "classifies the changed paths so docs-only PRs skip the native lanes; runs no checks",
  "build-macos": "builds the native release binary and runs CLI smoke tests; verify does no release builds",
  "build-windows": "builds the native release binary and runs CLI smoke tests on Windows; same as build-macos",
  "check-macos": "aggregates the macOS lane results for branch protection; runs no checks",
  "check-linux": "aggregates the Linux lane results for branch protection; runs no checks",
  "check-windows": "aggregates the Windows lane results for branch protection; runs no checks",
};

// First words of simple commands that are not checks.
const NOT_COMMANDS = {
  set: "shell options",
  export: "environment for later commands",
  test: "assertions on variables and files the lane itself prepared",
  "[": "assertions on variables and files the lane itself prepared",
  echo: "log output",
  printf: "log output",
  exit: "control flow",
  fi: "control flow",
  else: "control flow",
  true: "control flow",
  false: "control flow",
  grep: "filters another command's output (the release guard's grep has its own test below)",
  tee: "copies another command's output to a log",
  sort: "filters another command's output",
  tail: "filters another command's output",
  head: "filters another command's output",
  find: "locates files (the newest Xcode)",
  sed: "reads values from files (the MSRV)",
};

// CI commands verify deliberately does not run, and why.
const ALLOWLIST = [
  { run: "npm ci", reason: "installs dependencies; verify never installs (its preflight names `npm ci`)" },
  { run: "npx playwright install chromium", reason: "installs a browser; the browser check's preflight names it" },
  {
    run: "cargo install --locked cargo-deny --version 0.20.2",
    reason: "installs a tool; the deny check's preflight names this command",
  },
  {
    run: "npm run build",
    reason: "builds dist/, a prerequisite: verify's preflight asks for it before the checks that compile the app crate",
  },
  {
    run: "python -m pip install -r scripts/diarization_sv/requirements-evaluation.txt",
    reason: "installs packages; the python-diarization preflight names this command",
  },
  { run: "sudo apt-get", prefix: true, reason: "installs system packages; the linux-libs preflight names them" },
  { run: "rustup toolchain install", prefix: true, reason: "installs the MSRV toolchain; the msrv preflight names it" },
  {
    run: "curl --fail --location --retry 2",
    prefix: true,
    reason: "downloads the third-party scorer for the DER/JER cross-check; CI-only network step",
  },
  {
    run: "python scripts/diarization_sv/crosscheck_metrics.py",
    prefix: true,
    reason: "needs the scorer the lane downloads with curl; CI-only network step",
  },
  { run: "swift --version", reason: "prints the selected toolchain for the CI log" },
];

// ---------------------------------------------------------------------------
// A small reader for the subset of shell that the lanes' run scripts use.

// Split a script into simple commands, honouring quotes and $(…): each is a
// list of words with leading VAR=value assignments and redirections removed.
function simpleCommands(script) {
  const commands = [];
  for (const line of script.replace(/\\\n\s*/g, " ").split("\n")) {
    if (line.trim().startsWith("#")) continue;
    let words = [];
    let word = "";
    let quote = null;
    let depth = 0;
    const endWord = () => {
      if (word !== "") words.push(word);
      word = "";
    };
    const endCommand = () => {
      endWord();
      if (words.length > 0) commands.push(words);
      words = [];
    };
    for (let index = 0; index < line.length; index += 1) {
      const char = line[index];
      if (quote) {
        word += char;
        if (char === quote) quote = null;
      } else if (char === "'" || char === '"') {
        quote = char;
        word += char;
      } else if (char === "$" && line[index + 1] === "(") {
        depth += 1;
        word += "$(";
        index += 1;
      } else if (depth > 0) {
        if (char === "(") depth += 1;
        if (char === ")") depth -= 1;
        word += char;
      } else if (/\s/.test(char)) {
        endWord();
      } else if (char === ";" || char === "|" || (char === "&" && line[index + 1] === "&")) {
        if (line[index + 1] === char) index += 1;
        endCommand();
      } else {
        word += char;
      }
    }
    endCommand();
  }
  return commands
    .map((words) => {
      const kept = [];
      for (let index = 0; index < words.length; index += 1) {
        const current = words[index];
        if (/^(\d?>>?|<|&>)$/.test(current)) {
          index += 1; // `> file`: the target goes too
        } else if (!/^(\d?>>?|<|&>)/.test(current)) {
          kept.push(current);
        }
      }
      while (kept.length > 0 && /^(if|then|elif|else|do|while|until|!)$/.test(kept[0])) kept.shift();
      while (kept.length > 0 && /^[A-Za-z_][A-Za-z0-9_]*=/.test(kept[0])) kept.shift();
      return kept;
    })
    .filter((words) => words.length > 0);
}

// The commands of a run script that need a verify counterpart.
function commandsIn(script) {
  return simpleCommands(script)
    .filter((words) => !Object.hasOwn(NOT_COMMANDS, words[0]))
    .map((words) => words.join(" "));
}

const OS_OF_RUNNER = (runsOn) => (/^macos/.test(runsOn) ? "macos" : /^windows/.test(runsOn) ? "windows" : "linux");

// The commands of a lane as { job, run, cwd, step }, gates and action steps expanded.
function laneCommands(jobId) {
  const job = workflow.job(jobId);
  return expandedSteps(job).flatMap((step) => {
    const invocation = gateInvocation(step);
    if (invocation) {
      const gates = gatesFor({ os: invocation.os ?? OS_OF_RUNNER(job.runsOn), profile: invocation.profile });
      return gates.map((gate) => ({ job: jobId, run: commandOf(gate), cwd: gate.cwd, step }));
    }
    return commandsIn(step.run).map((run) => ({ job: jobId, run, cwd: step.workingDirectory ?? ".", step }));
  });
}
const ciCommands = TEST_LANES.flatMap(laneCommands);
const key = ({ run, cwd }) => `${cwd ?? "."}: ${run}`;

const classified = new Set(Object.values(TEST_GROUPS).flat());
// `node [--test] <files…>` is covered when every file is in the manifest:
// verify runs every group (node-ui, node-ci, node-release, code-health).
function nodeTestFilesCovered(run) {
  const match = /^node (?:--test )?((?:scripts\/\S+\.mjs ?)+)$/.exec(run);
  return (
    Boolean(match) &&
    match[1]
      .trim()
      .split(" ")
      .every((file) => classified.has(file))
  );
}

function allowlisted(run) {
  return ALLOWLIST.find((entry) => (entry.prefix ? run.startsWith(entry.run) : run === entry.run));
}

// ---------------------------------------------------------------------------

test("the reader handles nameless steps, quoting, $(…), assignments and any program", () => {
  const synthetic = `name: x
on:
  push:
jobs:
  lane:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v6
      - run: cargo test -p nameless
      - name: Named
        working-directory: src-tauri
        env:
          QA_OUTPUT_DIR: /tmp/qa
        run: |
          set -o pipefail
          if cargo tree -p x -e normal | grep -qiE 'alsa|cpal'; then
            echo "cargo is fine" >&2; exit 1
          fi
          newest=$(find /Applications -name "Xcode_*.app" | sort -V | tail -1)
          RUSTUP_TOOLCHAIN=1.88 cargo check --locked 2>&1 | tee "$RUNNER_TEMP/log"
          bash scripts/lint.sh && make lint
          python scripts/a.py \\
            --binary "$X" > out.txt 2> err.txt
  other:
    steps:
      - run: echo hi
`;
  const parsed = parseWorkflow(synthetic);
  assert.deepEqual(parsed.jobIds, ["lane", "other"]);
  const steps = parsed.job("lane").steps;
  assert.deepEqual(
    steps.map((step) => [step.name, step.workingDirectory]),
    [
      [undefined, undefined],
      [undefined, undefined],
      ["Named", "src-tauri"],
    ],
  );
  assert.deepEqual(steps[2].env, { QA_OUTPUT_DIR: "/tmp/qa" });
  assert.deepEqual(commandsIn(steps[1].run), ["cargo test -p nameless"]);
  assert.deepEqual(commandsIn(steps[2].run), [
    "cargo tree -p x -e normal",
    "cargo check --locked",
    "bash scripts/lint.sh",
    "make lint",
    'python scripts/a.py --binary "$X"',
  ]);
});

test("every ci.yml job is a test lane or a listed non-test job", () => {
  const jobs = workflow.jobIds;
  const known = new Set([...TEST_LANES, ...Object.keys(NON_TEST_JOBS)]);
  assert.deepEqual(
    jobs.filter((job) => !known.has(job)),
    [],
    "add the job to TEST_LANES (its commands then need verify checks) or to NON_TEST_JOBS with the reason",
  );
  for (const job of known) assert.ok(jobs.includes(job), `${job} is listed but not in ci.yml`);
  for (const [job, reason] of Object.entries(NON_TEST_JOBS)) assert.ok(reason.length > 20, `${job} needs a reason`);
  for (const [word, reason] of Object.entries(NOT_COMMANDS)) assert.ok(reason.length > 5, `${word} needs a reason`);
});

test("the parser finds the commands of the CI test lanes", () => {
  const keys = new Set(ciCommands.map(key));
  for (const expected of [
    "src-tauri: cargo fmt --all -- --check",
    "src-tauri: cargo test --release -p sagascript-core --lib dev_overrides",
    "src-tauri/engine-host/ort: cargo test --locked",
    "src-tauri/engine-host/coreml: swift test",
    ".: npm run test:frontend",
    "src-tauri: cargo check --workspace --locked",
    "src-tauri: cargo check -p sagascript-core --features record --locked",
  ]) {
    assert.ok(keys.has(expected), `parsed ${expected}`);
  }
  for (const job of TEST_LANES)
    assert.ok(
      ciCommands.some((command) => command.job === job),
      `${job} has commands`,
    );
  // The commands the lanes take from the gate list and from the setup action are visible too.
  for (const expected of ["src-tauri: cargo test --workspace", ".: npm ci"]) assert.ok(keys.has(expected), `parsed ${expected}`);
  assert.ok(
    ciCommands.some((command) => command.job === "test-linux" && command.run.startsWith("sudo apt-get")),
    "the setup action's apt step",
  );
  assert.ok(
    ciCommands.some((command) => command.job === "msrv-linux" && command.run.startsWith("rustup toolchain install")),
    "the MSRV install",
  );
});

test("every gate of the list is run by some test lane, so the list holds no command the parity test cannot see", () => {
  const seen = new Set(ciCommands.map(key));
  for (const gate of GATES) assert.ok(seen.has(key({ run: commandOf(gate), cwd: gate.cwd })), `${gate.id} is run by no test lane`);
});

test("every command in the CI test lanes has a verify check, a manifest entry or an allowlisted reason", () => {
  const claimed = new Set(CHECKS.flatMap((check) => check.ci.map(key)));
  const uncovered = ciCommands.filter(
    (command) => !claimed.has(key(command)) && !nodeTestFilesCovered(command.run) && !allowlisted(command.run),
  );
  assert.deepEqual(
    uncovered.map((command) => `${command.job} [${command.cwd}] ${command.run}`),
    [],
    "add a check to scripts/verify.mjs (or its `ci` list), or an ALLOWLIST entry with the reason",
  );
});

test("every CI command a verify check claims exists in the CI test lanes", () => {
  const present = new Set(ciCommands.map(key));
  const stale = CHECKS.flatMap((check) =>
    check.ci.filter((command) => !present.has(key(command))).map((command) => `${check.id}: ${key(command)}`),
  );
  assert.deepEqual(stale, []);
});

test("each check's lanes are the ci.yml jobs that run its commands", () => {
  for (const check of CHECKS) {
    const jobs = [
      ...new Set(ciCommands.filter((command) => check.ci.some((claim) => key(claim) === key(command))).map((command) => command.job)),
    ];
    const declared = check.lanes.filter((lane) => !lane.endsWith(".yml"));
    assert.deepEqual(declared.sort(), jobs.sort(), `${check.id} lanes`);
  }
});

test("every allowlist entry still matches a CI command", () => {
  for (const entry of ALLOWLIST) {
    assert.ok(
      ciCommands.some((command) => allowlisted(command.run) === entry),
      `stale allowlist entry: ${entry.run}`,
    );
    assert.ok(entry.reason.length > 20, `${entry.run} needs a reason`);
  }
});

test("verify's Node checks run every test group, as npm run test:frontend does", () => {
  const scripts = JSON.parse(packageJson).scripts;
  assert.equal(scripts["test:frontend"], "node scripts/run-frontend-tests.mjs", "test:frontend runs every group");
  const groupsRun = new Set();
  for (const check of CHECKS) {
    for (const step of check.steps) {
      const group = step.argv?.[1] === "scripts/run-frontend-tests.mjs" ? step.argv[3] : null;
      if (group) groupsRun.add(group);
      if (step.run === "npm run test:code-health") groupsRun.add("codeHealth");
    }
  }
  assert.deepEqual([...groupsRun].sort(), Object.keys(TEST_GROUPS).sort());
});

test("workflows outside ci.yml named in a check's lanes run its command", () => {
  for (const check of CHECKS) {
    for (const lane of check.lanes.filter((name) => name.endsWith(".yml"))) {
      const other = read(fileURLToPath(new URL(`../.github/workflows/${lane}`, import.meta.url)));
      for (const step of check.steps) assert.ok(other.includes(`run: ${step.run}`), `${lane} runs ${step.run}`);
    }
  }
});

test("the release guard proves its test ran, locally as in CI", () => {
  // cargo test exits 0 when the name filter matches nothing.
  const proof = GATES.find((gate) => gate.id === "release-guard").requireOutput?.pattern;
  assert.ok(proof, "the gate list requires the release guard's output");
  const [step] = CHECKS.find((check) => check.id === "release-guard").steps;
  assert.ok(
    step.requireOutput?.pattern.test("release_build_ignores_the_real_environment ... ok"),
    "verify's release-guard requires the same line",
  );
  assert.ok(proof.test("test tests::release_build_ignores_the_real_environment ... ok"));
  assert.deepEqual(
    [proof.source, step.requireOutput.pattern.source],
    [proof.source, proof.source],
    "verify and CI require the same output",
  );
});

test("the browser check gives the QA scripts an output directory, as CI does", () => {
  const ciStep = expandedSteps(workflow.job("test-macos")).find((step) => commandsIn(step.run).includes("npm run test:browser"));
  assert.ok(ciStep?.env.QA_OUTPUT_DIR, "test-macos sets QA_OUTPUT_DIR for npm run test:browser");
  const [step] = CHECKS.find((check) => check.id === "browser").steps;
  assert.ok(step.env?.QA_OUTPUT_DIR, "verify's browser check sets QA_OUTPUT_DIR");
});
