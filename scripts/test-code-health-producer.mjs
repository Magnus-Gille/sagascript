import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  assembleReportBundle,
  assertCompletePageCount,
  buildCiFirstAttemptPayload,
  classifyCiWorkflowConfiguration,
  ciCohortUnavailableReason,
  ciUnavailableMeasurement,
  createObjective,
  parseWorkflowJobIds,
  normalizeCiCohort,
  parseExportedCoverage,
  parseKnipCandidates,
  parseRustComplexity,
  wholeSecondUtc,
  sha256,
} from "./lib/code-health-producer.mjs";
import {
  CI_API_TOOL,
  collectFirstAttemptMetadata,
  collectRustCliCoverage,
  command,
  createAsyncMetadataTransport,
  githubActionsJson,
  paginateAsync,
  safeFailureDiagnostic as safeProducerFailureDiagnostic,
} from "./code-health-produce.mjs";
import { validateObjective, validateObjectiveSeries } from "./lib/code-health-objective.mjs";
import { seriesKey } from "./lib/code-health-policy.mjs";
import { summarizeCohorts } from "./ci-cohort-timings.mjs";

const readJson = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const config = readJson("../docs/code-health-producer-v1.json");
const schema = readJson("../docs/code-health-objective-v1.schema.json");
const fixture = (path) => readJson("../tests/fixtures/code-health/" + path);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const commit = "a".repeat(40);
const runRef = "ref:local-aaaaaaaaaaaa-attempt-1";
const observedAt = "2026-10-09T10:00:00Z";
const digest = "b".repeat(64);
const at = (offsetSeconds = 0) => new Date(Date.parse("2026-09-15T00:00:00Z") + offsetSeconds * 1000).toISOString().replace(".000Z", "Z");
const expectedJobs = ["scope", "test-macos"];

test("command floors fractional subprocess timeouts without passing invalid values to Node", () => {
  assert.equal(command(process.execPath, ["-e", "process.stdout.write('spawn-ok')"], { timeout: 10_000.75 }), "spawn-ok");
  assert.throws(
    () => command(process.execPath, ["-e", "process.stdout.write('must-not-run')"], { timeout: 0.75 }),
    /deadline has less than one millisecond remaining/,
  );
});

test("failed commands report captured stream sizes and the platform-specific child termination", () => {
  const expectedSignal = process.platform === "win32" ? "none" : "SIGTERM";
  assert.throws(
    () =>
      command(
        process.execPath,
        [
          "-e",
          "require('node:fs').writeSync(1, 'stdout-probe'); require('node:fs').writeSync(2, 'stderr-probe'); process.kill(process.pid, 'SIGTERM')",
        ],
        { timeout: 10_000 },
      ),
    (error) => {
      assert.match(error.message, new RegExp(`capture stdout_bytes=12 stderr_bytes=12 child_signal=${expectedSignal}`));
      return true;
    },
  );
});

test("Swift compiler diagnostics keep the error message without leaking source paths or snippets", () => {
  const diagnostic = safeProducerFailureDiagnostic(
    "",
    "/Users/magnus/repos/sagascript/src-tauri/engine-host/coreml/Tests/CoreMLTests.swift:3:8: error: no such module 'Testing'",
  );
  assert.match(diagnostic, /stderr swift-error: no such module 'Testing'/);
  assert.doesNotMatch(diagnostic, /CoreMLTests\.swift|\/Users\/magnus|import Testing/);
});

test("Rust failure diagnostics retain ANSI-colored test names and PID panic identities safely", () => {
  const red = "\u001b[31m";
  const reset = "\u001b[0m";
  const token = `ghp_${"a".repeat(32)}`;
  const diagnostic = safeProducerFailureDiagnostic(
    [
      `${red}test core::parser::rejects_bad_input ... FAILED${reset}`,
      "",
      `${red}failures:${reset}`,
      "",
      `${red}---- core::parser::rejects_bad_input stdout ----${reset}`,
      `${red}thread 'core::parser::rejects_bad_input' (38122136812) panicked at /Users/magnus/repos/sagascript/src-tauri/src/lib.rs:42:7: ${token}${reset}`,
      "",
      `${red}test result: FAILED. 0 passed; 1 failed${reset}`,
    ].join("\n"),
    `${red}error: token ${token} at /Users/magnus/private/credential.txt${reset}`,
  );
  const panicOnly = safeProducerFailureDiagnostic(
    "",
    `${red}thread 'core::parser::pid_only' (123) panicked at /private/tmp/test.rs:1:1:${reset}`,
  );
  assert.equal(panicOnly, "stderr test-failed: core::parser::pid_only");
  assert.match(diagnostic, /failed-test: core::parser::rejects_bad_input/);
  assert.match(diagnostic, /test-failed: core::parser::rejects_bad_input/);
  assert.match(diagnostic, /test result: FAILED/);
  assert.match(diagnostic, /<redacted-token>/);
  assert.doesNotMatch(diagnostic, /\u001b|38122136812|ghp_[a-zA-Z0-9_]{20,}|\/Users\/magnus/);
});

test("Rust command diagnostics retain only child exit causes and sanitize dyld loader errors", () => {
  const diagnostic = safeProducerFailureDiagnostic(
    "",
    [
      "process didn't exit successfully: `/private/tmp/private-cargo/bin/cargo test --secret-arg hidden` (signal: 9, SIGKILL)",
      "process didn't exit successfully: `/private/tmp/private-cargo/bin/test-binary --test-filter hidden` (signal: 6, SIGABRT: process abort signal)",
      "error: process didn't exit successfully: `/private/tmp/private-cargo/bin/cargo test --private-value hidden` (exit status: 101)",
      "error: process didn't exit successfully: `/private/tmp/private-cargo/bin/cargo test --unknown-value hidden` (unrecognized cause)",
      "Caused by:",
      "exit status: 102",
      "dyld[123]: Library not loaded: /Users/magnus/private/lib/libexample.dylib",
    ].join("\n"),
  );
  assert.match(diagnostic, /child-signal: SIGKILL \(9\)/);
  assert.match(diagnostic, /child-signal: SIGABRT \(6\)/);
  assert.match(diagnostic, /child-exit: 101/);
  assert.match(diagnostic, /child-exit: 102/);
  assert.match(diagnostic, /dyld-loader: Library not loaded/);
  assert.doesNotMatch(
    diagnostic,
    /private-cargo|secret-arg|private-value|test-binary|test-filter|abort signal|hidden|\/Users\/magnus|libexample\.dylib/,
  );
});

test("dyld failures expose only the category regardless of library path", () => {
  for (const path of [
    "/Users/example/Library/Application Support/PRIVATE_PROJECT_SENTINEL/libexample.dylib",
    "/srv/PRIVATE_PROJECT_SENTINEL/libexample.dylib",
  ]) {
    assert.equal(safeProducerFailureDiagnostic("", `dyld[123]: Library not loaded: ${path}`), "stderr dyld-loader: Library not loaded");
  }
});

test("runtime crash diagnostics expose fixed categories without paths, symbols, or allocator payloads", () => {
  const secret = "PRIVATE_SECRET_SENTINEL";
  const diagnostic = safeProducerFailureDiagnostic(
    "",
    [
      `dyld[123]: symbol not found in flat namespace '_${secret}'`,
      `dyld[123]: could not load inserted library: /Users/example/Library/Application Support/${secret}.dylib`,
      `fatal runtime error: stack overflow in /private/tmp/${secret}.rs`,
      `fatal runtime error: non-unwinding panic from ${secret}`,
      `thread caused non-unwinding panic. aborting. (${secret})`,
      `fatal runtime error: failed to initiate panic, error 5 (${secret})`,
      `LLVM Profile Error: /srv/${secret}/default.profraw is malformed`,
      `LLVM Profile Runtime: ${secret}`,
      `LLVM ERROR: unable to profile ${secret}`,
      `binary(123,0x${"a".repeat(16)}) /Users/example/Library/Application Support/${secret} malloc: *** error for object 0x${"a".repeat(16)}: pointer being freed was not allocated (${secret})`,
      `binary(123,0x${"b".repeat(16)}) malloc: nano zone abandoned due to inability to preallocate reserved vm space (${secret})`,
    ].join("\n"),
  );
  assert.equal(
    diagnostic,
    [
      "stderr dyld-loader: symbol-not-found",
      "stderr dyld-loader: other",
      "stderr rust-fatal-runtime: stack-overflow",
      "stderr rust-fatal-runtime: non-unwinding-panic",
      "stderr rust-fatal-runtime: non-unwinding-panic",
      "stderr rust-fatal-runtime: other",
      "stderr llvm-profile: error",
      "stderr llvm-runtime: fatal",
      "stderr llvm: error",
      "stderr malloc-allocator: fatal",
      "stderr malloc-allocator: diagnostic",
    ].join(" | "),
  );
  assert.doesNotMatch(diagnostic, /PRIVATE_SECRET_SENTINEL|Library\/Application Support|\/private\/tmp|\/srv\/|0x[a-f0-9]+|_PRIVATE/);
});

test("native assertion and termination diagnostics expose only fixed categories", () => {
  const secret = "PRIVATE_NATIVE_SECRET_SENTINEL";
  const diagnostic = safeProducerFailureDiagnostic(
    "",
    [
      `Assertion failed: ${secret}, function check, file /Users/example/Library/Application Support/${secret}.mm, line 7`,
      `GGML_ASSERT(${secret}) at /private/tmp/${secret}.cpp:8`,
      `ggml_abort: /srv/${secret}/model.gguf`,
      `libc++abi: terminating with uncaught exception of type ${secret}: /Users/example/${secret}`,
      `terminate called after throwing an instance of '${secret}'`,
    ].join("\n"),
  );
  assert.equal(
    diagnostic,
    [
      "stderr native-assertion: failed",
      "stderr native-assertion: failed",
      "stderr native-assertion: failed",
      "stderr native-termination: libcxxabi",
      "stderr native-termination: throwing",
    ].join(" | "),
  );
  assert.doesNotMatch(diagnostic, /PRIVATE_NATIVE_SECRET_SENTINEL|Library\/Application Support|\/private\/tmp|\/srv\/|model\.gguf/);
});

test("unknown process failures never fall through to generic permission diagnostics", () => {
  for (const header of ["error: ", "Error: ", "Caused by: ", ""]) {
    for (const suffix of ["ENOENT", "PermissionDenied", "EACCES"]) {
      const line = `${header}process didn't exit successfully: \`/private/tmp/test-bin --private-value never-emit-this --filter ${suffix}\` (unrecognized cause)`;
      assert.equal(safeProducerFailureDiagnostic("", line), "");
    }
  }
});

test("Rust complexity counts function-body complexity once and uses a strict threshold", () => {
  const result = parseRustComplexity(
    [
      {
        path: "src-tauri/crates/sagascript-cli/src/transcribe.rs",
        ast: fixture("rust-complexity.json"),
      },
    ],
    10,
  );
  assert.equal(result.eligible_functions, 4);
  assert.equal(result.above_threshold_functions, 2);
  assert.deepEqual(
    result.functions.map((item) => [item.name, item.cyclomatic]),
    [
      ["outer", 11],
      ["nested", 4],
      ["exact_threshold", 10],
      ["<anonymous>", 11],
    ],
  );
  assert.throws(() => parseRustComplexity([], 10), /inventory is empty/);
  assert.throws(
    () => parseRustComplexity([{ path: "broken.rs", ast: { spaces: [{ kind: "function", spaces: [] }] } }], 10),
    /score is missing/,
  );
});

test("the rust-cli complexity scope lists every production module of the transcribe command", () => {
  // The scope names files, so a module added to transcribe/ (or to a directory module inside it)
  // would silently drop out of the measurement until someone remembers to list it. Test files
  // (tests.rs, *_tests.rs) are not measured.
  const directory = "src-tauri/crates/sagascript-cli/src/transcribe";
  const productionModules = (dir) =>
    readdirSync(join(repoRoot, dir), { withFileTypes: true }).flatMap((entry) => {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) return productionModules(path);
      return entry.name.endsWith(".rs") && !/(^|_)tests\.rs$/.test(entry.name) ? [path] : [];
    });
  const modules = productionModules(directory).sort();
  const configured = config.complexity.scopes["rust-cli"];
  assert.deepEqual(
    configured.filter((path) => path.startsWith(`${directory}/`)).sort(),
    modules,
    "docs/code-health-producer-v1.json complexity.scopes.rust-cli must list exactly the production files of transcribe/",
  );
  for (const path of configured) assert.ok(existsSync(join(repoRoot, path)), `${path} is listed in the rust-cli scope but does not exist`);
});

test("the rust-core complexity scope lists every production module of the file pipeline", () => {
  // The file pipeline moved from the CLI's transcribe/ to core's meeting/pipeline/ (#374). Same
  // rule as for the CLI: a module added there would drop out of the measurement until someone
  // remembers to list it. Three files are not listed because the analyzer finds no function in
  // them, which it reports as an error for a selected file: services.rs is one struct, narrate.rs
  // a trait and a macro, and mod.rs only lists modules.
  const directory = "src-tauri/crates/sagascript-core/src/meeting/pipeline";
  const unmeasured = new Set([
    `${directory}/narrate.rs`,
    `${directory}/mod.rs`,
    `${directory}/services.rs`,
  ]);
  const productionModules = (dir) =>
    readdirSync(join(repoRoot, dir), { withFileTypes: true }).flatMap((entry) => {
      const path = `${dir}/${entry.name}`;
      if (entry.isDirectory()) return productionModules(path);
      return entry.name.endsWith(".rs") && !/(^|_)tests\.rs$/.test(entry.name) ? [path] : [];
    });
  const modules = productionModules(directory)
    .filter((path) => !unmeasured.has(path))
    .sort();
  const configured = config.complexity.scopes["rust-core"];
  assert.deepEqual(
    configured.filter((path) => path.startsWith(`${directory}/`)).sort(),
    modules,
    "docs/code-health-producer-v1.json complexity.scopes.rust-core must list exactly the production files of meeting/pipeline/",
  );
  for (const path of configured) assert.ok(existsSync(join(repoRoot, path)), `${path} is listed in the rust-core scope but does not exist`);
});

test("Rust coverage labels inline tests and keeps inactive or non-emitted files out of the denominator", () => {
  const parsed = parseExportedCoverage(fixture("rust-coverage.json"), {
    repoRoot: "/workspace/sagascript",
    inventoryPaths: [
      "src-tauri/crates/sagascript-core/src/covered.rs",
      "src-tauri/crates/sagascript-core/src/zero.rs",
      "src-tauri/crates/sagascript-core/src/feature-disabled.rs",
      "src-tauri/crates/sagascript-core/tests/it.rs",
    ],
    sourceRoots: ["src-tauri/crates/sagascript-core/src/", "src-tauri/crates/sagascript-core/tests/"],
    excludedPaths: ["src-tauri/crates/sagascript-core/src/generated/BuildInfo.rs"],
    includesInlineTests: true,
  });
  assert.equal(parsed.measure, "lines");
  assert.equal(parsed.covered, 7);
  assert.equal(parsed.eligible, 16);
  assert.equal(parsed.emitted_source_files, 2);
  assert.equal(parsed.eligible_source_files, 2);
  assert.equal(parsed.includes_inline_tests, true);
  assert.equal(parsed.source_inventory, "exported-only");
  assert.deepEqual(parsed.not_emitted_paths, [
    "src-tauri/crates/sagascript-core/src/feature-disabled.rs",
    "src-tauri/crates/sagascript-core/tests/it.rs",
  ]);
  assert.ok(parsed.excluded_report_paths.every((item) => item.path === null || !item.path.startsWith("/")));
  assert.throws(
    () =>
      parseExportedCoverage(
        { data: [] },
        {
          repoRoot: "/workspace/sagascript",
          inventoryPaths: [],
          sourceRoots: ["src/"],
        },
      ),
    /emitted no files/,
  );
  assert.throws(
    () =>
      parseExportedCoverage(
        { data: [{ files: [{ filename: "src/a.rs", summary: { lines: { count: 1, covered: 2 } } }] }] },
        {
          repoRoot: "/workspace/sagascript",
          inventoryPaths: ["src/a.rs"],
          sourceRoots: ["src/"],
        },
      ),
    /counts are invalid/,
  );
});

test("Rust CLI collector uses an isolated no-default profile and tracked CLI inventory", () => {
  const tempRoot = mkdtempSync(join(tmpdir(), "sagascript-cli-coverage-test-"));
  const captured = {};
  try {
    const result = collectRustCliCoverage(
      tempRoot,
      "cargo 1.93.1",
      "cargo-llvm-cov 0.9.1",
      "rustc 1.93.1",
      true,
      (executable, args, options) => {
        captured.executable = executable;
        captured.args = args;
        captured.options = options;
        const outputPath = args[args.indexOf("--output-path") + 1];
        writeFileSync(
          outputPath,
          JSON.stringify({
            data: [
              {
                files: [
                  { filename: join(repoRoot, "src-tauri/crates/sagascript-cli/src/lib.rs"), summary: { lines: { count: 10, covered: 4 } } },
                  {
                    filename: join(repoRoot, "src-tauri/crates/sagascript-cli/tests/latency_report.rs"),
                    summary: { lines: { count: 3, covered: 1 } },
                  },
                  {
                    filename: join(repoRoot, "src-tauri/crates/sagascript-cli/src/not-tracked.rs"),
                    summary: { lines: { count: 5, covered: 5 } },
                  },
                  { filename: join(repoRoot, "src-tauri/crates/sagascript-core/src/lib.rs"), summary: { lines: { count: 7, covered: 7 } } },
                  { filename: join(tempRoot, "external.rs"), summary: { lines: { count: 9, covered: 9 } } },
                ],
              },
            ],
          }),
        );
        return "";
      },
    );
    assert.equal(result.status, "measured");
    assert.equal(result.value.payload.covered, 5);
    assert.equal(result.value.payload.eligible, 13);
    assert.deepEqual(result.value.payload.included_paths, [
      "src-tauri/crates/sagascript-cli/src/lib.rs",
      "src-tauri/crates/sagascript-cli/tests/latency_report.rs",
    ]);
    assert.ok(result.value.payload.not_emitted_paths.includes("src-tauri/crates/sagascript-cli/src/diarization.rs"));
    assert.ok(result.value.payload.excluded_report_paths.some((item) => item.reason === "outside-repository"));
    assert.ok(
      result.value.payload.excluded_report_paths.some(
        (item) => item.reason === "not-in-declared-inventory" && item.path.endsWith("not-tracked.rs"),
      ),
    );
    assert.equal(captured.executable, "cargo");
    assert.deepEqual(captured.args, [
      "+1.93.1",
      "llvm-cov",
      "-p",
      "sagascript-cli",
      "--no-default-features",
      "--tests",
      "--json",
      "--output-path",
      join(tempRoot, "rust-cli-coverage.json"),
    ]);
    assert.equal(captured.options.cwd, join(repoRoot, "src-tauri"));
    assert.match(captured.options.env.CARGO_TARGET_DIR, /cargo-target-rust-cli$/);
    assert.match(captured.options.env.LLVM_PROFILE_FILE, /rust-cli-%p-%m\.profraw$/);
    assert.equal(captured.options.env.CARGO_ENCODED_RUSTFLAGS, "");
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
});

test("Rust CLI collector fails instead of emitting a zero result on command or report errors", () => {
  for (const [label, runCommand, expected] of [
    [
      "command",
      () => {
        throw new Error("fake cargo failed");
      },
      /fake cargo failed/,
    ],
    [
      "report",
      (_executable, args) => writeFileSync(args[args.indexOf("--output-path") + 1], JSON.stringify({ data: [] })),
      /emitted no files/,
    ],
  ]) {
    const tempRoot = mkdtempSync(join(tmpdir(), `sagascript-cli-coverage-${label}-`));
    try {
      const result = collectRustCliCoverage(tempRoot, "cargo 1.93.1", "cargo-llvm-cov 0.9.1", "rustc 1.93.1", false, runCommand);
      assert.equal(result.status, "failed");
      assert.equal(result.value, undefined);
      assert.match(result.error.message, expected);
    } finally {
      rmSync(tempRoot, { recursive: true, force: true });
    }
  }
});

test("Swift source-only coverage excludes generated identity and test bundle files", () => {
  const parsed = parseExportedCoverage(fixture("swift-coverage.json"), {
    repoRoot: "/workspace/sagascript",
    inventoryPaths: [
      "src-tauri/engine-host/coreml/Sources/EngineHostCore/ProtocolServer.swift",
      "src-tauri/engine-host/coreml/Sources/EngineHostCore/CoreMLEngine.swift",
    ],
    sourceRoots: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/"],
    excludedPaths: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/BuildInfo.swift"],
  });
  assert.equal(parsed.covered, 80);
  assert.equal(parsed.eligible, 100);
  assert.equal(parsed.emitted_source_files, 1);
  assert.deepEqual(parsed.not_emitted_paths, ["src-tauri/engine-host/coreml/Sources/EngineHostCore/CoreMLEngine.swift"]);
  assert.ok(!parsed.included_paths.some((path) => path.includes("BuildInfo") || path.includes("Tests/")));
});

test("Knip candidates are projected by source language and generated/test candidates are excluded", () => {
  const report = fixture("knip.json");
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "typescript" }), [
    { category: "exports", path: "src/lib/Settings.ts", name: "oldSetting", line: 18 },
    { category: "files", path: "src/lib/Unused.ts", name: null, line: 1 },
  ]);
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "svelte" }), [
    { category: "svelte", path: "src/lib/Unused.svelte", name: "Unused.svelte", line: 1 },
  ]);
  assert.throws(() => parseKnipCandidates("not json", { repoRoot: "/workspace/sagascript", language: "typescript" }), /not valid JSON/);
});

test("native Knip JSON retains grouped file candidates and nested member candidates", () => {
  const report = fixture("knip-native.json");
  const typescript = parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "typescript" });
  assert.deepEqual(
    typescript.map(({ category, path, name, line }) => ({ category, path, name, line })),
    [
      { category: "classMembers", path: "src/lib/Settings.ts", name: "Settings.unusedMethod", line: 41 },
      { category: "duplicates", path: "src/lib/Settings.ts", name: "duplicateValue", line: 48 },
      { category: "enumMembers", path: "src/lib/Settings.ts", name: "Mode.Legacy", line: 35 },
      { category: "exports", path: "src/lib/Settings.ts", name: "oldSetting", line: 18 },
      { category: "nsExports", path: "src/lib/Settings.ts", name: "unusedNamespaceExport", line: 22 },
      { category: "nsTypes", path: "src/lib/Settings.ts", name: "UnusedNamespaceType", line: 31 },
      { category: "types", path: "src/lib/Settings.ts", name: "OldSettings", line: 27 },
      { category: "files", path: "src/lib/Unused.ts", name: null, line: 1 },
    ],
  );
  assert.deepEqual(parseKnipCandidates(report, { repoRoot: "/workspace/sagascript", language: "svelte" }), [
    { category: "files", path: "src/lib/Unused.svelte", name: null, line: 1 },
    { category: "exports", path: "src/lib/Widget.svelte", name: "unusedComponentExport", line: 7 },
  ]);
  assert.throws(
    () => parseKnipCandidates({ files: [null], issues: [] }, { repoRoot: "/workspace/sagascript", language: "typescript" }),
    /file candidate is malformed/,
  );
});

test("CI cohort uses attempt 1 despite a successful retry and preserves job outcomes", () => {
  const normalized = normalizeCiCohort({
    runs: [
      {
        id: 42,
        head_sha: "c".repeat(40),
        run_attempt: 2,
        event: "push",
        head_branch: "main",
        created_at: at(0),
        workflow_config_digest: digest,
        attempt1_jobs: [
          { id: 1001, name: "scope", status: "completed", conclusion: "failure", started_at: at(5), completed_at: at(20), steps: [] },
          {
            id: 1002,
            name: "test-macos",
            status: "completed",
            conclusion: "cancelled",
            started_at: at(6),
            completed_at: at(10),
            steps: [],
          },
        ],
        latest_conclusion: "success",
      },
    ],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.equal(normalized.runs.length, 1);
  assert.equal(normalized.runs[0].attempt, 1);
  assert.equal(normalized.runs[0].latest_attempt, 2);
  assert.equal(normalized.runs[0].overall_conclusion, "failure");
  assert.deepEqual(
    normalized.runs[0].jobs.map((job) => job.conclusion),
    ["failure", "cancelled"],
  );
  const payload = buildCiFirstAttemptPayload({
    normalizedRuns: normalized.runs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    expectedJobIds: expectedJobs,
    runInventoryRef: "ref:ci-main-push-28-day",
  });
  assert.equal(payload.expected_run_count, 1);
  assert.equal(payload.runs[0].latest_attempt, 2);
  const timings = summarizeCohorts(normalized.timing_runs);
  assert.equal(timings.cohorts[0].attempt, 1);
  assert.equal(timings.cohorts[0].counts.outcomes.failure, 1);
  assert.equal(timings.cohorts[0].ledger[0].attempt, 1);
});

test("measured CI cohort uses a schema-safe tool identity", () => {
  const scope = config.scopes.find((item) => item.name === "ci-release");
  const normalized = normalizeCiCohort({
    runs: [
      {
        id: 42,
        head_sha: "c".repeat(40),
        run_attempt: 2,
        event: "push",
        head_branch: "main",
        created_at: at(0),
        workflow_config_digest: digest,
        status: "completed",
        conclusion: "success",
        attempt1_jobs: expectedJobs.map((name, index) => ({
          id: 1001 + index,
          name,
          status: "completed",
          conclusion: "success",
          steps: [],
        })),
      },
    ],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot,
  });
  const payload = buildCiFirstAttemptPayload({
    normalizedRuns: normalized.runs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    expectedJobIds: expectedJobs,
    runInventoryRef: "ref:ci-main-push-28-day",
  });
  const objective = createObjective(
    scope,
    { commitSha: commit, runRef, attempt: 1, observedAt },
    {
      ci_first_attempt: {
        status: "measured",
        payload,
        tool: CI_API_TOOL,
        command: "GET /repos/{owner}/{repo}/actions/workflows/ci.yml/runs",
        included_refs: [payload.run_inventory_ref],
        excluded_refs: [],
        scope_policy: {
          workflow: config.ci.workflow,
          branch: config.ci.branch,
          event: config.ci.event,
          window_days: config.ci.window_days,
          expected_job_ids: expectedJobs,
          limits: config.ci.limits,
          attempt_policy: config.ci.attempt_policy,
          scope_version: "main-push-current-config-ci-v2",
        },
      },
    },
  );
  const validation = validateObjective(schema, objective);
  assert.equal(validation.valid, true, validation.schemaErrors.join("; "));
});

test("async CI metadata preserves the 89-run current-configuration attempt-1 cohort with bounded concurrency", async () => {
  const currentWorkflow = Buffer.from("name: CI\njobs:\n  scope:\n  test-macos:\n");
  const oldWorkflow = Buffer.from("name: legacy CI\njobs:\n  build:\n");
  const currentDigest = sha256(currentWorkflow);
  const windowStart = "2026-09-11T10:00:00Z";
  const windowEnd = "2026-10-09T10:00:00Z";
  const workflowBySha = new Map();
  const shaByKey = new Map();
  const shaFor = (key) => {
    if (!shaByKey.has(key)) shaByKey.set(key, String(shaByKey.size + 1).padStart(40, "0"));
    return shaByKey.get(key);
  };
  const apiRuns = Array.from({ length: 89 }, (_, index) => {
    const isCurrent = index < 19;
    const group = isCurrent ? `current-${Math.floor(index / 5)}` : `old-${Math.floor((index - 19) / 5)}`;
    const headSha = shaFor(group);
    workflowBySha.set(headSha, isCurrent ? currentWorkflow : oldWorkflow);
    return {
      id: 30000 + index,
      head_sha: headSha,
      run_attempt: [1, 2, 3].includes(index) ? 2 : 1,
      event: "push",
      head_branch: "main",
      created_at: at(10 * 86400 + index),
      status: "completed",
      conclusion: "success",
    };
  });
  const runById = new Map(apiRuns.map((run) => [run.id, run]));
  const jobIds = config.ci.expected_job_ids;
  let activeRequests = 0;
  let maximumActiveRequests = 0;
  let completedRequests = 0;
  let requestStarts = 0;
  let returned = false;
  let startsAfterReturn = 0;
  const transport = async (rawUrl, _timeoutMs, signal) => {
    const url = new URL(rawUrl);
    requestStarts += 1;
    if (returned) startsAfterReturn += 1;
    activeRequests += 1;
    maximumActiveRequests = Math.max(maximumActiveRequests, activeRequests);
    let timer;
    let onAbort;
    try {
      await new Promise((resolve, reject) => {
        timer = setTimeout(resolve, 2);
        onAbort = () => {
          clearTimeout(timer);
          reject(signal.reason);
        };
        signal.addEventListener("abort", onAbort, { once: true });
      });
      if (url.pathname.endsWith("/actions/workflows/ci.yml/runs")) {
        return { total_count: apiRuns.length, workflow_runs: apiRuns };
      }
      const identity = /\/actions\/runs\/(\d+)\/attempts\/1$/.exec(url.pathname);
      if (identity) {
        const run = runById.get(Number(identity[1]));
        if (run.id === 30000) {
          const error = new Error("not found");
          error.status = 404;
          throw error;
        }
        return {
          id: run.id,
          run_attempt: 1,
          head_sha: run.head_sha,
          event: run.event,
          head_branch: run.head_branch,
          status: "completed",
          conclusion: run.id === 30001 ? "failure" : "success",
        };
      }
      const jobs = /\/actions\/runs\/(\d+)\/attempts\/1\/jobs$/.exec(url.pathname);
      if (jobs) {
        const runId = Number(jobs[1]);
        return {
          total_count: jobIds.length,
          jobs: jobIds.map((name, index) => ({
            id: runId * 10 + index,
            name,
            status: "completed",
            conclusion: runId === 30001 && name === "test-macos" ? "failure" : "success",
            started_at: at(10 * 86400 + 10),
            completed_at: at(10 * 86400 + 20),
            steps: [{ number: 1, name: "test", status: "completed", conclusion: "success" }],
          })),
        };
      }
      throw new Error("unexpected API endpoint");
    } finally {
      if (timer) clearTimeout(timer);
      if (onAbort) signal.removeEventListener("abort", onAbort);
      activeRequests -= 1;
      completedRequests += 1;
    }
  };
  const request = createAsyncMetadataTransport(transport, { budgetMs: 5000 });
  try {
    const inventory = await paginateAsync(
      `https://api.github.com/repos/${config.repository.owner}/${config.repository.name}/actions/workflows/ci.yml/runs?branch=main&event=push&created=2026-09-11..2026-10-09`,
      "workflow_runs",
      config.ci.limits.maximum_runs,
      request,
    );
    assert.equal(inventory.length, 89);
    let historicalReads = 0;
    const metadata = await collectFirstAttemptMetadata({
      apiRuns: inventory,
      request,
      currentWorkflowDigest: currentDigest,
      windowStart,
      windowEnd,
      maximumRuns: config.ci.limits.maximum_runs,
      maximumJobsPerRun: config.ci.limits.maximum_jobs_per_run,
      readHistoricalWorkflow: (sha) => {
        historicalReads += 1;
        return workflowBySha.get(sha);
      },
    });
    assert.equal(historicalReads, workflowBySha.size, "historical workflow bytes are read once per SHA and classified per run");
    assert.equal(metadata.normalizedInputs.length, 19);
    assert.equal(metadata.runInventoryRows.length, 89);
    assert.equal(metadata.excludedWorkflowRuns.length, 70);
    assert.equal(Object.keys(metadata.runEvidence).length, 19);
    assert.equal(
      metadata.normalizedInputs.find((run) => run.id === 30000).status,
      "completed",
      "attempt-1 404 falls back only when no rerun exists",
    );
    assert.equal(metadata.normalizedInputs.find((run) => run.id === 30001).conclusion, "failure");
    assert.equal(metadata.normalizedInputs.find((run) => run.id === 30001).latest_conclusion, "success");
    const rerunJobs = metadata.normalizedInputs.find((run) => run.id === 30001).attempt1_jobs;
    assert.equal(rerunJobs.length, jobIds.length);
    assert.equal(rerunJobs.find((job) => job.name === "scope").conclusion, "success");
    assert.equal(rerunJobs.find((job) => job.name === "test-macos").conclusion, "failure");
    const cohort = normalizeCiCohort({
      runs: metadata.normalizedInputs,
      expectedJobIds: jobIds,
      windowStart,
      windowEnd,
      workflowConfigDigest: currentDigest,
      workflowPath: config.ci.workflow,
      repoRoot,
      preExcludedRuns: metadata.excludedWorkflowRuns,
    });
    assert.equal(cohort.runs.length, 19);
    assert.equal(cohort.excluded_runs.length, 70);
    assert.equal(
      cohort.runs.find((run) => run.run_ref === "ref:ci-run-30001-attempt-1").jobs.find((job) => job.job_ref === "ref:job-ci-test-macos")
        .conclusion,
      "failure",
    );
    const payload = buildCiFirstAttemptPayload({
      normalizedRuns: cohort.runs,
      windowStart,
      windowEnd,
      workflowConfigDigest: currentDigest,
      expectedJobIds: jobIds,
      runInventoryRef: "ref:ci-main-push-28-day-runs",
    });
    const objective = createObjective(
      config.scopes.find((scope) => scope.name === "ci-release"),
      {
        commitSha: commit,
        runRef,
        attempt: 1,
        observedAt,
      },
      {
        ci_first_attempt: {
          status: "measured",
          payload,
          tool: CI_API_TOOL,
          command: "GET /repos/{owner}/{repo}/actions/workflows/ci.yml/runs and attempt-1 jobs with pagination",
          included_refs: [payload.run_inventory_ref],
          excluded_refs: [],
          scope_policy: {
            workflow: config.ci.workflow,
            branch: config.ci.branch,
            event: config.ci.event,
            window_days: config.ci.window_days,
            expected_job_ids: jobIds,
            limits: config.ci.limits,
            attempt_policy: config.ci.attempt_policy,
            scope_version: "main-push-current-config-ci-v2",
          },
        },
      },
    );
    const validation = validateObjective(schema, objective);
    assert.equal(validation.valid, true, validation.schemaErrors.join("; "));
  } finally {
    request.close();
  }
  returned = true;
  const completedAtReturn = completedRequests;
  await Promise.resolve();
  assert.equal(activeRequests, 0);
  assert.equal(completedRequests, completedAtReturn);
  assert.equal(startsAfterReturn, 0);
  assert.equal(requestStarts, 1 + 19 * 2);
  assert.equal(maximumActiveRequests, 8);
});

test("async CI metadata cancels in-flight work and sanitizes diagnostics after a request fails", async () => {
  const workflow = Buffer.from("name: CI\njobs:\n  scope:\n  test-macos:\n");
  const apiRuns = Array.from({ length: 20 }, (_, index) => ({
    id: 70000 + index,
    head_sha: "c".repeat(40),
    run_attempt: 1,
    event: "push",
    head_branch: "main",
    created_at: at(10 * 86400),
    status: "completed",
    conclusion: "success",
  }));
  let active = 0;
  let peak = 0;
  let starts = 0;
  let startsAfterReturn = 0;
  let returned = false;
  const transport = async (rawUrl, _timeoutMs, signal) => {
    const url = new URL(rawUrl);
    const isFailingJobsRequest = url.pathname.endsWith("/runs/70000/attempts/1/jobs");
    starts += 1;
    if (returned) startsAfterReturn += 1;
    active += 1;
    peak = Math.max(peak, active);
    let timer;
    let onAbort;
    try {
      await new Promise((resolve, reject) => {
        timer = setTimeout(resolve, isFailingJobsRequest ? 2 : 100);
        onAbort = () => {
          clearTimeout(timer);
          reject(signal.reason);
        };
        signal.addEventListener("abort", onAbort, { once: true });
      });
      if (isFailingJobsRequest) {
        const error = new Error("https://api.github.com/private?token=PRIVATE_SENTINEL github_pat_PRIVATE_SENTINEL");
        error.status = 503;
        throw error;
      }
      const identity = /\/actions\/runs\/(\d+)\/attempts\/1$/.exec(url.pathname);
      if (identity) return { ...apiRuns.find((run) => run.id === Number(identity[1])), run_attempt: 1 };
      return { total_count: 2, jobs: [] };
    } finally {
      if (timer) clearTimeout(timer);
      if (onAbort) signal.removeEventListener("abort", onAbort);
      active -= 1;
    }
  };
  const request = createAsyncMetadataTransport(transport, { budgetMs: 5000 });
  let failure;
  try {
    await assert.rejects(
      collectFirstAttemptMetadata({
        apiRuns,
        request,
        currentWorkflowDigest: sha256(workflow),
        windowStart: "2026-09-11T10:00:00Z",
        windowEnd: "2026-10-09T10:00:00Z",
        readHistoricalWorkflow: () => workflow,
      }),
      (error) => {
        failure = error;
        return true;
      },
    );
  } finally {
    request.close();
    returned = true;
  }
  const startsAtReturn = starts;
  await Promise.resolve();
  assert.equal(active, 0);
  assert.equal(starts, startsAtReturn);
  assert.equal(startsAfterReturn, 0);
  assert.ok(peak <= 8);
  assert.match(failure.message, /attempt-1 jobs failed run_id=70000 request_count=/);
  assert.match(failure.message, /HTTP 503/);
  assert.doesNotMatch(failure.message, /PRIVATE_SENTINEL|github_pat_|https?:\/\/|token=/);
  assert.ok(starts < apiRuns.length * 2, "failure cancels queued metadata requests before all runs are dispatched");
});

test("async CI pagination checks completeness and the metadata transport enforces one deadline", async () => {
  const urls = [];
  const entries = await paginateAsync("https://api.github.com/repos/acme/app/runs", "entries", 200, async (url) => {
    urls.push(url);
    const page = Number(new URL(url).searchParams.get("page"));
    return { total_count: 101, entries: Array.from({ length: page === 1 ? 100 : 1 }, (_, index) => index) };
  });
  assert.equal(entries.length, 101);
  assert.deepEqual(
    urls.map((url) => Number(new URL(url).searchParams.get("page"))),
    [1, 2],
  );
  await assert.rejects(
    paginateAsync("https://api.github.com/repos/acme/app/runs", "entries", 200, async () => ({ total_count: 101, entries: [1] })),
    /pagination ended before total_count/,
  );

  let now = 0;
  const request = createAsyncMetadataTransport(
    async () => {
      now = 31;
      return { complete: true };
    },
    { budgetMs: 30, now: () => now },
  );
  try {
    await assert.rejects(request("https://api.github.com/test"), /exceeded its 30-second budget/);
    assert.equal(request.requestCount, 1);
  } finally {
    request.close();
  }
});

test("native async GitHub transport pins the API origin, rejects redirects, and keeps tokens out of errors", async () => {
  const previous = process.env.GH_TOKEN;
  const sentinel = "test-token-value-not-a-real-credential";
  process.env.GH_TOKEN = sentinel;
  const url = `https://api.github.com/repos/${config.repository.owner}/${config.repository.name}/actions/runs/123/attempts/1`;
  const controller = new AbortController();
  let captured;
  try {
    const body = await githubActionsJson(url, 1000, controller.signal, async (target, options) => {
      captured = { target, options };
      return new Response('{"id":123}', { status: 200 });
    });
    assert.deepEqual(body, { id: 123 });
    assert.equal(captured.target, url);
    assert.equal(captured.options.method, "GET");
    assert.equal(captured.options.redirect, "error");
    assert.equal(captured.options.headers.Authorization, `Bearer ${sentinel}`);
    const inventoryUrl = `https://api.github.com/repos/${config.repository.owner}/${config.repository.name}/actions/workflows/ci.yml/runs?branch=main&event=push&created=2026-09-11..2026-10-09&per_page=100&page=1`;
    await githubActionsJson(
      inventoryUrl,
      1000,
      controller.signal,
      async () => new Response('{"total_count":0,"workflow_runs":[]}', { status: 200 }),
    );
    await assert.rejects(
      githubActionsJson(`${url}?token=PRIVATE_SENTINEL`, 1000, controller.signal, async () => assert.fail("unexpected fetch")),
      /outside the fixed read-only endpoint set/,
    );
    let fetchFailure;
    await assert.rejects(
      githubActionsJson(url, 1000, controller.signal, async () => {
        fetchFailure = new Error(`redirect to https://example.test/?token=${sentinel}`);
        throw fetchFailure;
      }),
      (error) => error.message === "GitHub Actions API request failed" && error.cause === fetchFailure && !error.message.includes(sentinel),
    );
    let responseReadFailure;
    const failedBody = new Response(
      new ReadableStream({
        start(streamController) {
          responseReadFailure = new Error(`body from https://example.test/?token=${sentinel}`);
          streamController.error(responseReadFailure);
        },
      }),
      { status: 200 },
    );
    await assert.rejects(
      githubActionsJson(url, 1000, controller.signal, async () => failedBody),
      (error) =>
        error.message === "GitHub Actions API request failed" && error.cause === responseReadFailure && !error.message.includes(sentinel),
    );
    const currentWorkflow = Buffer.from("name: CI\njobs:\n  scope:\n");
    const diagnosticRequest = createAsyncMetadataTransport(
      (target, timeoutMs, signal) =>
        githubActionsJson(target, timeoutMs, signal, async () => {
          throw new Error(`request to https://example.test/?token=${sentinel}`);
        }),
      { budgetMs: 1000 },
    );
    try {
      await assert.rejects(
        collectFirstAttemptMetadata({
          apiRuns: [
            {
              id: 1234,
              head_sha: "d".repeat(40),
              run_attempt: 1,
              event: "push",
              head_branch: "main",
              created_at: at(10 * 86400),
              status: "completed",
              conclusion: "success",
            },
          ],
          request: diagnosticRequest,
          currentWorkflowDigest: sha256(currentWorkflow),
          windowStart: "2026-09-11T00:00:00Z",
          windowEnd: "2026-10-09T00:00:00Z",
          readHistoricalWorkflow: () => currentWorkflow,
        }),
        (error) => {
          assert.match(error.message, /attempt-1 identity failed run_id=1234 request_count=1/);
          assert.doesNotMatch(error.message, /PRIVATE_SENTINEL|https?:\/\/|token=/);
          assert.equal(error.cause, undefined, "public collector diagnostics do not retain the transport cause");
          return true;
        },
      );
    } finally {
      diagnosticRequest.close();
    }
    for (const status of [404, 503]) {
      let bodyCancelled = false;
      const response = new Response(
        new ReadableStream({
          cancel: async () => {
            await Promise.resolve();
            bodyCancelled = true;
          },
        }),
        { status },
      );
      await assert.rejects(
        githubActionsJson(url, 1000, controller.signal, async () => response),
        (error) => error.status === status && error.message === `GitHub Actions API returned HTTP ${status}`,
      );
      assert.equal(bodyCancelled, true, `HTTP ${status} response body is cancelled before the request settles`);
    }
  } finally {
    if (previous === undefined) delete process.env.GH_TOKEN;
    else process.env.GH_TOKEN = previous;
  }
});

test("CI cohort excludes historical workflow configurations and measures current runs only", () => {
  const oldCommit = "f".repeat(40);
  const oldWorkflow = Buffer.from("name: older CI workflow\n");
  const classified = classifyCiWorkflowConfiguration({
    runId: 41,
    commit: oldCommit,
    historicalWorkflow: oldWorkflow,
    expectedDigest: digest,
  });
  assert.equal(classified.included, false);
  assert.equal(classified.excluded.id, 41);
  assert.equal(classified.excluded.commit, oldCommit);
  assert.equal(classified.excluded.reason, "workflow-config-mismatch");
  assert.match(classified.excluded.workflow_config_digest, /^[a-f0-9]{64}$/);
  assert.throws(
    () =>
      classifyCiWorkflowConfiguration({
        runId: 41,
        commit: oldCommit,
        expectedDigest: digest,
      }),
    /historical workflow evidence is missing or malformed/,
  );

  const currentRun = {
    id: 42,
    head_sha: "c".repeat(40),
    run_attempt: 1,
    event: "push",
    head_branch: "main",
    created_at: at(0),
    workflow_config_digest: digest,
    attempt1_jobs: [
      { id: 1001, name: "scope", status: "completed", conclusion: "success", steps: [] },
      { id: 1002, name: "test-macos", status: "completed", conclusion: "success", steps: [] },
    ],
  };
  const normalized = normalizeCiCohort({
    runs: [currentRun],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    preExcludedRuns: [classified.excluded],
  });
  assert.equal(normalized.runs.length, 1);
  assert.equal(normalized.runs[0].overall_conclusion, "success");
  assert.deepEqual(normalized.excluded_runs, [classified.excluded]);
  assert.equal(ciCohortUnavailableReason(normalized), null);
  assert.equal(ciCohortUnavailableReason({ runs: [], incomplete_runs: [], excluded_runs: [classified.excluded] }), "not-collected");
  const args = {
    runs: [currentRun],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
  };
  assert.throws(() => normalizeCiCohort({ ...args, preExcludedRuns: [classified.excluded, classified.excluded] }), /duplicated/);
  assert.throws(() => normalizeCiCohort({ ...args, preExcludedRuns: [{ ...classified.excluded, id: currentRun.id }] }), /duplicate run ID/);
  assert.throws(
    () => normalizeCiCohort({ ...args, preExcludedRuns: [{ ...classified.excluded, workflow_config_digest: digest }] }),
    /malformed/,
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        ...args,
        runs: [{ ...currentRun, attempt1_jobs: [{ id: 1001, name: "unregistered", status: "completed", conclusion: "success" }] }],
      }),
    /unregistered job/,
  );
});

test("empty current CI cohort remains unknown while incomplete failed evidence remains failed", () => {
  const scope = config.scopes.find((item) => item.name === "ci-release");
  const context = { commitSha: commit, runRef, attempt: 1, observedAt };
  const empty = createObjective(scope, context, { ci_first_attempt: ciUnavailableMeasurement("not-collected") });
  assert.equal(empty.metrics.ci_first_attempt.status, "unknown");
  assert.equal(empty.metrics.ci_first_attempt.reason, "not-collected");
  assert.equal(empty.metrics.ci_first_attempt.payload, null);
  assert.equal(validateObjective(schema, empty).valid, true);
  const incomplete = createObjective(scope, context, { ci_first_attempt: ciUnavailableMeasurement("incomplete-input") });
  assert.equal(incomplete.metrics.ci_first_attempt.status, "failed");
  assert.equal(incomplete.metrics.ci_first_attempt.reason, "incomplete-input");
  assert.throws(() => ciUnavailableMeasurement("unsupported-tool"), /Invalid/);
});

test("CI gaps stay unknown and unproven startup or timeout conclusions stay failures with raw evidence", () => {
  const base = {
    id: 8,
    head_sha: "d".repeat(40),
    run_attempt: 1,
    event: "push",
    head_branch: "main",
    created_at: at(400),
    workflow_config_digest: digest,
    attempt1_jobs: [
      { id: 2001, name: "scope", status: "completed", conclusion: "startup_failure", steps: [] },
      { id: 2002, name: "test-macos", status: "completed", conclusion: "timed_out", steps: [] },
    ],
  };
  const normalized = normalizeCiCohort({
    runs: [base],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(
    normalized.runs[0].jobs.map((job) => job.conclusion),
    ["failure", "failure"],
  );
  assert.equal(normalized.runs[0].overall_conclusion, "failure");
  assert.deepEqual(
    normalized.timing_runs[0].jobs.map((job) => job.conclusion),
    ["startup_failure", "timed_out"],
  );
  const missingJob = normalizeCiCohort({
    runs: [{ ...base, attempt1_jobs: [{ id: 2001, name: "scope", status: "completed", conclusion: "startup_failure" }] }],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(
    missingJob.runs[0].jobs.map((job) => job.conclusion),
    ["failure", "unknown"],
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        runs: [{ ...base, created_at: "not-a-date" }],
        expectedJobIds: expectedJobs,
        windowStart: "2026-09-11T00:00:00Z",
        windowEnd: "2026-10-09T00:00:00Z",
        workflowConfigDigest: digest,
      }),
    /malformed identity or timestamp/,
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        runs: [{ ...base, workflow_config_digest: "e".repeat(64) }],
        expectedJobIds: expectedJobs,
        windowStart: "2026-09-11T00:00:00Z",
        windowEnd: "2026-10-09T00:00:00Z",
        workflowConfigDigest: digest,
      }),
    /configuration changed/,
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        runs: [
          { ...base, attempt1_jobs: [...base.attempt1_jobs, { id: 2003, name: "new-job", status: "completed", conclusion: "success" }] },
        ],
        expectedJobIds: expectedJobs,
        windowStart: "2026-09-11T00:00:00Z",
        windowEnd: "2026-10-09T00:00:00Z",
        workflowConfigDigest: digest,
      }),
    /unregistered job/,
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        runs: [{ ...base, id: "9" }],
        expectedJobIds: expectedJobs,
        windowStart: "2026-09-11T00:00:00Z",
        windowEnd: "2026-10-09T00:00:00Z",
        workflowConfigDigest: digest,
      }),
    /malformed identity or timestamp/,
  );
  assert.throws(
    () =>
      normalizeCiCohort({
        runs: [{ ...base, attempt1_jobs: [{ id: "2001", name: "scope", status: "completed", conclusion: "success" }] }],
        expectedJobIds: expectedJobs,
        windowStart: "2026-09-11T00:00:00Z",
        windowEnd: "2026-10-09T00:00:00Z",
        workflowConfigDigest: digest,
      }),
    /malformed identity/,
  );
});

test("failed workflow with mixed known and unknown jobs is retained but makes the CI slot incomplete", () => {
  const normalized = normalizeCiCohort({
    runs: [
      {
        id: 91,
        head_sha: "e".repeat(40),
        run_attempt: 1,
        event: "push",
        head_branch: "main",
        created_at: at(500),
        status: "completed",
        conclusion: "failure",
        workflow_config_digest: digest,
        attempt1_jobs: [{ id: 9101, name: "scope", status: "completed", conclusion: "success" }],
      },
    ],
    expectedJobIds: expectedJobs,
    windowStart: "2026-09-11T00:00:00Z",
    windowEnd: "2026-10-09T00:00:00Z",
    workflowConfigDigest: digest,
    repoRoot: "/workspace/sagascript",
  });
  assert.deepEqual(
    normalized.runs[0].jobs.map((job) => job.conclusion),
    ["success", "unknown"],
  );
  assert.equal(normalized.runs[0].overall_conclusion, "unknown");
  assert.deepEqual(normalized.incomplete_runs, [{ id: 91, status: "completed", conclusion: "failure" }]);
  assert.equal(normalized.timing_runs[0].status, "completed");
  assert.equal(normalized.timing_runs[0].conclusion, "failure");
});

test("CI workflow job inventory and pagination reject incomplete or duplicate evidence", () => {
  const workflow = `name: CI\njobs:\n  # Stable IDs are the cohort slots.\n  scope:\n    runs-on: ubuntu-latest\n  test-macos:\n    needs: scope\non:\n  push:\n`;
  assert.deepEqual(parseWorkflowJobIds(workflow), ["scope", "test-macos"]);
  assert.throws(() => parseWorkflowJobIds("name: empty\non:\n  push:\n"), /no jobs section/);
  assert.throws(() => parseWorkflowJobIds("jobs:\n  scope:\n  scope:\n"), /duplicated/);
  assert.doesNotThrow(() => assertCompletePageCount(120, 120, 1000));
  assert.throws(() => assertCompletePageCount(100, 120, 1000), /does not match total_count/);
  assert.throws(() => assertCompletePageCount(1001, 1001, 1000), /exceeds the configured limit/);
});

test("pinned CI cohort job registry matches the current workflow inventory", () => {
  const ciWorkflow = readFileSync(join(repoRoot, ".github/workflows/ci.yml"), "utf8");
  assert.deepEqual(parseWorkflowJobIds(ciWorkflow), config.ci.expected_job_ids);
});

test("contract scope registry creates six exact-schema snapshots with distinct slot identities", () => {
  const scopes = config.scopes;
  const snapshots = scopes.map((scope) => ({
    contract_version: "1.0",
    snapshot_id: "ref:sagascript-" + scope.name + "-" + commit.slice(0, 12),
    supersedes_ref: null,
    correction_ref: null,
    repository: { owner: "Magnus-Gille", name: "sagascript" },
    commit,
    observed_at: observedAt,
    metrics: Object.fromEntries(
      [
        ["complex_functions", "functions"],
        ["unused_candidates", "candidates"],
        ["coverage", "lines"],
        ["ci_first_attempt", "runs"],
        ["confirmed_regressions", "regressions"],
      ].map(([name, unit]) => {
        const slot = scope.slots[name];
        return [
          name,
          {
            observed_at: observedAt,
            source: null,
            population: null,
            unit,
            status: slot.status === "measured" ? "unknown" : slot.status,
            payload: null,
            reason: slot.status === "measured" ? "not-collected" : slot.reason,
            slot_ref: slot.slot_ref,
          },
        ];
      }),
    ),
  }));
  assert.equal(new Set(scopes.flatMap((scope) => Object.values(scope.slots).map((slot) => slot.slot_ref))).size, 30);
  for (const snapshot of snapshots) assert.equal(validateObjective(schema, snapshot).valid, true);
});

test("snapshot identities distinguish collections and attempts while replaying the same artifact", () => {
  const scope = config.scopes.find((item) => item.name === "rust-core");
  const context = (runRef, attempt) => ({
    commitSha: commit,
    observedAt,
    runRef,
    attempt,
  });
  const first = createObjective(scope, context("ref:github-run-7021-attempt-1", 1));
  const replay = createObjective(scope, context("ref:github-run-7021-attempt-1", 1));
  const rerun = createObjective(scope, context("ref:github-run-7021-attempt-2", 2));
  const otherRun = createObjective(scope, context("ref:github-run-7022-attempt-1", 1));
  const sameCommitPrefix = createObjective(scope, {
    ...context("ref:github-run-7021-attempt-1", 1),
    commitSha: "a".repeat(12) + "b".repeat(28),
  });

  assert.equal(first.snapshot_id, replay.snapshot_id);
  assert.notEqual(first.snapshot_id, rerun.snapshot_id);
  assert.notEqual(first.snapshot_id, otherRun.snapshot_id);
  assert.notEqual(first.snapshot_id, sameCommitPrefix.snapshot_id);
  const admission = validateObjectiveSeries(schema, [first, replay, rerun, otherRun, sameCommitPrefix]);
  assert.equal(admission.valid, true, admission.errors.join("; "));
  assert.equal(admission.replayCount, 1);
});

test("scope digest tracks registered policy rather than changing population membership", () => {
  const scope = config.scopes.find((item) => item.name === "rust-core");
  const context = { commitSha: commit, observedAt, runRef, attempt: 1 };
  const scopePolicy = {
    inventory: "configured-rust-source-files-v1",
    included_paths: ["src-tauri/crates/sagascript-core/src/download.rs"],
    excluded_paths: ["generated/**"],
    parser: config.complexity.function_inventory,
  };
  const objectiveFor = (includedRefs, policy = scopePolicy) =>
    createObjective(scope, context, {
      complex_functions: {
        status: "measured",
        payload: {
          algorithm: config.complexity.algorithm,
          threshold: config.complexity.threshold,
          eligible_functions: includedRefs.length,
          above_threshold_functions: 0,
        },
        tool: { name: "rust-code-analysis-cli", version: "0.0.25" },
        command: "rust-code-analysis-cli -p <selected-repository-path> -m -F -O json --pr",
        scope_policy: policy,
        included_refs: includedRefs,
        excluded_refs: [],
      },
    });
  const first = objectiveFor(["ref:function-a"]);
  const changedPopulation = objectiveFor(["ref:function-a", "ref:function-b"]);
  const changedPolicy = objectiveFor(["ref:function-a"], {
    ...scopePolicy,
    excluded_paths: ["generated/**", "vendor/**"],
  });
  assert.equal(seriesKey(schema, first, "complex_functions"), seriesKey(schema, changedPopulation, "complex_functions"));
  assert.notEqual(seriesKey(schema, first, "complex_functions"), seriesKey(schema, changedPolicy, "complex_functions"));
  assert.deepEqual(first.metrics.complex_functions.population.included_refs, ["ref:function-a"]);
  assert.deepEqual(changedPopulation.metrics.complex_functions.population.included_refs, ["ref:function-a", "ref:function-b"]);
});

test("report transport uses the root manifest shape and rejects unresolved population evidence", () => {
  const noMeasures = Object.fromEntries(config.scopes.map((scope) => [scope.name, {}]));
  const bundle = assembleReportBundle({
    scopes: config.scopes,
    measurementsByScope: noMeasures,
    context: { commitSha: commit, observedAt, runRef, attempt: 1 },
    evidenceFiles: { "evidence/collection-overhead.json": { elapsed_seconds: 0.5, measured: true } },
    config,
  });
  assert.deepEqual(Object.keys(bundle.manifest), [
    "contract_version",
    "repo_owner",
    "repo_name",
    "commit_sha",
    "snapshots",
    "evidence_index",
  ]);
  assert.equal(bundle.manifest.snapshots.length, 6);
  assert.equal(bundle.evidenceIndex.version, "1.0");
  for (const snapshot of bundle.snapshots) assert.equal(validateObjective(schema, snapshot).valid, true);
  assert.throws(
    () =>
      assembleReportBundle({
        scopes: [config.scopes[0]],
        measurementsByScope: {
          "rust-core": {
            complex_functions: {
              status: "measured",
              payload: { algorithm: "cyclomatic-complexity-v1", threshold: 10, eligible_functions: 2, above_threshold_functions: 1 },
              tool: { name: "rust-code-analysis-cli", version: "0.0.25" },
              command: "fixture",
              scope_policy: { inventory: "fixture-policy-v1" },
              included_refs: ["ref:missing-inventory"],
              excluded_refs: [],
            },
          },
        },
        context: { commitSha: commit, observedAt, runRef, attempt: 1 },
        config,
      }),
    /unresolved/,
  );
});

test("timestamps are normalized to the contract's UTC whole-second precision", () => {
  assert.equal(wholeSecondUtc("2026-10-09T10:00:00.987Z"), observedAt);
  assert.throws(() => wholeSecondUtc("invalid"), /invalid timestamp/);
});
