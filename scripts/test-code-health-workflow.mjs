import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  assertCleanCheckout,
  KNIP_ARGS,
  parseProducerOptions,
  runSwiftCoverageSteps,
  safeFailureDiagnostic,
  workflowRunContext,
} from "./code-health-produce.mjs";
import { createObjective } from "./lib/code-health-producer.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(join(root, ".github/workflows/code-health.yml"), "utf8");
const config = JSON.parse(readFileSync(join(root, "docs/code-health-producer-v1.json"), "utf8"));
const tooling = JSON.parse(readFileSync(join(root, "tooling/code-health/package.json"), "utf8"));
const toolingLock = JSON.parse(readFileSync(join(root, "tooling/code-health/package-lock.json"), "utf8"));

test("producer modes separate weekly static collection from CI-only snapshots", () => {
  assert.deepEqual(parseProducerOptions(["--mode", "static", "--output-dir", "/tmp/report"]).mode, "static");
  assert.deepEqual(parseProducerOptions(["--mode", "ci-only", "--output-dir", "/tmp/report"]).mode, "ci-only");
  assert.throws(() => parseProducerOptions(["--mode", "everything"]), /--mode must be static or ci-only/);
  assert.throws(() => parseProducerOptions(["--mode", "ci-only", "--ci"]), /cannot be combined/);
  assert.deepEqual(parseProducerOptions(["--ci"]).includeCi, true);

  const ciOnly = createObjective(config.scopes.find(scope => scope.name === "rust-core"), {
    commitSha: "a".repeat(40), observedAt: "2026-10-09T00:00:00Z", runRef: "ref:collection-run", attempt: 1,
  });
  assert.equal(ciOnly.metrics.complex_functions.status, "unknown");
  assert.equal(ciOnly.metrics.complex_functions.reason, "not-collected");
  assert.equal(ciOnly.metrics.coverage.status, "unknown");
  assert.equal(ciOnly.metrics.coverage.reason, "not-collected");
});

test("static GitHub collection binds exact repo, checkout, run and attempt without a token", () => {
  const commitSha = "b".repeat(40);
  const env = {
    GITHUB_ACTIONS: "true",
    GITHUB_REPOSITORY: `${config.repository.owner}/${config.repository.name}`,
    GITHUB_SHA: commitSha,
    GITHUB_WORKSPACE: "/checkout/sagascript",
    GITHUB_RUN_ID: "7021",
    GITHUB_RUN_ATTEMPT: "2",
  };
  const context = workflowRunContext(env, commitSha, "/checkout/sagascript");
  assert.deepEqual(context, { runId: 7021, attempt: 2, runRef: "ref:github-run-7021-attempt-2" });
  assert.equal("GH_TOKEN" in env, false);
  assert.equal(workflowRunContext({}, commitSha, "/checkout/sagascript"), null);

  assert.throws(() => workflowRunContext({ ...env, GITHUB_RUN_ID: "0" }, commitSha, "/checkout/sagascript"), /run identity/);
  assert.throws(() => workflowRunContext({ ...env, GITHUB_RUN_ID: "1e2" }, commitSha, "/checkout/sagascript"), /run identity/);
  assert.throws(() => workflowRunContext({ ...env, GITHUB_RUN_ATTEMPT: "two" }, commitSha, "/checkout/sagascript"), /run identity/);
  assert.throws(() => workflowRunContext({ ...env, GITHUB_REPOSITORY: "someone/else" }, commitSha, "/checkout/sagascript"), /repository identity/);
  assert.throws(() => workflowRunContext({ ...env, GITHUB_SHA: "c".repeat(40) }, commitSha, "/checkout/sagascript"), /commit identity/);
  assert.throws(() => workflowRunContext({ ...env, GITHUB_WORKSPACE: "/checkout/other" }, commitSha, "/checkout/sagascript"), /workspace/);
});

test("producer rejects modified, staged, and untracked checkout state", () => {
  assert.doesNotThrow(() => assertCleanCheckout(""));
  assert.throws(() => assertCleanCheckout(" M src/App.svelte\n"), /clean checkout/);
  assert.throws(() => assertCleanCheckout("?? src/new-file.ts\n"), /clean checkout/);
  assert.throws(() => assertCleanCheckout("A  src/staged.ts\n"), /clean checkout/);
  assert.throws(() => assertCleanCheckout(null), /status is malformed/);
});

test("Knip candidates do not make the producer command fail", () => {
  assert.ok(KNIP_ARGS.includes("--no-exit-code"));
  assert.match(config.unused_candidates.command, /--no-exit-code$/);
});

test("Swift runs coverage tests before querying the generated codecov report", () => {
  const calls = [];
  const scratchPath = "/tmp/swift-task/scratch";
  const reportPath = `${scratchPath}/codecov/default/codecov.json`;
  const actual = runSwiftCoverageSteps({
    scratchPath,
    tempRoot: "/tmp/swift-task",
    runCommand: (args, options) => {
      calls.push({ args, options });
      return args.includes("--show-codecov-path") ? `${reportPath}\n` : "test run completed\n";
    },
  });
  assert.deepEqual(calls.map(call => call.args), [
    ["test", "--enable-code-coverage", "--jobs", "2", "--scratch-path", scratchPath],
    ["test", "--show-codecov-path", "--scratch-path", scratchPath],
  ]);
  assert.deepEqual(calls.map(call => call.options.env.TMPDIR), ["/tmp/swift-task", "/tmp/swift-task"]);
  assert.equal(actual, reportPath);

  const failedCalls = [];
  assert.throws(() => runSwiftCoverageSteps({
    scratchPath,
    tempRoot: "/tmp/swift-task",
    runCommand: args => {
      failedCalls.push(args);
      throw new Error("swift test failed");
    },
  }), /swift test failed/);
  assert.equal(failedCalls.length, 1);
  assert.ok(!failedCalls[0].includes("--show-codecov-path"));
  assert.throws(() => runSwiftCoverageSteps({
    scratchPath,
    tempRoot: "/tmp/swift-task",
    runCommand: args => args.includes("--show-codecov-path") ? "/tmp/outside/coverage.json" : "",
  }), /outside task-local scratch/);
});

test("failed tool diagnostics retain test names and errors without URLs or source excerpts", () => {
  const output = safeFailureDiagnostic(
    "thread 'engine::tests::recording_failure' panicked at file:///private/var/task/file.rs:10\nfailures:\n    engine::tests::recording_failure\ntest result: FAILED. 0 passed; 1 failed\ntranscript contents must never appear\nlet source_excerpt = \"private value\";\n",
    "TypeError: TypeScript getDefaultLibFilePath is unavailable at file:///Users/runner/private.ts\n",
  );
  assert.match(output, /engine::tests::recording_failure/);
  assert.match(output, /FAILED\. 0 passed; 1 failed/);
  assert.match(output, /TypeError: TypeScript getDefaultLibFilePath is unavailable/);
  assert.doesNotMatch(output, /file:\/\//i);
  assert.doesNotMatch(output, /\/private\/|\/Users\//);
  assert.doesNotMatch(output, /transcript contents|private value|source_excerpt/);
  assert.ok(output.length <= 6000);
});

test("workflow collection schedule and manual dispatch are explicit and informational", () => {
  assert.match(workflow, /pull_request:\s*\n\s*branches: \[main\]/);
  assert.match(workflow, /- cron: "17 5 \* \* \*"[^\n]*Daily first-attempt CI cohort/);
  assert.match(workflow, /- cron: "29 5 \* \* 1"[^\n]*Weekly static measurements/);
  assert.match(workflow, /workflow_dispatch:\s*\n\npermissions:/);
  assert.match(workflow, /pull_request\) mode=conformance/);
  assert.match(workflow, /workflow_dispatch\) mode=static/);
  assert.match(workflow, /steps\.mode\.outputs\.mode == 'static'/);
  assert.doesNotMatch(workflow, /INPUT_MODE|inputs\.mode/);
  assert.match(workflow, /github\.event_name != 'workflow_dispatch' \|\| github\.ref == 'refs\/heads\/main'/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /actions: read\s*\n\s*contents: read/);
  assert.match(workflow, /- name: Initialize collection status artifact[\s\S]*?workflow-setup-not-completed/);
  assert.match(workflow, /- name: Test code-health contracts and workflow\s*\n\s*run: npm run test:code-health/);
  assert.match(workflow, /if: always\(\)[\s\S]*?uses: actions\/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9[\s\S]*?name: code-health-v1[\s\S]*?if-no-files-found: error[\s\S]*?retention-days: 30/);
  assert.doesNotMatch(workflow, /secrets\.(?!github_token)|HEIMDALL|MUNIN/i);

  const staticStep = workflow.match(/- name: Collect static measurements([\s\S]*?)(?=\n      - name:)/)?.[1];
  const ciStep = workflow.match(/- name: Collect daily CI cohort([\s\S]*?)(?=\n      - name:)/)?.[1];
  assert.ok(staticStep);
  assert.ok(ciStep);
  assert.doesNotMatch(staticStep, /GH_TOKEN|github\.token/);
  assert.match(ciStep, /if: \$\{\{ steps\.mode\.outputs\.mode == 'ci-only' \}\}/);
  assert.match(ciStep, /GH_TOKEN: \$\{\{ github\.token \}\}/);
  assert.match(staticStep, /rm -f "\$report_dir\/workflow-status\.json"/);
  assert.match(staticStep, /collector-exit/);
  assert.match(ciStep, /collector-exit/);
});

test("workflow analyzer installs match the producer's declared exact versions", () => {
  const expected = [
    ["rust_toolchain", /rustup toolchain install ([0-9.]+)/],
    ["cargo_llvm_cov", /cargo install[^\n]*cargo-llvm-cov --version ([0-9.]+)/],
    ["rust_code_analysis_cli", /cargo install[^\n]*rust-code-analysis-cli --version ([0-9.]+)/],
  ];
  for (const [key, expression] of expected) {
    const match = expression.exec(workflow);
    assert.ok(match, `workflow install command for ${key} is missing`);
    assert.equal(match[1], config.tools[key], `workflow ${key} version differs from the producer registry`);
  }
  assert.match(workflow, /--locked/);
  assert.equal(tooling.devDependencies.knip, config.tools.knip);
  assert.equal(tooling.devDependencies.knip, "5.46.0");
  assert.match(workflow, /npm ci --prefix tooling\/code-health --ignore-scripts/);
  assert.match(workflow, /CODE_HEALTH_KNIP: \$\{\{ github\.workspace \}\}\/tooling\/code-health\/node_modules\/\.bin\/knip/);
  assert.doesNotMatch(workflow, /package-lock=false|knip@\^|knip@~/);
  assert.equal(toolingLock.lockfileVersion, 3);
  assert.equal(toolingLock.packages[""].devDependencies.knip, config.tools.knip);
  assert.equal(toolingLock.packages["node_modules/knip"].version, config.tools.knip);
  assert.match(toolingLock.packages["node_modules/knip"].integrity, /^sha512-[A-Za-z0-9+/]+=*$/);
  assert.equal(tooling.devDependencies.typescript, config.tools.typescript);
  assert.equal(toolingLock.packages[""].devDependencies.typescript, config.tools.typescript);
  assert.equal(toolingLock.packages["node_modules/typescript"].version, config.tools.typescript);
});
