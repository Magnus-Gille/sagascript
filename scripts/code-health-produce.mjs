import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { basename, delimiter, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  assembleReportBundle,
  assertCompletePageCount,
  assertNoAbsoluteArtifactPaths,
  buildCiFirstAttemptPayload,
  normalizeCiCohort,
  parseExportedCoverage,
  parseKnipCandidates,
  parseRustComplexity,
  parseWorkflowJobIds,
  repoSourcePath,
  sha256,
  wholeSecondUtc,
} from "./lib/code-health-producer.mjs";
import { validateObjective } from "./lib/code-health-objective.mjs";
import { summarizeCohorts } from "./ci-cohort-timings.mjs";
import { summarizeRun } from "./ci-run-timings.mjs";
import { canonical } from "./lib/code-health-schema.mjs";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const TEMP_BASE = process.env.RUNNER_TEMP ?? "/private/tmp";
const CONFIG_PATH = join(REPO_ROOT, "docs/code-health-producer-v1.json");
const SCHEMA_PATH = join(REPO_ROOT, "docs/code-health-objective-v1.schema.json");
const TOOL_DIR = process.env.CODE_HEALTH_TOOL_DIR ?? "";
const MAX_CAPTURE_BYTES = 128 * 1024 * 1024;
const MAX_COMMAND_MS = 15 * 60 * 1000;
export const KNIP_ARGS = ["--reporter", "json", "--no-progress", "--no-config-hints", "--no-exit-code"];

const config = JSON.parse(readFileSync(CONFIG_PATH, "utf8"));
const schema = JSON.parse(readFileSync(SCHEMA_PATH, "utf8"));
const sourceRefs = new Map();
const evidenceRefs = {};
const evidenceFiles = {};
const measurementsByScope = Object.fromEntries(config.scopes.map(scope => [scope.name, {}]));
const collectionStatus = { producer_version: "1.0", collectors: {} };

export function parseProducerOptions(argv) {
  const parsed = { outputDir: null, mode: "static", includeCi: false, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--output-dir" && argv[index + 1]) parsed.outputDir = resolve(argv[++index]);
    else if (arg === "--mode" && argv[index + 1]) {
      parsed.mode = argv[++index];
      if (!["static", "ci-only"].includes(parsed.mode)) throw new Error("--mode must be static or ci-only");
    }
    else if (arg === "--ci") parsed.includeCi = true;
    else if (arg === "--help" || arg === "-h") parsed.help = true;
    else throw new Error(`unknown or incomplete argument: ${arg}`);
  }
  if (parsed.mode === "ci-only" && parsed.includeCi) throw new Error("--mode ci-only cannot be combined with --ci");
  return parsed;
}

function toolPath(name) {
  return TOOL_DIR ? join(TOOL_DIR, name) : name;
}

function command(commandName, args, options = {}) {
  const commandEnv = { ...process.env, ...(options.env ?? {}) };
  if (TOOL_DIR) commandEnv.PATH = `${TOOL_DIR}${delimiter}${commandEnv.PATH ?? ""}`;
  const result = spawnSync(commandName, args, {
    cwd: options.cwd ?? REPO_ROOT,
    env: commandEnv,
    encoding: "utf8",
    maxBuffer: MAX_CAPTURE_BYTES,
    timeout: options.timeout ?? MAX_COMMAND_MS,
    windowsHide: true,
  });
  if (result.error) throw new Error(`${commandName} could not run (${result.error.code ?? "spawn-error"})`);
  if (result.status !== 0) {
    const diagnostic = safeDiagnostic(result.stderr || result.stdout || `exit ${result.status}`);
    throw new Error(`${commandName} failed (${result.status ?? result.signal ?? "unknown"}): ${diagnostic}`);
  }
  return result.stdout;
}

function safeDiagnostic(value) {
  return String(value)
    .replaceAll(REPO_ROOT, "<repo>")
    .replace(/\/(?:Users|private|var|tmp|Applications|opt|Library|System|Volumes|home|usr)\/[A-Za-z0-9._/-]+/g, "<local-path>")
    .replace(/(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})/g, "<redacted-token>")
    .replace(/\s+/g, " ")
    .trim()
    .slice(-1600);
}

function git(args) {
  return command("git", args);
}

function gitTracked(prefix, extensions = null) {
  const paths = git(["ls-files", "-z", "--", prefix]).split("\0").filter(Boolean);
  return paths.filter(path => !extensions || extensions.some(extension => path.endsWith(extension))).sort();
}

function utcNow() {
  return wholeSecondUtc(new Date().toISOString());
}

function refForSource(path, line = 1, namespace = "source") {
  const safePath = repoSourcePath(path, REPO_ROOT);
  const key = `${namespace}\0${safePath}\0${line}`;
  if (!sourceRefs.has(key)) {
    const ref = `ref:src-${createHash("sha256").update(key).digest("hex").slice(0, 40)}`;
    sourceRefs.set(key, ref);
    evidenceRefs[ref] = { kind: "source", path: safePath, line };
  }
  return sourceRefs.get(key);
}

function evidenceRef(name) {
  return `ref:evidence-${name.replace(/[^a-z0-9-]/gi, "-").toLowerCase()}`;
}

function addEvidenceFile(path, data, ref = null) {
  evidenceFiles[path] = data;
  if (ref) evidenceRefs[ref] = { kind: "artifact-file", path };
}

function recordCollector(name, actualVersion, work) {
  const start = performance.now();
  try {
    const value = work();
    collectionStatus.collectors[name] = {
      status: "measured",
      actual_tool_version: actualVersion,
      elapsed_seconds: Number(((performance.now() - start) / 1000).toFixed(3)),
      error: null,
    };
    return { status: "measured", value };
  } catch (error) {
    collectionStatus.collectors[name] = {
      status: "failed",
      actual_tool_version: actualVersion,
      elapsed_seconds: Number(((performance.now() - start) / 1000).toFixed(3)),
      error: safeDiagnostic(error?.message ?? error),
    };
    return { status: "failed", error };
  }
}

function toolVersion(executable, args, options = {}) {
  return safeDiagnostic(command(executable, args, options)).replace(/\s+/g, " ").trim().slice(0, 160);
}

function setFailure(scopeName, metricName, reason = "producer-error") {
  measurementsByScope[scopeName][metricName] = { status: "failed", reason };
}

function setMeasured(scopeName, metricName, value) {
  measurementsByScope[scopeName][metricName] = { status: "measured", ...value };
}

function addSourceInventoryEvidence(name, paths) {
  const path = `evidence/inventory-${name}.json`;
  const ref = evidenceRef(`inventory-${name}`);
  addEvidenceFile(path, { inventory: paths, inventory_kind: "git-tracked-source" }, ref);
  return { path, ref };
}

function collectRustComplexity(scopeName, files, executable, actualVersion) {
  const result = recordCollector(`complexity-${scopeName}`, actualVersion, () => {
    if (!files.length) throw new Error("selected complexity source inventory is empty");
    const reports = files.map(path => {
      const stdout = command(executable, ["-p", path, "-m", "-F", "-O", "json", "--pr"]);
      const parsed = JSON.parse(stdout);
      const roots = Array.isArray(parsed) ? parsed : [parsed];
      if (roots.length !== 1 || !roots[0] || !Array.isArray(roots[0].spaces)) {
        throw new Error(`analyzer output for ${path} is malformed`);
      }
      return { path, ast: roots[0] };
    });
    const payload = parseRustComplexity(reports, config.complexity.threshold);
    if (payload.functions.length > 1000) throw new Error("selected complexity inventory exceeds the contract reference limit");
    const refs = [...new Set(payload.functions.map(item => refForSource(item.path, item.start_line)))];
    const inventory = addSourceInventoryEvidence(scopeName, files);
    const functionEvidencePath = `evidence/functions-${scopeName}.json`;
    addEvidenceFile(functionEvidencePath, {
      algorithm: payload.algorithm,
      threshold: payload.threshold,
      source_inventory_ref: inventory.ref,
      functions: payload.functions,
    }, evidenceRef(`functions-${scopeName}`));
    return {
      payload: {
        algorithm: payload.algorithm,
        threshold: payload.threshold,
        eligible_functions: payload.eligible_functions,
        above_threshold_functions: payload.above_threshold_functions,
      },
      included_refs: refs,
      excluded_refs: [],
      tool: { name: "rust-code-analysis-cli", version: actualVersion },
      command: "rust-code-analysis-cli -p <selected-repository-path> -m -F -O json --pr",
    };
  });
  if (result.status === "measured") setMeasured(scopeName, "complex_functions", result.value);
  else setFailure(scopeName, "complex_functions");
}

function collectRustCoverage(tempRoot, cargoVersion, llvmCovVersion, rustVersion, useNamedToolchain) {
  const scopeName = "rust-core";
  const packageRoot = join(REPO_ROOT, "src-tauri");
  const coveragePath = join(tempRoot, "rust-core-coverage.json");
  const isolatedTarget = join(tempRoot, "cargo-target-rust-core");
  mkdirSync(isolatedTarget, { recursive: true });
  const env = {
    ...process.env,
    CARGO_TARGET_DIR: isolatedTarget,
    CARGO_BUILD_JOBS: "2",
    LLVM_PROFILE_FILE: join(tempRoot, "rust-core-%p-%m.profraw"),
    TMPDIR: tempRoot,
  };
  delete env.CARGO_ENCODED_RUSTFLAGS;
  const result = recordCollector("coverage-rust-core", `${rustVersion}; ${cargoVersion}; ${llvmCovVersion}`, () => {
    const toolchainArgs = useNamedToolchain ? ["+1.93.1"] : [];
    command("cargo", [
      ...toolchainArgs, "llvm-cov", "-p", "sagascript-core", "--no-default-features", "--lib", "--tests",
      "--json", "--output-path", coveragePath,
    ], { cwd: packageRoot, env });
    const raw = JSON.parse(readFileSync(coveragePath, "utf8"));
    const inventory = [
      ...gitTracked("src-tauri/crates/sagascript-core/src/", [".rs"]),
      ...gitTracked("src-tauri/crates/sagascript-core/tests/", [".rs"]),
    ].sort();
    const profileRef = evidenceRef("profile-rust-core-no-default-inline");
    const parsed = parseExportedCoverage(raw, {
      repoRoot: REPO_ROOT,
      inventoryPaths: inventory,
      sourceRoots: [
        "src-tauri/crates/sagascript-core/src/",
        "src-tauri/crates/sagascript-core/tests/",
      ],
      excludedPaths: ["src-tauri/crates/sagascript-core/src/generated/BuildInfo.rs"],
      includesInlineTests: true,
    });
    if (parsed.included_paths.length > 1000 || parsed.not_emitted_paths.length > 1000) {
      throw new Error("coverage path inventory exceeds the contract reference limit");
    }
    const inventoryEvidence = addSourceInventoryEvidence("rust-core-no-default", inventory);
    const includedRefs = parsed.included_paths.map(path => refForSource(path, 1, "coverage"));
    const excludedRefs = parsed.not_emitted_paths.map(path => refForSource(path, 1, "coverage-excluded"));
    const profilePath = "evidence/profile-rust-core-no-default-inline.json";
    addEvidenceFile(profilePath, {
      profile: "sagascript-core no-default-features; --lib --tests; inline tests included",
      tool: { rustc: rustVersion, cargo_llvm_cov: llvmCovVersion },
      inventory_ref: inventoryEvidence.ref,
      measure: parsed.measure,
      covered: parsed.covered,
      eligible: parsed.eligible,
      emitted_source_files: parsed.emitted_source_files,
      included_paths: parsed.included_paths,
      not_emitted_paths: parsed.not_emitted_paths,
      excluded_report_paths: parsed.excluded_report_paths,
    }, profileRef);
    return {
      payload: parsed,
      included_refs: includedRefs,
      excluded_refs: excludedRefs,
      profile_ref: profileRef,
      tool: { name: "cargo-llvm-cov", version: llvmCovVersion },
      command: `cargo ${useNamedToolchain ? "+1.93.1 " : ""}llvm-cov -p sagascript-core --no-default-features --lib --tests --json --output-path <task-temporary-report>`,
    };
  });
  if (result.status === "measured") setMeasured(scopeName, "coverage", result.value);
  else setFailure(scopeName, "coverage");
}

function collectSwiftCoverage(tempRoot) {
  const sourcePackageRoot = join(REPO_ROOT, "src-tauri/engine-host/coreml");
  const packageRoot = join(tempRoot, "swift-package");
  const scratchPath = join(tempRoot, "swift-scratch");
  const result = recordCollector("coverage-swift-coreml", "runner-provided", () => {
    const versionOutput = toolVersion("swift", ["--version"]);
    const swiftVersion = /Apple Swift version\s+([0-9.]+)/.exec(versionOutput)?.[1];
    if (!swiftVersion) throw new Error("could not parse the active Swift compiler version");
    cpSync(sourcePackageRoot, packageRoot, {
      recursive: true,
      filter: path => !path.split(sep).some(part => [".build", "recordings", "transcripts"].includes(part)),
    });
    const versionText = JSON.parse(readFileSync(join(REPO_ROOT, "src-tauri/tauri.conf.json"), "utf8")).version;
    const buildInfo = [
      "// Generated only in the task-temporary source copy.",
      "import Foundation",
      "public enum BuildInfo {",
      `    public static let version = ${JSON.stringify(versionText)}`,
      `    public static let gitSHA = ${JSON.stringify(git(["rev-parse", "HEAD"]).trim())}`,
      "    public static let dirty = false",
      "}",
      "",
    ].join("\n");
    writeFileSync(join(packageRoot, "Sources/EngineHostCore/BuildInfo.swift"), buildInfo);
    const codecovPath = command("swift", [
      "test", "--enable-code-coverage", "--jobs", "2", "--show-codecov-path",
      "--scratch-path", scratchPath,
    ], { cwd: packageRoot, env: { TMPDIR: tempRoot } }).trim().split(/\r?\n/).at(-1);
    if (!codecovPath || !resolve(codecovPath).startsWith(resolve(scratchPath) + sep)) {
      throw new Error("Swift reported a coverage path outside task-local scratch space");
    }
    const raw = JSON.parse(readFileSync(codecovPath, "utf8"));
    const packagePrefix = resolve(packageRoot) + sep;
    for (const datum of raw.data ?? []) {
      if (!Array.isArray(datum.files)) throw new Error("Swift coverage report file list is malformed");
      for (const file of datum.files) {
        const path = resolve(file.filename);
        if (!path.startsWith(packagePrefix)) throw new Error("Swift coverage report contains a path outside the temporary source copy");
        const localPath = relative(packageRoot, path).split(sep).join("/");
        file.filename = join(sourcePackageRoot, localPath);
      }
    }
    const inventory = gitTracked("src-tauri/engine-host/coreml/Sources/EngineHostCore/", [".swift"])
      .filter(path => basename(path) !== "BuildInfo.swift");
    const profileRef = evidenceRef("profile-swift-coreml-source-only");
    const parsed = parseExportedCoverage(raw, {
      repoRoot: REPO_ROOT,
      inventoryPaths: inventory,
      sourceRoots: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/"],
      excludedPaths: ["src-tauri/engine-host/coreml/Sources/EngineHostCore/BuildInfo.swift"],
    });
    const inventoryEvidence = addSourceInventoryEvidence("swift-coreml-source-only", inventory);
    const includedRefs = parsed.included_paths.map(path => refForSource(path, 1, "coverage"));
    const excludedRefs = parsed.not_emitted_paths.map(path => refForSource(path, 1, "coverage-excluded"));
    addEvidenceFile("evidence/profile-swift-coreml-source-only.json", {
      profile: "Swift Core ML host; source-only; generated BuildInfo and test bundle excluded",
      tool: { swift: swiftVersion },
      inventory_ref: inventoryEvidence.ref,
      measure: parsed.measure,
      covered: parsed.covered,
      eligible: parsed.eligible,
      emitted_source_files: parsed.emitted_source_files,
      included_paths: parsed.included_paths,
      not_emitted_paths: parsed.not_emitted_paths,
      excluded_report_paths: parsed.excluded_report_paths,
    }, profileRef);
    return {
      payload: parsed,
      included_refs: includedRefs,
      excluded_refs: excludedRefs,
      profile_ref: profileRef,
      tool: { name: "swift", version: swiftVersion },
      command: "swift test --enable-code-coverage --jobs 2 --show-codecov-path --scratch-path <task-temporary-directory>",
    };
  });
  if (result.status === "measured") setMeasured("swift-coreml", "coverage", result.value);
  else setFailure("swift-coreml", "coverage");
}

function collectKnip(tempRoot, knipPath) {
  const versionResult = recordCollector("unused-candidates", "5.46.0", () => {
    const version = toolVersion(knipPath, ["--version"]);
    installPinnedToolVersion(version, "5.46.0", "knip");
    const output = command(knipPath, KNIP_ARGS, {
      env: { ...process.env, NO_COLOR: "1" },
    });
    return { report: JSON.parse(output), version: "5.46.0" };
  });
  if (versionResult.status !== "measured") {
    setFailure("typescript", "unused_candidates");
    setFailure("svelte", "unused_candidates");
    return;
  }
  for (const language of ["typescript", "svelte"]) {
    const scopeName = language;
    const inventory = gitTracked("src/", language === "svelte" ? [".svelte"] : [".ts", ".tsx"]);
    const parsed = recordCollector(`unused-candidates-${scopeName}`, "knip 5.46.0", () => {
      const candidates = parseKnipCandidates(versionResult.value.report, { repoRoot: REPO_ROOT, language });
      const scopedCandidates = candidates.filter(candidate => inventory.includes(candidate.path));
      const candidatesPath = `evidence/knip-candidates-${scopeName}.json`;
      const candidateRef = evidenceRef(`knip-candidates-${scopeName}`);
      addEvidenceFile(candidatesPath, {
        tool: { name: "knip", version: versionResult.value.version },
        language,
        graph_status: "unqualified",
        candidates: scopedCandidates,
        note: "Every candidate is uncertain until owner validation; this evidence grants no deletion authority.",
      }, candidateRef);
      const inventoryEvidence = addSourceInventoryEvidence(`knip-${scopeName}`, inventory);
      return {
        payload: {
          graph_status: "unqualified",
          candidate_count: scopedCandidates.length,
          validated_count: 0,
          false_positive_count: 0,
          uncertain_count: scopedCandidates.length,
          owner_validation_refs: [],
          graph_qualification_ref: null,
        },
        included_refs: [candidateRef],
        excluded_refs: [],
        tool: { name: "knip", version: versionResult.value.version },
        command: "knip --reporter json --no-progress --no-config-hints --no-exit-code",
      };
    });
    if (parsed.status === "measured") setMeasured(scopeName, "unused_candidates", parsed.value);
    else setFailure(scopeName, "unused_candidates");
  }
}

function restJson(url) {
  const token = process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN;
  if (!token) throw new Error("CI artifact collection requires the scoped GH_TOKEN environment value");
  const curlConfig = [
    `url = "${url}"`,
    'header = "Accept: application/vnd.github+json"',
    'header = "X-GitHub-Api-Version: 2022-11-28"',
    `header = "Authorization: Bearer ${token}"`,
    "",
  ].join("\n");
  const response = spawnSync("curl", [
    "--config", "-", "--silent", "--show-error", "--location", "--max-time", "60",
    "--write-out", "\n__HTTP_STATUS__%{http_code}",
  ], { input: curlConfig, encoding: "utf8", maxBuffer: 8 * 1024 * 1024, windowsHide: true });
  if (response.error || response.status !== 0) throw new Error("GitHub Actions API request failed");
  const httpResult = /\n__HTTP_STATUS__(\d{3})\s*$/.exec(response.stdout);
  if (!httpResult) throw new Error("GitHub Actions API response status is missing");
  const httpStatus = Number(httpResult[1]);
  const responseBody = response.stdout.slice(0, httpResult.index);
  if (httpStatus === 404) {
    const error = new Error("GitHub Actions API returned HTTP 404");
    error.status = 404;
    throw error;
  }
  if (httpStatus < 200 || httpStatus >= 300) throw new Error(`GitHub Actions API returned HTTP ${httpStatus}`);
  let value;
  try { value = JSON.parse(responseBody); } catch { throw new Error("GitHub Actions API returned invalid JSON"); }
  return value;
}

function paginate(url, collectionKey, maxItems) {
  let page = 1;
  let expectedTotal = null;
  const all = [];
  while (true) {
    const separator = url.includes("?") ? "&" : "?";
    const body = restJson(`${url}${separator}per_page=100&page=${page}`);
    if (!Number.isSafeInteger(body.total_count) || !Array.isArray(body[collectionKey])) {
      throw new Error(`GitHub Actions API ${collectionKey} inventory is malformed`);
    }
    if (expectedTotal === null) expectedTotal = body.total_count;
    if (body.total_count !== expectedTotal) throw new Error("GitHub Actions API total changed during pagination");
    all.push(...body[collectionKey]);
    if (all.length > maxItems || body.total_count > maxItems) throw new Error("GitHub Actions API inventory exceeds the configured limit");
    if (all.length >= expectedTotal) break;
    if (body[collectionKey].length !== 100) throw new Error("GitHub Actions API pagination ended before total_count");
    page += 1;
  }
  assertCompletePageCount(all.length, expectedTotal, maxItems);
  return all;
}

function ciWindow(observedAt) {
  const endMillis = Date.parse(observedAt);
  return {
    windowStart: new Date(endMillis - config.ci.window_days * 86400000).toISOString().replace(".000Z", "Z"),
    windowEnd: observedAt,
  };
}

function collectCi(observedAt) {
  const scopeName = "ci-release";
  const latestWorkflow = readFileSync(join(REPO_ROOT, config.ci.workflow));
  const currentWorkflowDigest = sha256(latestWorkflow);
  const expectedJobIds = config.ci.expected_job_ids;
  const result = recordCollector("ci-first-attempt", "GitHub Actions REST API 2022-11-28", () => {
    const actualJobIds = parseWorkflowJobIds(latestWorkflow.toString("utf8"));
    if (canonical(actualJobIds) !== canonical(expectedJobIds)) {
      throw new Error("CI workflow job inventory differs from the pinned cohort registry");
    }
    const { windowStart, windowEnd } = ciWindow(observedAt);
    const dateFilter = `${windowStart.slice(0, 10)}..${windowEnd.slice(0, 10)}`;
    const runsUrl = `https://api.github.com/repos/${config.repository.owner}/${config.repository.name}/actions/workflows/ci.yml/runs?branch=main&event=push&created=${encodeURIComponent(dateFilter)}`;
    const apiRuns = paginate(runsUrl, "workflow_runs", config.ci.limits.maximum_runs);
    const normalizedInputs = [];
    const runEvidence = {};
    for (const apiRun of apiRuns) {
      if (!Number.isSafeInteger(apiRun.id) || !Number.isSafeInteger(apiRun.run_attempt) || !/^[a-f0-9]{40}$/.test(apiRun.head_sha)
        || typeof apiRun.created_at !== "string" || !Number.isFinite(Date.parse(apiRun.created_at))) {
        throw new Error("GitHub Actions workflow run inventory contains a malformed identity or timestamp");
      }
      const sha = apiRun.head_sha;
      const historicalWorkflow = command("git", ["show", `${sha}:${config.ci.workflow}`]);
      const historicalDigest = sha256(historicalWorkflow);
      if (historicalDigest !== currentWorkflowDigest) {
        throw new Error("CI workflow configuration changed inside the 28-day cohort");
      }
      let attempt1Jobs = [];
      try {
        attempt1Jobs = paginate(
          `https://api.github.com/repos/${config.repository.owner}/${config.repository.name}/actions/runs/${apiRun.id}/attempts/1/jobs`,
          "jobs",
          config.ci.limits.maximum_jobs_per_run,
        );
      } catch (error) {
        if (error?.status !== 404) throw error;
        // A missing attempt-1 endpoint is retained as unknown for each registered job.
      }
      const runRef = `ref:ci-run-${apiRun.id}-attempt-1`;
      runEvidence[runRef] = { kind: "github-run", run_id: apiRun.id, attempt: 1 };
      normalizedInputs.push({
        id: apiRun.id,
        head_sha: sha,
        run_attempt: apiRun.run_attempt,
        event: apiRun.event,
        head_branch: apiRun.head_branch,
        created_at: apiRun.created_at,
        workflow_path: config.ci.workflow,
        workflow_config_digest: historicalDigest,
        attempt1_jobs: attempt1Jobs.map(job => ({
          id: job.id,
          name: job.name,
          status: job.status,
          conclusion: job.conclusion,
          started_at: job.started_at,
          completed_at: job.completed_at,
          steps: job.steps,
        })),
      });
    }
    const cohort = normalizeCiCohort({
      runs: normalizedInputs,
      expectedJobIds,
      windowStart,
      windowEnd,
      workflowConfigDigest: currentWorkflowDigest,
      workflowPath: config.ci.workflow,
      repoRoot: REPO_ROOT,
    });
    const payload = buildCiFirstAttemptPayload({
      normalizedRuns: cohort.runs,
      windowStart,
      windowEnd,
      workflowConfigDigest: currentWorkflowDigest,
      expectedJobIds,
      runInventoryRef: evidenceRef("ci-main-push-28-day-runs"),
    });
    const runInventoryPath = "evidence/ci-main-push-28-day-runs.json";
    addEvidenceFile(runInventoryPath, {
      source: "GitHub Actions REST API",
      workflow_path: config.ci.workflow,
      workflow_config_digest: currentWorkflowDigest,
      filter: { branch: "main", event: "push", window_start: windowStart, window_end: windowEnd },
      api_total_count: apiRuns.length,
      included_run_ids: cohort.runs.map(run => Number(run.run_ref.match(/ci-run-(\d+)-attempt-1/)[1])).sort((a, b) => a - b),
      excluded_runs: cohort.excluded_runs,
    }, payload.run_inventory_ref);
    Object.assign(evidenceRefs, runEvidence);
    const workflowRef = "ref:workflow-ci-main-push";
    const workflowPath = "evidence/ci-workflow.json";
    addEvidenceFile(workflowPath, {
      path: config.ci.workflow,
      digest: { algorithm: "sha256", value: currentWorkflowDigest },
      expected_job_ids: expectedJobIds,
    });
    for (const jobId of expectedJobIds) {
      evidenceRefs[`ref:job-ci-${jobId}`] = { kind: "artifact-file", path: "evidence/ci-job-inventory.json" };
    }
    addEvidenceFile("evidence/ci-job-inventory.json", { job_ids: expectedJobIds });
    const runRef = `ref:github-run-${process.env.GITHUB_RUN_ID}-attempt-${process.env.GITHUB_RUN_ATTEMPT ?? "1"}`;
    evidenceRefs[runRef] = {
      kind: "github-run",
      run_id: Number(process.env.GITHUB_RUN_ID),
      attempt: Number(process.env.GITHUB_RUN_ATTEMPT ?? 1),
    };
    const timingRows = cohort.timing_runs.map(run => ({
      databaseId: run.databaseId,
      headSha: run.headSha,
      workflowName: run.workflowName,
      event: run.event,
      attempt: run.attempt,
      status: run.status,
      conclusion: run.conclusion,
      createdAt: run.createdAt,
      jobs: run.jobs.map(job => ({
        databaseId: job.databaseId,
        name: job.name,
        status: job.status,
        conclusion: job.conclusion,
        startedAt: job.startedAt,
        completedAt: job.completedAt,
        steps: job.steps,
      })),
    }));
    const timingRunSummaries = timingRows.map(run => summarizeRun(run));
    const timingCohorts = timingRows.length ? summarizeCohorts(timingRows) : { cohorts: [], note: "No CI runs fell in the closed cohort window." };
    addEvidenceFile("evidence/ci-attempt1-timings.json", {
      source: "scripts/ci-run-timings.mjs and scripts/ci-cohort-timings.mjs",
      attempt_policy: "attempt 1 only",
      per_run: timingRunSummaries,
      cohorts: timingCohorts,
    });
    void workflowRef;
    return {
      payload,
      included_refs: [payload.run_inventory_ref],
      excluded_refs: [],
      tool: { name: "GitHub Actions REST API", version: "2022-11-28" },
      command: "GET /repos/{owner}/{repo}/actions/workflows/ci.yml/runs and attempt-1 jobs with pagination",
      workflowRef,
    };
  });
  if (result.status === "measured") setMeasured(scopeName, "ci_first_attempt", result.value);
  else setFailure(scopeName, "ci_first_attempt", "incomplete-input");
}

function installPinnedToolVersion(value, expected, label) {
  if (!value) throw new Error(`${label} runtime version does not match pinned ${expected}`);
  const escaped = expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = String(value).match(new RegExp(`\\b${escaped}\\b`));
  if (!match) throw new Error(`${label} runtime version does not match pinned ${expected}`);
  return match[0];
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
}

export function assertCleanCheckout(statusOutput) {
  if (typeof statusOutput !== "string") throw new TypeError("Git checkout status is malformed");
  if (statusOutput.length > 0) throw new Error("code-health collection requires a clean checkout, including no untracked files");
}

export function workflowRunContext(env, commitSha, repoRoot) {
  if (!env.GITHUB_ACTIONS) return null;
  if (env.GITHUB_ACTIONS !== "true") throw new Error("GitHub Actions identity flag is malformed");
  if (env.GITHUB_REPOSITORY !== `${config.repository.owner}/${config.repository.name}`) {
    throw new Error("GitHub repository identity does not match the configured producer repository");
  }
  if (!/^[a-f0-9]{40}$/.test(commitSha) || env.GITHUB_SHA !== commitSha) {
    throw new Error("GitHub commit identity does not match the checked-out revision");
  }
  if (typeof env.GITHUB_WORKSPACE !== "string" || !isAbsolute(env.GITHUB_WORKSPACE)
    || resolve(env.GITHUB_WORKSPACE) !== resolve(repoRoot)) {
    throw new Error("GitHub workspace does not match the producer checkout");
  }
  const runId = typeof env.GITHUB_RUN_ID === "string" && /^[1-9]\d*$/.test(env.GITHUB_RUN_ID)
    ? Number(env.GITHUB_RUN_ID) : NaN;
  const attempt = typeof env.GITHUB_RUN_ATTEMPT === "string" && /^[1-9]\d*$/.test(env.GITHUB_RUN_ATTEMPT)
    ? Number(env.GITHUB_RUN_ATTEMPT) : NaN;
  if (!Number.isSafeInteger(runId) || runId < 1 || !Number.isSafeInteger(attempt) || attempt < 1) {
    throw new Error("GitHub run identity is missing or malformed");
  }
  return { runId, attempt, runRef: `ref:github-run-${runId}-attempt-${attempt}` };
}

function validateCiRunContext(commitSha) {
  if (!process.env.GITHUB_ACTIONS) throw new Error("CI collection is only supported inside the standalone GitHub Actions workflow");
  const context = workflowRunContext(process.env, commitSha, REPO_ROOT);
  if (!context) throw new Error("GitHub run identity is missing");
  return context;
}

function main() {
  const options = parseProducerOptions(process.argv.slice(2));
  if (options.help) {
    process.stdout.write("Usage: node scripts/code-health-produce.mjs --output-dir PATH [--mode static|ci-only] [--ci]\n");
    return;
  }
  if (!options.outputDir) throw new Error("--output-dir is required; report output must be explicit");
  const commitSha = git(["rev-parse", "HEAD"]).trim();
  if (!/^[a-f0-9]{40}$/.test(commitSha)) throw new Error("checkout HEAD is not a full commit SHA");
  assertCleanCheckout(git(["status", "--porcelain=v1", "--untracked-files=all"]));
  const workflowContext = workflowRunContext(process.env, commitSha, REPO_ROOT);
  const started = performance.now();
  const observedAt = utcNow();
  mkdirSync(TEMP_BASE, { recursive: true });
  const tempRoot = mkdtempSync(join(TEMP_BASE, "sagascript-code-health-"));
  const outputDir = options.outputDir;
  const ciOnly = options.mode === "ci-only";
  const includeCi = ciOnly || options.includeCi;
  const localRunRef = `ref:collection-run-${commitSha.slice(0, 12)}-attempt-1`;
  let runRef = workflowContext?.runRef ?? localRunRef;
  let attempt = workflowContext?.attempt ?? 1;
  try {
    collectionStatus.mode = options.mode;
    if (!ciOnly) {
      const rustVersionResult = recordCollector("toolchain-rust", config.tools.rust_toolchain, () => {
        const version = toolVersion("rustc", ["--version"]);
        return installPinnedToolVersion(version, config.tools.rust_toolchain, "rustc");
      });
      const rustVersion = rustVersionResult.status === "measured" ? rustVersionResult.value : "unknown";
      const namedToolchainInstalled = recordCollector("toolchain-rustup-name", config.tools.rust_toolchain, () => {
        const installed = command("rustup", ["toolchain", "list"]);
        const escaped = config.tools.rust_toolchain.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
        return installed.split(/\r?\n/).some(line => new RegExp(`^${escaped}(?:[-\\s]|$)`).test(line));
      });
      const useNamedToolchain = namedToolchainInstalled.status === "measured" && namedToolchainInstalled.value;
      const cargoVersion = recordCollector("tool-cargo-llvm-cov", config.tools.cargo_llvm_cov, () => {
        const version = toolVersion("cargo", ["llvm-cov", "--version"]);
        return installPinnedToolVersion(version, config.tools.cargo_llvm_cov, "cargo-llvm-cov");
      });
      const cargoCovActual = cargoVersion.status === "measured" ? cargoVersion.value : null;
      const cargoToolchainVersion = recordCollector("tool-cargo", config.tools.rust_toolchain, () => {
        return installPinnedToolVersion(toolVersion("cargo", ["--version"]), config.tools.rust_toolchain, "cargo");
      });
      const rcaVersionResult = recordCollector("tool-rust-code-analysis", config.tools.rust_code_analysis_cli, () => {
        const version = toolVersion(toolPath("rust-code-analysis-cli"), ["--version"]);
        return installPinnedToolVersion(version, config.tools.rust_code_analysis_cli, "rust-code-analysis-cli");
      });
      const rcaActual = rcaVersionResult.status === "measured" ? rcaVersionResult.value : null;
      if (rcaActual) {
        for (const [scope, files] of Object.entries(config.complexity.scopes)) {
          collectRustComplexity(scope, files, toolPath("rust-code-analysis-cli"), rcaActual);
        }
      } else {
        for (const scope of Object.keys(config.complexity.scopes)) setFailure(scope, "complex_functions");
      }
      if (rustVersion !== "unknown" && cargoCovActual && cargoToolchainVersion.status === "measured") {
        collectRustCoverage(tempRoot, cargoToolchainVersion.value, cargoCovActual, rustVersion, useNamedToolchain);
      } else setFailure("rust-core", "coverage");

      collectSwiftCoverage(tempRoot);

      const knipPath = process.env.CODE_HEALTH_KNIP ?? (TOOL_DIR ? join(TOOL_DIR, "knip") : "knip");
      collectKnip(tempRoot, knipPath);
    }

    let ciRuntime = null;
    if (includeCi) {
      ciRuntime = validateCiRunContext(commitSha);
      runRef = ciRuntime.runRef;
      attempt = ciRuntime.attempt;
      collectCi(observedAt);
    }
    addEvidenceFile("evidence/ci-workflow.json", evidenceFiles["evidence/ci-workflow.json"]
      ?? { path: config.ci.workflow, digest: { algorithm: "sha256", value: sha256(readFileSync(join(REPO_ROOT, config.ci.workflow))) }, expected_job_ids: config.ci.expected_job_ids });

    const elapsedSeconds = Number(((performance.now() - started) / 1000).toFixed(3));
    addEvidenceFile("evidence/collection-overhead.json", {
      elapsed_seconds: elapsedSeconds,
      elapsed_definition: "measurement and evidence collection phase through tool/API aggregation; excludes final serialization/upload",
      measured: true,
      local_or_ci: includeCi ? "github-actions" : "local",
    });
    addEvidenceFile("evidence/collection-status.json", collectionStatus);
    evidenceRefs[runRef] ??= { kind: "artifact-file", path: "evidence/collection-overhead.json" };
    const context = { commitSha, observedAt, runRef, attempt };
    const bundle = assembleReportBundle({
      scopes: config.scopes,
      measurementsByScope,
      context,
      evidenceFiles,
      evidenceRefs,
      config,
    });
    for (const snapshot of bundle.snapshots) {
      const validation = validateObjective(schema, snapshot);
      if (!validation.valid) throw new Error(`objective snapshot ${snapshot.snapshot_id} failed schema or semantic conformance: ${JSON.stringify(validation)}`);
    }
    assertNoAbsoluteArtifactPaths(bundle);
    mkdirSync(outputDir, { recursive: true, mode: 0o700 });
    if (readdirSync(outputDir).length > 0) throw new Error("report output directory must be empty");
    writeJson(join(outputDir, "manifest.json"), bundle.manifest);
    writeJson(join(outputDir, "evidence-index.json"), bundle.evidenceIndex);
    for (let index = 0; index < bundle.snapshots.length; index += 1) {
      writeJson(join(outputDir, bundle.snapshotFiles[index]), bundle.snapshots[index]);
    }
    for (const [path, value] of bundle.files) writeJson(join(outputDir, path), value);
    process.stdout.write(`${JSON.stringify({ report_dir: outputDir, snapshots: bundle.manifest.snapshots.length, elapsed_seconds: elapsedSeconds, collectors: Object.keys(collectionStatus.collectors).length })}\n`);
  } finally {
    if (process.env.CODE_HEALTH_KEEP_TEMP !== "1") rmSync(tempRoot, { recursive: true, force: true });
  }
}

function isDirectEntry() {
  return process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === pathToFileURL(fileURLToPath(import.meta.url)).href;
}

if (isDirectEntry()) {
  try { main(); }
  catch (error) {
    process.stderr.write(`code-health-produce: ${safeDiagnostic(error?.message ?? error)}\n`);
    process.exitCode = 1;
  }
}
