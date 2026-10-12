import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { TEST_GROUPS } from "./node-test-groups.mjs";
import { CHECKS } from "./verify.mjs";

// Keeps scripts/verify.mjs (the local check list, #350) and the test lanes of
// .github/workflows/ci.yml from drifting apart until CI calls verify itself
// (#358): every test or lint command in those lanes is covered by a verify
// check, an explicitly allowlisted difference, or (for Node test files) the
// test-group manifest; every CI command a check claims exists in ci.yml.

const read = async (path) =>
  (await readFile(new URL(`../${path}`, import.meta.url), "utf8")).replace(/\r\n?/g, "\n");
const [workflow, packageJson] = await Promise.all([read(".github/workflows/ci.yml"), read("package.json")]);

const TEST_LANES = ["test-macos", "test-linux", "msrv-linux", "test-windows"];
const TOOLS = /^(?:cargo|npm|npx|node|python3?|swift|\.\/scripts\/\S+)(?:\s|$)/;

// CI commands verify deliberately does not run, and why.
const ALLOWLIST = [
  { run: "npm ci", reason: "installs dependencies; verify never installs (its preflight names `npm ci`)" },
  { run: "npx playwright install chromium", reason: "installs a browser; the browser check's preflight names it" },
  {
    run: "npm run build",
    reason: "builds dist/, a prerequisite: verify's preflight asks for it before the checks that compile the app crate",
  },
  {
    run: "python -m pip install -r scripts/diarization_sv/requirements-evaluation.txt",
    reason: "installs packages; the python-diarization preflight names this command",
  },
  {
    run: "python scripts/diarization_sv/crosscheck_metrics.py",
    prefix: true,
    reason: "needs a third-party scorer the lane downloads with curl; CI-only network step",
  },
  {
    run: "./scripts/stage-engine-host.sh",
    reason: "stages the arm64 release engine host for bundling (packaging); the swift check builds it with build-engine-host.sh",
  },
  {
    run: "./scripts/check-engine-host-identity.sh",
    prefix: true,
    reason: "checks the staged host carries $GITHUB_SHA; belongs to the packaging step above",
  },
  { run: "swift --version", reason: "prints the selected toolchain for the CI log" },
];

function jobBlock(jobId) {
  const start = workflow.indexOf(`\n  ${jobId}:\n`);
  assert.notEqual(start, -1, `ci.yml must define ${jobId}`);
  const body = workflow.slice(start + 1);
  const next = body.slice(1).search(/^ {2}[a-z][a-z0-9-]*:$/m);
  return next === -1 ? body : body.slice(0, next + 1);
}

// Steps of a job as { run, cwd } with `run` the raw script.
function steps(jobId) {
  return jobBlock(jobId)
    .split(/\n(?= {6}- name: )/)
    .slice(1)
    .map((raw) => {
      const inline = /^ {8}run: (?![|>])(.*)$/m.exec(raw)?.[1];
      const block = /^ {8}run: [|>]-?\n((?: {10}.*\n?|\n)*)/m
        .exec(raw)?.[1]
        ?.split("\n")
        .map((line) => line.replace(/^ {10}/, ""))
        .join("\n");
      return { run: inline ?? block ?? "", cwd: /^ {8}working-directory: (.*)$/m.exec(raw)?.[1] };
    })
    .filter((step) => step.run.trim() !== "");
}

// The tool invocations in a run script: each simple command (split at
// `&&`, `||`, `;` and `|`) that starts with a tool we check, with
// redirections dropped. `echo` lines and comments are ignored.
function commandsIn(script) {
  const commands = [];
  for (const line of script.replace(/\\\n\s*/g, " ").split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#") || trimmed.startsWith("echo ")) continue;
    for (let segment of trimmed.split(/\s*(?:&&|\|\||;|\|)\s*/)) {
      segment = segment.replace(/^(?:if|then|!)\s+/, "").replace(/\s+\d?>.*$/, "").trim();
      if (TOOLS.test(segment)) commands.push(segment.replace(/\s+/g, " "));
    }
  }
  return commands;
}

const ciCommands = TEST_LANES.flatMap((job) =>
  steps(job).flatMap((step) => commandsIn(step.run).map((run) => ({ job, run, cwd: step.cwd ?? "." }))),
);
const key = ({ run, cwd }) => `${cwd ?? "."}: ${run}`;

const classified = new Set(Object.values(TEST_GROUPS).flat());
// `node [--test] <files…>` is covered when every file is in the manifest:
// verify runs every group (node-ui, node-ci, node-release, code-health).
function nodeTestFilesCovered(run) {
  const match = /^node (?:--test )?((?:scripts\/\S+\.mjs ?)+)$/.exec(run);
  return Boolean(match) && match[1].trim().split(" ").every((file) => classified.has(file));
}

function allowlisted(run) {
  return ALLOWLIST.find((entry) => (entry.prefix ? run.startsWith(entry.run) : run === entry.run));
}

test("the parser finds the commands of the CI test lanes", () => {
  assert.deepEqual(commandsIn("if cargo tree -p x -e normal | grep -qiE 'alsa|cpal'; then\n  echo cargo is fine\nfi"), [
    "cargo tree -p x -e normal",
  ]);
  assert.deepEqual(commandsIn("python scripts/a.py \\\n  --binary \"$X\" > out.txt 2> err.txt"), [
    'python scripts/a.py --binary "$X"',
  ]);
  const keys = new Set(ciCommands.map(key));
  for (const expected of [
    "src-tauri: cargo fmt --all -- --check",
    "src-tauri: cargo test --release -p sagascript-core --lib dev_overrides",
    "src-tauri/engine-host/ort: cargo test --locked",
    "src-tauri/engine-host/coreml: swift test",
    ".: npm run test:frontend",
    "src-tauri: cargo check --workspace --locked",
  ]) {
    assert.ok(keys.has(expected), `parsed ${expected}`);
  }
  for (const job of TEST_LANES) assert.ok(ciCommands.some((command) => command.job === job), `${job} has commands`);
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
  const stale = CHECKS.flatMap((check) => check.ci.filter((command) => !present.has(key(command))).map((command) => `${check.id}: ${key(command)}`));
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
    assert.ok(ciCommands.some((command) => allowlisted(command.run) === entry), `stale allowlist entry: ${entry.run}`);
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

test("workflows outside ci.yml named in a check's lanes run its command", async () => {
  for (const check of CHECKS) {
    for (const lane of check.lanes.filter((name) => name.endsWith(".yml"))) {
      const other = await read(`.github/workflows/${lane}`);
      for (const step of check.steps) assert.ok(other.includes(`run: ${step.run}`), `${lane} runs ${step.run}`);
    }
  }
});

test("the release guard proves its test ran, locally as in CI", () => {
  // cargo test exits 0 when the name filter matches nothing.
  const proof = /grep -q '(release_build_ignores_the_real_environment \.\.\. ok)'/.exec(workflow)?.[1];
  assert.ok(proof, "test-linux greps the release guard's output");
  const [step] = CHECKS.find((check) => check.id === "release-guard").steps;
  assert.ok(step.requireOutput?.pattern.test(proof), "verify's release-guard requires the same line");
});
