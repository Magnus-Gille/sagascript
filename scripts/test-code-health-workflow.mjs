import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  assertCleanCheckout,
  createMetadataTransport,
  firstAttemptRunContext,
  KNIP_ARGS,
  newLocalCollectionRunRef,
  paginate,
  parseProducerOptions,
  filterSwiftCoverageReport,
  prepareSwiftCoverageInputs,
  restJson,
  runSwiftCoverageSteps,
  safeFailureDiagnostic,
  workflowRunContext,
} from "./code-health-produce.mjs";
import { createObjective } from "./lib/code-health-producer.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(join(root, ".github/workflows/code-health.yml"), "utf8").replaceAll("\r\n", "\n");
const collectionDocs = readFileSync(join(root, "docs/code-health-collection.md"), "utf8");
const producerSource = readFileSync(join(root, "scripts/code-health-produce.mjs"), "utf8");
const config = JSON.parse(readFileSync(join(root, "docs/code-health-producer-v1.json"), "utf8"));
const tooling = JSON.parse(readFileSync(join(root, "tooling/code-health/package.json"), "utf8"));
const toolingLock = JSON.parse(readFileSync(join(root, "tooling/code-health/package-lock.json"), "utf8"));
const modeScriptMatch = workflow.match(/- name: Select collection mode[\s\S]*?\n        run: \|\n((?:          .*\n?)+?)(?=\n      - name:)/);
assert.ok(modeScriptMatch, "collection mode selection script is missing");
const modeScript = modeScriptMatch[1].split("\n").map(line => line.slice(10)).join("\n");

function runModeSelection({ eventName, eventSchedule = "", dispatchMode = "" }) {
  const outputDir = mkdtempSync(join(tmpdir(), "sagascript-mode-selection-test-"));
  const outputPath = join(outputDir, "github-output");
  const result = spawnSync("bash", ["-euo", "pipefail", "-c", modeScript], {
    encoding: "utf8",
    env: {
      ...process.env,
      EVENT_NAME: eventName,
      EVENT_SCHEDULE: eventSchedule,
      DISPATCH_MODE: dispatchMode,
      GITHUB_OUTPUT: outputPath,
    },
  });
  const output = existsSync(outputPath) ? readFileSync(outputPath, "utf8") : "";
  rmSync(outputDir, { recursive: true, force: true });
  return { ...result, output };
}

test("workflow-level first-attempt failure cannot be replaced by a successful rerun", () => {
  const run = { id: 42, head_sha: "a".repeat(40), run_attempt: 2, event: "push", head_branch: "main", status: "completed", conclusion: "success" };
  const first = firstAttemptRunContext(run, url => {
    assert.match(url, /\/runs\/42\/attempts\/1$/);
    return { ...run, run_attempt: 1, conclusion: "startup_failure" };
  });
  assert.deepEqual(first, { status: "completed", conclusion: "startup_failure" });
  const missing = () => { throw Object.assign(new Error("missing"), { status: 404 }); };
  assert.deepEqual(firstAttemptRunContext(run, missing), { status: null, conclusion: null });
  assert.deepEqual(firstAttemptRunContext({ ...run, run_attempt: 1, conclusion: "failure" }, missing), { status: "completed", conclusion: "failure" });
  assert.throws(() => firstAttemptRunContext(run, () => ({ ...run })), /identity does not match/);
});

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

test("local collections receive a unique stable run reference", () => {
  const first = newLocalCollectionRunRef();
  const second = newLocalCollectionRunRef();
  assert.match(first, /^ref:collection-run-[0-9a-f-]+-attempt-1$/);
  assert.notEqual(first, second);
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
  const rootPath = mkdtempSync(join(tmpdir(), "sagascript-swift-steps-test-"));
  try {
    const tempRoot = join(rootPath, "task-temp");
    const scratchPath = join(tempRoot, "scratch");
    const reportPath = `${scratchPath}/codecov/default/codecov.json`;
    const calls = [];
    const actual = runSwiftCoverageSteps({
      scratchPath,
      tempRoot,
      runCommand: (args, options) => {
        calls.push({ args, options });
        return args.includes("--show-codecov-path") ? `${reportPath}\n` : "test run completed\n";
      },
    });
    const sharedArgs = [
      "--cache-path", join(tempRoot, "swiftpm-cache"),
      "--config-path", join(tempRoot, "swiftpm-config"),
      "--security-path", join(tempRoot, "swiftpm-security"),
      "--scratch-path", scratchPath,
    ];
    assert.deepEqual(calls.map(call => call.args), [
      ["test", "--enable-code-coverage", "--jobs", "2", ...sharedArgs],
      ["test", "--show-codecov-path", ...sharedArgs],
    ]);
    assert.deepEqual(calls.map(call => call.options.env), [
      {
        TMPDIR: tempRoot,
        CLANG_MODULE_CACHE_PATH: join(tempRoot, "clang-module-cache"),
        SWIFT_MODULECACHE_PATH: join(tempRoot, "swift-module-cache"),
      },
      {
        TMPDIR: tempRoot,
        CLANG_MODULE_CACHE_PATH: join(tempRoot, "clang-module-cache"),
        SWIFT_MODULECACHE_PATH: join(tempRoot, "swift-module-cache"),
      },
    ]);
    assert.equal(actual, reportPath);

    const failedCalls = [];
    assert.throws(() => runSwiftCoverageSteps({
      scratchPath,
      tempRoot,
      runCommand: args => {
        failedCalls.push(args);
        throw new Error("swift test failed");
      },
    }), /swift test failed/);
    assert.equal(failedCalls.length, 1);
    assert.ok(!failedCalls[0].includes("--show-codecov-path"));
    assert.throws(() => runSwiftCoverageSteps({
      scratchPath,
      tempRoot,
      runCommand: args => args.includes("--show-codecov-path") ? "/tmp/outside/coverage.json" : "",
    }), /outside task-local scratch/);
  } finally {
    rmSync(rootPath, { recursive: true, force: true });
  }
});

test("Swift coverage filters external and generated exports while retaining source-only paths", () => {
  const packageRoot = resolve(join("/tmp/swift-task", "swift-package"));
  const sourcePackageRoot = resolve(join("/repo", "src-tauri/engine-host/coreml"));
  const report = {
    data: [{ files: [
      { filename: join(packageRoot, "Sources/EngineHostCore/Engine.swift"), summary: { lines: { count: 10, covered: 7 } } },
      { filename: join(packageRoot, "Sources/EngineHostCore/BuildInfo.swift"), summary: { lines: { count: 4, covered: 4 } } },
      { filename: join(packageRoot, "Tests/EngineHostCoreTests/ContextBiasingTests.swift"), summary: { lines: { count: 20, covered: 20 } } },
      { filename: join(packageRoot, ".build/out/Intermediates.noindex/test_entry_point.swift"), summary: { lines: { count: 5, covered: 5 } } },
      { filename: "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/lib/swift/Swift.swiftmodule/Swift.swiftinterface", summary: { lines: { count: 100, covered: 100 } } },
    ] }],
  };

  const filtered = filterSwiftCoverageReport(report, { packageRoot, sourcePackageRoot });
  assert.deepEqual(filtered.report.data[0].files.map(file => file.filename), [
    join(sourcePackageRoot, "Sources/EngineHostCore/Engine.swift"),
  ]);
  assert.deepEqual(filtered.excludedPaths, [
    { path: "Sources/EngineHostCore/BuildInfo.swift", reason: "generated-build-info" },
    { path: "Tests/EngineHostCoreTests/ContextBiasingTests.swift", reason: "outside-source-root" },
    { path: ".build/out/Intermediates.noindex/test_entry_point.swift", reason: "outside-source-root" },
    { path: null, reason: "outside-temporary-package" },
  ]);
  assert.equal(JSON.stringify(filtered).includes("/Applications/Xcode.app"), false);
});

test("Swift coverage preparation places the shared tracked vectors at the package sibling path", () => {
  assert.match(producerSource, /sharedFixtureRoot: join\(REPO_ROOT, "src-tauri\/engine-host\/test-vectors"\)/);
  const rootPath = mkdtempSync(join(tmpdir(), "sagascript-swift-input-test-"));
  try {
    const sourcePackageRoot = join(rootPath, "source-package");
    const tempRoot = join(rootPath, "task-temp");
    const packageRoot = join(tempRoot, "swift-package");
    const fixtureRoot = join(rootPath, "test-vectors");
    mkdirSync(join(sourcePackageRoot, "Tests/EngineHostCoreTests"), { recursive: true });
    writeFileSync(join(sourcePackageRoot, "Tests/EngineHostCoreTests/ContextBiasingTests.swift"), "fixture consumer");
    mkdirSync(join(fixtureRoot, "protocol"), { recursive: true });
    writeFileSync(join(fixtureRoot, "context-biasing.json"), '{"fixture":"shared"}\n');
    writeFileSync(join(fixtureRoot, "protocol/handshake.jsonl"), '{"note":"golden"}\n');
    const copied = prepareSwiftCoverageInputs({ sourcePackageRoot, packageRoot, tempRoot, sharedFixtureRoot: fixtureRoot });
    assert.equal(copied, join(tempRoot, "test-vectors"));
    assert.equal(readFileSync(join(copied, "context-biasing.json"), "utf8"), '{"fixture":"shared"}\n');
    assert.equal(readFileSync(join(copied, "protocol/handshake.jsonl"), "utf8"), '{"note":"golden"}\n');
    assert.equal(readFileSync(join(packageRoot, "Tests/EngineHostCoreTests/ContextBiasingTests.swift"), "utf8"), "fixture consumer");
  } finally {
    rmSync(rootPath, { recursive: true, force: true });
  }
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
  assert.match(workflow, /workflow_dispatch:\s*\n\s*inputs:\s*\n\s*mode:\s*\n\s*description: Collection mode for this manual run\s*\n\s*required: false\s*\n\s*default: static\s*\n\s*type: choice\s*\n\s*options:\s*\n\s*- static\s*\n\s*- ci-only/);
  assert.match(workflow, /pull_request\) mode=conformance/);
  assert.match(workflow, /DISPATCH_MODE: \$\{\{ inputs\.mode \}\}/);
  assert.match(workflow, /workflow_dispatch\)[\s\S]*?''\|static\) mode=static/);
  assert.match(workflow, /workflow_dispatch\)[\s\S]*?ci-only\) mode=ci-only/);
  assert.match(workflow, /Unsupported manual mode: \$DISPATCH_MODE/);
  assert.doesNotMatch(workflow, /workflow_dispatch\) mode=\$\{\{/);
  for (const [dispatchMode, expected] of [["", "static"], ["static", "static"], ["ci-only", "ci-only"]]) {
    const result = runModeSelection({ eventName: "workflow_dispatch", dispatchMode });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.output.trim(), `mode=${expected}`);
  }
  const invalid = runModeSelection({ eventName: "workflow_dispatch", dispatchMode: "everything" });
  assert.notEqual(invalid.status, 0);
  assert.match(invalid.stderr, /Unsupported manual mode: everything/);
  for (const [eventName, eventSchedule, expected] of [
    ["pull_request", "", "conformance"],
    ["schedule", "17 5 * * *", "ci-only"],
    ["schedule", "29 5 * * 1", "static"],
  ]) {
    const result = runModeSelection({ eventName, eventSchedule });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.output.trim(), `mode=${expected}`);
  }
  const invalidSchedule = runModeSelection({ eventName: "schedule", eventSchedule: "0 0 * * *" });
  assert.notEqual(invalidSchedule.status, 0);
  assert.match(invalidSchedule.stderr, /Unregistered schedule/);
  assert.match(workflow, /name: Select newest Xcode for Swift coverage[\s\S]*?find \/Applications -maxdepth 1 -name "Xcode_\*\.app"/);
  assert.match(workflow, /name: Select newest Xcode for Swift coverage[\s\S]*?if: \$\{\{ steps\.mode\.outputs\.mode == 'static' \}\}/);
  assert.match(workflow, /name: Select newest Xcode for Swift coverage[\s\S]*?echo "DEVELOPER_DIR=\$developer_dir" >> "\$GITHUB_ENV"/);
  assert.match(workflow, /name: Select newest Xcode for Swift coverage[\s\S]*?swift --version/);
  assert.ok(workflow.indexOf("name: Select newest Xcode for Swift coverage") < workflow.indexOf("name: Collect static measurements"));
  assert.match(workflow, /steps\.mode\.outputs\.mode == 'static'/);
  assert.match(workflow, /github\.event_name != 'workflow_dispatch' \|\| github\.ref == 'refs\/heads\/main'/);
  assert.match(workflow, /github\.event_name != 'schedule' \|\| vars\.CODE_HEALTH_COLLECTION_ENABLED == 'true'/);
  assert.match(workflow, /jobs:\s*\n\s*collect:\s*\n\s*if: \$\{\{[^\n]*workflow_dispatch[^\n]*CODE_HEALTH_COLLECTION_ENABLED/);
  assert.match(collectionDocs, /CODE_HEALTH_COLLECTION_ENABLED=true/);
  assert.match(collectionDocs, /unsetting `CODE_HEALTH_COLLECTION_ENABLED` or setting it to\s+`false`/);
  assert.match(collectionDocs, /pull-request conformance and main-branch manual collection remain available/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /actions: read\s*\n\s*contents: read/);
  assert.match(workflow, /- name: Initialize collection status artifact[\s\S]*?workflow-setup-not-completed/);
  assert.match(workflow, /- name: Finalize workflow phase and collection budgets[\s\S]*?preupload_elapsed_seconds[\s\S]*?metadata_over_budget/);
  assert.match(workflow, /collector_failures[\s\S]*?status=partial[\s\S]*?phase_elapsed_seconds/);
  assert.match(workflow, /CONFORMANCE_OUTCOME: \$\{\{ steps\.conformance\.outcome \}\}/);
  assert.match(workflow, /timeout-minutes: 3[\s\S]*?name: code-health-v1/);
  assert.match(workflow, /- name: Test code-health contracts and workflow\s*\n\s*id: conformance\s*\n\s*run: npm run test:code-health/);
  assert.match(workflow, /if: always\(\)[\s\S]*?uses: actions\/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9[\s\S]*?name: code-health-v1[\s\S]*?if-no-files-found: error[\s\S]*?retention-days: 30/);
  assert.doesNotMatch(workflow, /secrets\.(?!github_token)|HEIMDALL|MUNIN/i);
  assert.match(producerSource, /local_or_ci: workflowContext \? "github-actions" : "local"/);
  assert.doesNotMatch(producerSource, /local_or_ci: includeCi/);

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

test("GitHub metadata requests share an injectable monotonic deadline across pagination", () => {
  let now = 0;
  const requests = [];
  const request = createMetadataTransport((url, timeoutMs) => {
    requests.push({ url, timeoutMs });
    now += 8;
    const page = Number(new URL(url).searchParams.get("page"));
    return { total_count: 101, entries: Array.from({ length: page === 1 ? 100 : 1 }, (_, index) => index) };
  }, { budgetMs: 30, now: () => now });
  const result = paginate("https://api.github.com/repos/acme/app/runs", "entries", 200, request);
  assert.equal(result.length, 101);
  assert.deepEqual(requests.map(item => item.timeoutMs), [30, 22]);
  request("https://api.github.com/next?page=3");
  request("https://api.github.com/next?page=4");
  assert.throws(() => request("https://api.github.com/next?page=5"), /30-second budget/);
  assert.deepEqual(requests.map(item => item.timeoutMs), [30, 22, 14, 6]);
});

test("GitHub REST transport sends credentials only on stdin, rejects redirects, and has no redirect following", () => {
  const previous = process.env.GH_TOKEN;
  process.env.GH_TOKEN = "test-token-value-not-a-real-credential";
  let captured;
  try {
    assert.throws(() => restJson("https://api.github.com/repos/acme/app", 1250, (_command, args, options) => {
      captured = { args, input: options.input };
      return { error: null, status: 0, stdout: '{"message":"redirect"}\n__HTTP_STATUS__302' };
    }), /HTTP 302/);
  } finally {
    if (previous === undefined) delete process.env.GH_TOKEN;
    else process.env.GH_TOKEN = previous;
  }
  assert.ok(captured);
  assert.ok(!captured.args.includes("--location"));
  assert.equal(captured.args[captured.args.indexOf("--max-time") + 1], "1.25");
  assert.ok(!captured.args.includes("test-token-value-not-a-real-credential"));
  assert.ok(captured.input.includes("Authorization: Bearer test-token-value-not-a-real-credential"));
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

test("Rust CLI coverage is a separate no-default-features profile", () => {
  assert.deepEqual(config.coverage.rust_cli, {
    package: "sagascript-cli",
    command: "cargo +1.93.1 llvm-cov -p sagascript-cli --no-default-features --tests --json --output-path <task-temporary-report>",
    target: "isolated task-local CARGO_TARGET_DIR",
    includes_inline_tests: true,
    source_roots: [
      "src-tauri/crates/sagascript-cli/src/",
      "src-tauri/crates/sagascript-cli/tests/",
    ],
    inventory: "exported-only",
    features: "no-default-features",
  });
  assert.match(producerSource, /coverage-rust-cli[\s\S]*sagascript-cli[\s\S]*--no-default-features[\s\S]*--tests/);
  assert.match(producerSource, /setFailure\("rust-cli", "coverage"\)/);
  assert.match(collectionDocs, /Rust CLI coverage runs the lean batch profile/);
});
