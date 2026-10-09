import { createHash } from "node:crypto";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { canonical } from "./code-health-schema.mjs";

export const METRIC_NAMES = [
  "complex_functions",
  "unused_candidates",
  "coverage",
  "ci_first_attempt",
  "confirmed_regressions",
];

export const CI_CONCLUSION_PRIORITY = [
  "failure",
  "infra_failure",
  "unknown",
  "pending",
  "cancelled",
  "skipped",
  "success",
];

const METRIC_UNITS = {
  complex_functions: "functions",
  unused_candidates: "candidates",
  coverage: "lines",
  ci_first_attempt: "runs",
  confirmed_regressions: "regressions",
};

const SHA256 = /^[a-f0-9]{64}$/;
const FULL_SHA = /^[a-f0-9]{40}$/;
const REF = /^ref:[a-z0-9][a-z0-9-]{0,95}$/;

export function sha256(value) {
  const bytes = Buffer.isBuffer(value) ? value : Buffer.from(String(value));
  return createHash("sha256").update(bytes).digest("hex");
}

export function wholeSecondUtc(value) {
  const time = typeof value === "number" ? value : Date.parse(value);
  if (!Number.isFinite(time)) throw new TypeError("invalid timestamp");
  return new Date(Math.floor(time / 1000) * 1000).toISOString().replace(".000Z", "Z");
}

export function safeRepoPath(value, repoRoot) {
  if (typeof value !== "string" || value.length === 0) return null;
  const root = resolve(repoRoot);
  const absolute = isAbsolute(value) ? resolve(value) : resolve(root, value);
  const rel = relative(root, absolute);
  if (rel === "" || rel === ".." || rel.startsWith(".." + sep) || isAbsolute(rel)) return null;
  return rel.split(sep).join("/");
}

export function parseExportedCoverage(report, {
  repoRoot,
  inventoryPaths,
  sourceRoots,
  excludedPaths = [],
  measure = "lines",
  includesInlineTests = false,
} = {}) {
  if (!report || !Array.isArray(report.data) || !Array.isArray(inventoryPaths) || !Array.isArray(sourceRoots)) {
    throw new TypeError("coverage report or declared inventory is malformed");
  }
  if (!["lines", "functions", "branches"].includes(measure)) throw new TypeError("unsupported coverage measure");
  const inventory = new Set(inventoryPaths);
  const excluded = new Set(excludedPaths);
  const emitted = new Map();
  const outsideInventory = [];
  for (const datum of report.data) {
    if (!datum || !Array.isArray(datum.files)) throw new TypeError("coverage data.files is missing");
    for (const file of datum.files) {
      if (!file || typeof file.filename !== "string" || !file.summary || !file.summary[measure]) {
        throw new TypeError("coverage file summary is malformed");
      }
      const path = safeRepoPath(file.filename, repoRoot);
      if (path === null) {
        outsideInventory.push({ path: null, reason: "outside-repository" });
        continue;
      }
      if (excluded.has(path) || !sourceRoots.some(root => path.startsWith(root))) continue;
      if (!inventory.has(path)) {
        outsideInventory.push({ path, reason: "not-in-declared-inventory" });
        continue;
      }
      if (emitted.has(path)) throw new TypeError("coverage report has duplicate source file");
      const summary = file.summary[measure];
      const count = summary.count;
      const covered = summary.covered;
      if (!Number.isSafeInteger(count) || count < 0 || !Number.isSafeInteger(covered) || covered < 0 || covered > count) {
        throw new TypeError("coverage counts are invalid");
      }
      emitted.set(path, { eligible: count, covered });
    }
  }
  const paths = [...emitted.keys()].sort();
  if (paths.length === 0) throw new TypeError("coverage report emitted no files in the declared source scope");
  const eligible = paths.reduce((sum, path) => sum + emitted.get(path).eligible, 0);
  const covered = paths.reduce((sum, path) => sum + emitted.get(path).covered, 0);
  if (eligible < 1) throw new TypeError("coverage report has no eligible units");
  const notEmitted = [...inventory].filter(path => !emitted.has(path)).sort();
  return {
    measure,
    covered,
    eligible,
    emitted_source_files: paths.length,
    eligible_source_files: paths.length,
    includes_inline_tests: includesInlineTests,
    clean_profiles: true,
    source_inventory: "exported-only",
    included_paths: paths,
    not_emitted_paths: notEmitted,
    excluded_report_paths: outsideInventory,
    per_file: paths.map(path => ({ path, ...emitted.get(path) })),
  };
}

function nearestFunctionChildren(node) {
  const result = [];
  function visit(current) {
    for (const child of current.spaces ?? []) {
      if (!child || typeof child !== "object") continue;
      if (child.kind === "function") result.push(child);
      else visit(child);
    }
  }
  visit(node);
  return result;
}

export function parseRustComplexity(fileReports, threshold) {
  if (!Array.isArray(fileReports) || fileReports.length === 0) throw new TypeError("complexity file inventory is empty");
  if (!Number.isSafeInteger(threshold) || threshold < 1) throw new TypeError("complexity threshold is invalid");
  const seenPaths = new Set();
  const functions = [];
  for (const item of fileReports) {
    if (!item || typeof item.path !== "string" || !item.ast || !Array.isArray(item.ast.spaces)) {
      throw new TypeError("complexity analyzer output is malformed");
    }
    if (seenPaths.has(item.path)) throw new TypeError("complexity analyzer emitted a duplicate source file");
    seenPaths.add(item.path);
    let found = 0;
    function visit(node) {
      for (const child of node.spaces ?? []) {
        if (!child || typeof child !== "object") continue;
        if (child.kind === "function") {
          const aggregate = child.metrics?.cyclomatic?.sum;
          if (typeof aggregate !== "number" || !Number.isSafeInteger(aggregate) || aggregate < 1) {
            throw new TypeError("complexity function score is missing or invalid");
          }
          const nested = nearestFunctionChildren(child);
          const nestedTotal = nested.reduce((sum, nestedNode) => {
            const score = nestedNode.metrics?.cyclomatic?.sum;
            if (typeof score !== "number" || !Number.isSafeInteger(score) || score < 1) {
              throw new TypeError("nested complexity function score is missing or invalid");
            }
            return sum + score;
          }, 0);
          const cyclomatic = aggregate - nestedTotal;
          if (!Number.isSafeInteger(cyclomatic) || cyclomatic < 1) {
            throw new TypeError("nested complexity scores exceed their parent");
          }
          const startLine = child.start_line;
          const endLine = child.end_line;
          if (!Number.isSafeInteger(startLine) || startLine < 1 || !Number.isSafeInteger(endLine) || endLine < startLine) {
            throw new TypeError("complexity function span is invalid");
          }
          functions.push({
            path: item.path,
            name: typeof child.name === "string" ? child.name : "<anonymous>",
            kind: "function-or-closure",
            start_line: startLine,
            end_line: endLine,
            cyclomatic,
          });
          found += 1;
          visit(child);
        } else {
          visit(child);
        }
      }
    }
    visit(item.ast);
    if (found === 0) throw new TypeError("complexity analyzer emitted no functions for a selected source file");
  }
  functions.sort((a, b) => a.path.localeCompare(b.path) || a.start_line - b.start_line || a.name.localeCompare(b.name));
  return {
    algorithm: "cyclomatic-complexity-v1",
    threshold,
    eligible_functions: functions.length,
    above_threshold_functions: functions.filter(item => item.cyclomatic > threshold).length,
    functions,
  };
}

const KNIP_CODE_CATEGORIES = ["exports", "nsExports", "types", "nsTypes", "enumMembers", "classMembers", "duplicates"];

function nativeKnipCandidates(report) {
  if (!Array.isArray(report.files) || !Array.isArray(report.issues)) {
    throw new TypeError("Knip JSON report is missing file candidates or grouped issues");
  }
  const collected = report.files.map(file => {
    if (typeof file !== "string") throw new TypeError("Knip file candidate is malformed");
    return { category: "files", item: { file, line: 1 } };
  });
  for (const row of report.issues) {
    if (!row || typeof row !== "object" || Array.isArray(row) || typeof row.file !== "string") {
      throw new TypeError("Knip grouped issue row is malformed");
    }
    for (const category of KNIP_CODE_CATEGORIES) {
      const value = row[category];
      if (value === undefined) continue;
      if (category === "enumMembers" || category === "classMembers") {
        if (!value || typeof value !== "object" || Array.isArray(value)) {
          throw new TypeError(`Knip ${category} group is malformed`);
        }
        for (const [parent, members] of Object.entries(value)) {
          if (!Array.isArray(members)) throw new TypeError(`Knip ${category} member list is malformed`);
          for (const member of members) {
            if (!member || typeof member !== "object" || Array.isArray(member)) {
              throw new TypeError(`Knip ${category} member is malformed`);
            }
            const name = typeof member.name === "string" ? member.name : null;
            collected.push({ category, item: { ...member, file: row.file, name: name ? `${parent}.${name}` : parent } });
          }
        }
        continue;
      }
      if (!Array.isArray(value)) throw new TypeError(`Knip ${category} candidate list is malformed`);
      for (const entry of value) {
        const entries = category === "duplicates" && Array.isArray(entry) ? entry : [entry];
        for (const item of entries) {
          if (!item || typeof item !== "object" || Array.isArray(item)) {
            throw new TypeError(`Knip ${category} candidate is malformed`);
          }
          collected.push({ category, item: { ...item, file: row.file } });
        }
      }
    }
  }
  return collected;
}

function legacyKnipCandidates(report) {
  const collected = [];
  function visit(value, category = "candidate", file = null) {
    if (Array.isArray(value)) {
      for (const item of value) visit(item, category, file);
      return;
    }
    if (!value || typeof value !== "object") return;
    const itemPath = value.file ?? value.path ?? file;
    if (itemPath && typeof itemPath === "string") {
      const { file: _file, path: _path, ...details } = value;
      if (Object.keys(details).length) collected.push({ category, item: { ...details, file: itemPath } });
      for (const [key, child] of Object.entries(value)) {
        if (key !== "file" && key !== "path") visit(child, key, itemPath);
      }
      return;
    }
    for (const [key, child] of Object.entries(value)) visit(child, key, file);
  }
  visit(report.issues ?? report);
  return collected;
}

export function parseKnipCandidates(report, { repoRoot, language } = {}) {
  if (typeof report === "string") {
    try {
      report = JSON.parse(report);
    } catch {
      throw new TypeError("Knip output is not valid JSON");
    }
  }
  if (!report || typeof report !== "object" || Array.isArray(report)) throw new TypeError("Knip output is malformed");
  if (!["typescript", "svelte"].includes(language)) throw new TypeError("Knip candidate language is unsupported");
  const collected = Array.isArray(report.files) || Array.isArray(report.issues)
    ? nativeKnipCandidates(report)
    : legacyKnipCandidates(report);
  const extension = language === "svelte" ? ".svelte" : null;
  const candidates = [];
  for (const { category, item } of collected) {
    const sourcePath = safeRepoPath(item.file ?? item.path, repoRoot);
    if (!sourcePath || !sourcePath.startsWith("src/")) continue;
    const pathParts = sourcePath.toLowerCase().split("/");
    const fileName = pathParts.at(-1);
    if (pathParts.some(part => ["generated", "vendor", "__tests__", "tests", "build", "coverage"].includes(part))
      || /\.(test|spec)\.(ts|tsx)$/.test(fileName) || /^buildinfo\./.test(fileName)) continue;
    if (language === "svelte") {
      if (!sourcePath.endsWith(extension)) continue;
    } else if (!/\.(ts|tsx)$/.test(sourcePath)) {
      continue;
    }
    const line = Number.isSafeInteger(item.line) && item.line > 0 ? item.line : (category === "files" ? 1 : null);
    const candidateName = [item.symbol, item.name, item.dependency, item.export]
      .find(value => typeof value === "string" && value.length > 0) ?? null;
    candidates.push({ category, path: sourcePath, name: candidateName, line });
  }
  const deduplicated = new Map();
  for (const candidate of candidates) {
    const key = canonical(candidate);
    deduplicated.set(key, candidate);
  }
  return [...deduplicated.values()].sort((a, b) => a.path.localeCompare(b.path)
    || a.category.localeCompare(b.category) || (a.line ?? 0) - (b.line ?? 0) || (a.name ?? "").localeCompare(b.name ?? ""));
}

export function parseWorkflowJobIds(workflowText) {
  if (typeof workflowText !== "string") throw new TypeError("CI workflow source must be text");
  const lines = workflowText.split(/\r?\n/);
  const jobsLine = lines.findIndex(line => /^jobs:\s*$/.test(line));
  if (jobsLine < 0) throw new TypeError("CI workflow has no jobs section");
  const jobIds = [];
  for (const line of lines.slice(jobsLine + 1)) {
    if (!line.trim() || line.trimStart().startsWith("#")) continue;
    if (!/^\s/.test(line)) break;
    const match = /^  ([a-zA-Z0-9_-]+):\s*(?:#.*)?$/.exec(line);
    if (match) jobIds.push(match[1]);
  }
  if (!jobIds.length || new Set(jobIds).size !== jobIds.length) {
    throw new TypeError("CI workflow job inventory is empty or duplicated");
  }
  return jobIds;
}

export function assertCompletePageCount(actualCount, totalCount, maximumCount) {
  if (!Number.isSafeInteger(actualCount) || actualCount < 0
    || !Number.isSafeInteger(totalCount) || totalCount < 0
    || !Number.isSafeInteger(maximumCount) || maximumCount < 1) {
    throw new TypeError("GitHub Actions page counts are malformed");
  }
  if (totalCount > maximumCount) throw new RangeError("GitHub Actions API inventory exceeds the configured limit");
  if (actualCount !== totalCount) throw new Error("GitHub Actions API page count does not match total_count");
}

function conclusionForJob(job) {
  if (!job || typeof job !== "object") return "unknown";
  if (job.status && job.status !== "completed") {
    if (["queued", "in_progress", "waiting", "pending", "requested"].includes(job.status)) return "pending";
    return "unknown";
  }
  switch (job.conclusion) {
    case "success": return "success";
    case "failure": return "failure";
    case "startup_failure": return "failure";
    case "timed_out": return "failure";
    case "cancelled": return "cancelled";
    case "skipped": return "skipped";
    default: return "unknown";
  }
}

function runConclusion(jobs) {
  return CI_CONCLUSION_PRIORITY.find(conclusion => jobs.some(job => job.conclusion === conclusion)) ?? "unknown";
}

function inHalfOpenWindow(timestamp, start, end) {
  const instant = Date.parse(timestamp);
  return Number.isFinite(instant) && instant >= Date.parse(start) && instant < Date.parse(end);
}

export function normalizeCiCohort({
  runs,
  expectedJobIds,
  windowStart,
  windowEnd,
  workflowConfigDigest,
  repoRoot,
  workflowPath = ".github/workflows/ci.yml",
} = {}) {
  if (!Array.isArray(runs) || !Array.isArray(expectedJobIds) || expectedJobIds.length === 0) {
    throw new TypeError("CI run or job inventory is malformed");
  }
  if (runs.length > 1000) throw new RangeError("CI run inventory exceeds the v1 limit");
  if (!SHA256.test(workflowConfigDigest)) throw new TypeError("CI workflow digest is invalid");
  if (new Set(expectedJobIds).size !== expectedJobIds.length) throw new TypeError("expected CI jobs are duplicated");
  const ids = new Set();
  const normalizedRuns = [];
  const excludedRuns = [];
  const timingRuns = [];
  const incompleteRuns = [];
  for (const run of [...runs].sort((a, b) => Number(a.id) - Number(b.id))) {
    if (!run || !Number.isSafeInteger(run.id) || run.id < 1 || !FULL_SHA.test(run.head_sha)
      || !Number.isSafeInteger(run.run_attempt) || run.run_attempt < 1
      || typeof run.created_at !== "string" || !Number.isFinite(Date.parse(run.created_at))
      || typeof run.event !== "string" || typeof run.head_branch !== "string") {
      throw new TypeError("CI run inventory contains a malformed identity or timestamp");
    }
    if (ids.has(run.id)) throw new TypeError("CI run inventory contains a duplicate run ID");
    ids.add(run.id);
    if (run.event !== "push" || run.head_branch !== "main" || !inHalfOpenWindow(run.created_at, windowStart, windowEnd)) {
      excludedRuns.push({ id: run.id, reason: "outside-declared-cohort" });
      continue;
    }
    const commitWorkflowPath = run.workflow_path ?? workflowPath;
    if (commitWorkflowPath !== workflowPath) throw new TypeError("CI run references a different workflow file");
    if (run.workflow_config_digest !== workflowConfigDigest) throw new TypeError("CI workflow configuration changed inside the cohort");
    const attemptJobs = Array.isArray(run.attempt1_jobs) ? run.attempt1_jobs : [];
    if (attemptJobs.length > 50) throw new RangeError("CI job inventory exceeds the v1 limit");
    const actualByName = new Map();
    const jobIds = new Set();
    for (const job of attemptJobs) {
      if (!job || !Number.isSafeInteger(job.id) || job.id < 1 || typeof job.name !== "string" || job.name.length === 0) {
        throw new TypeError("CI attempt-1 job has a malformed identity");
      }
      if (jobIds.has(job.id)) throw new TypeError("CI attempt-1 contains a duplicate job ID");
      jobIds.add(job.id);
      if (actualByName.has(job.name)) throw new TypeError("CI attempt-1 contains duplicate job names");
      if (!expectedJobIds.includes(job.name)) throw new TypeError("CI attempt-1 contains an unregistered job");
      actualByName.set(job.name, job);
    }
    const jobs = expectedJobIds.map(jobId => ({
      job_ref: "ref:job-ci-" + jobId,
      conclusion: conclusionForJob(actualByName.get(jobId)),
    }));
    const normalized = {
      run_ref: "ref:ci-run-" + run.id + "-attempt-1",
      commit: run.head_sha,
      created_at: wholeSecondUtc(run.created_at),
      attempt: 1,
      latest_attempt: run.run_attempt,
      jobs,
      overall_conclusion: runConclusion(jobs),
    };
    const workflowStatus = typeof run.status === "string" ? run.status : null;
    const workflowConclusion = typeof run.conclusion === "string" ? run.conclusion : null;
    if (workflowStatus === "completed"
      && ["failure", "startup_failure", "timed_out"].includes(workflowConclusion)
      && normalized.overall_conclusion === "unknown") {
      incompleteRuns.push({ id: run.id, status: workflowStatus, conclusion: workflowConclusion });
    }
    normalizedRuns.push(normalized);
    timingRuns.push({
      databaseId: run.id,
      headSha: run.head_sha,
      workflowName: "CI",
      event: "push",
      attempt: 1,
      status: workflowStatus ?? (jobs.every(job => !["pending", "unknown"].includes(job.conclusion)) ? "completed" : "in_progress"),
      conclusion: workflowConclusion ?? normalized.overall_conclusion,
      createdAt: normalized.created_at,
      jobs: attemptJobs.map(job => ({
        databaseId: job.id,
        name: job.name,
        status: job.status,
        conclusion: job.conclusion,
        startedAt: job.started_at ?? null,
        completedAt: job.completed_at ?? null,
        steps: (job.steps ?? []).map(step => ({
          number: step.number,
          name: step.name,
          status: step.status,
          conclusion: step.conclusion,
          startedAt: step.started_at ?? null,
          completedAt: step.completed_at ?? null,
        })),
      })),
    });
  }
  return { runs: normalizedRuns, excluded_runs: excludedRuns, timing_runs: timingRuns, incomplete_runs: incompleteRuns };
}

export function buildCiFirstAttemptPayload({
  normalizedRuns,
  windowStart,
  windowEnd,
  workflowConfigDigest,
  expectedJobIds,
  runInventoryRef,
} = {}) {
  if (!Array.isArray(normalizedRuns) || !REF.test(runInventoryRef) || !SHA256.test(workflowConfigDigest)) {
    throw new TypeError("CI payload input is malformed");
  }
  return {
    branch: "main",
    event: "push",
    workflow_ref: "ref:workflow-ci-main-push",
    workflow_config_digest: { algorithm: "sha256", value: workflowConfigDigest },
    expected_job_refs: expectedJobIds.map(id => "ref:job-ci-" + id),
    window_start: wholeSecondUtc(windowStart),
    window_end: wholeSecondUtc(windowEnd),
    runs: normalizedRuns,
    expected_run_count: normalizedRuns.length,
    run_inventory_ref: runInventoryRef,
  };
}

function sourceFor(slot, context, observation) {
  const configDigest = sha256(canonical({
    tool: observation.tool,
    command: observation.command,
    metric_version: slot.metric_version,
  }));
  const scopeDigest = sha256(canonical({
    slot_ref: slot.slot_ref,
    language: slot.language,
    platform: slot.platform,
    feature_refs: slot.feature_refs,
    scope_version: slot.scope_version,
    included_refs: observation.included_refs,
    excluded_refs: observation.excluded_refs,
  }));
  return {
    run_ref: context.runRef,
    attempt: context.attempt,
    tool: observation.tool,
    language: slot.language,
    platform: slot.platform,
    feature_refs: [...slot.feature_refs],
    metric_version: slot.metric_version,
    scope_version: slot.scope_version,
    config_digest: { algorithm: "sha256", value: configDigest },
    scope_digest: { algorithm: "sha256", value: scopeDigest },
  };
}

function createMetric(name, slot, context, measurement) {
  const unit = METRIC_UNITS[name];
  if (!measurement) {
    const status = slot.status === "measured" ? "unknown" : slot.status;
    const reason = slot.status === "measured" ? "not-collected"
      : slot.reason ?? (status === "unsupported" ? "unsupported-tool" : "not-collected");
    return {
      observed_at: context.observedAt,
      source: null,
      population: null,
      unit,
      status,
      payload: null,
      reason,
      slot_ref: slot.slot_ref,
    };
  }
  if (measurement.status !== "measured") {
    return {
      observed_at: context.observedAt,
      source: null,
      population: null,
      unit,
      status: measurement.status,
      payload: null,
      reason: measurement.reason,
      slot_ref: slot.slot_ref,
    };
  }
  if (!measurement.payload || !measurement.tool || !Array.isArray(measurement.included_refs)
    || !Array.isArray(measurement.excluded_refs) || typeof measurement.command !== "string") {
    throw new TypeError("measured metric is missing source or population provenance");
  }
  const payload = structuredClone(measurement.payload);
  if (name === "complex_functions") {
    for (const key of Object.keys(payload)) {
      if (!["algorithm", "threshold", "eligible_functions", "above_threshold_functions"].includes(key)) delete payload[key];
    }
  } else if (name === "unused_candidates") {
    for (const key of Object.keys(payload)) {
      if (!["graph_status", "candidate_count", "validated_count", "false_positive_count", "uncertain_count", "owner_validation_refs", "graph_qualification_ref"].includes(key)) delete payload[key];
    }
  } else if (name === "coverage") {
    const coverageKeys = ["covered", "eligible", "emitted_source_files", "eligible_source_files", "includes_inline_tests", "clean_profiles", "source_inventory"];
    const coveragePayload = Object.fromEntries(coverageKeys.map(key => [key, payload[key]]));
    coveragePayload.profile_refs = [measurement.profile_ref];
    coveragePayload.measure = "lines";
    return {
      observed_at: context.observedAt,
      source: sourceFor(slot, context, measurement),
      population: {
        included_refs: [...measurement.included_refs],
        excluded_refs: [...measurement.excluded_refs],
      },
      unit,
      status: "measured",
      payload: coveragePayload,
      reason: null,
      slot_ref: slot.slot_ref,
    };
  }
  const source = sourceFor(slot, context, measurement);
  return {
    observed_at: context.observedAt,
    source,
    population: {
      included_refs: [...measurement.included_refs],
      excluded_refs: [...measurement.excluded_refs],
    },
    unit,
    status: "measured",
    payload,
    reason: null,
    slot_ref: slot.slot_ref,
  };
}

export function createObjective(scope, context, measurements = {}) {
  if (!scope || typeof scope.name !== "string" || !scope.slots) throw new TypeError("scope registry entry is malformed");
  if (!FULL_SHA.test(context.commitSha) || !REF.test(context.runRef) || !Number.isSafeInteger(context.attempt) || context.attempt < 1) {
    throw new TypeError("objective source identity is invalid");
  }
  const metrics = {};
  for (const name of METRIC_NAMES) {
    const slot = scope.slots[name];
    if (!slot || !REF.test(slot.slot_ref)) throw new TypeError("scope registry is missing a metric slot");
    metrics[name] = createMetric(name, slot, context, measurements[name]);
  }
  return {
    contract_version: "1.0",
    snapshot_id: "ref:sagascript-" + scope.name + "-" + context.commitSha.slice(0, 12),
    supersedes_ref: null,
    correction_ref: null,
    repository: { owner: "Magnus-Gille", name: "sagascript" },
    commit: context.commitSha,
    observed_at: context.observedAt,
    metrics,
  };
}

function safeArtifactPath(path) {
  return typeof path === "string" && path.length > 0 && !path.startsWith("/")
    && !path.includes("\\") && !path.split("/").some(part => part === "" || part === "." || part === "..");
}

function artifactRef(path) {
  if (!safeArtifactPath(path)) throw new TypeError("artifact evidence path must be relative and normalized");
  return { kind: "artifact-file", path };
}

function addEvidence(refs, key, value) {
  if (!REF.test(key)) throw new TypeError("evidence reference is invalid");
  if (refs[key] && canonical(refs[key]) !== canonical(value)) throw new TypeError("evidence reference collision");
  refs[key] = value;
}

export function assembleReportBundle({
  scopes,
  measurementsByScope,
  context,
  evidenceFiles = {},
  evidenceRefs = {},
  config,
} = {}) {
  if (!Array.isArray(scopes) || !config || !context) throw new TypeError("report bundle input is malformed");
  const snapshots = [];
  const snapshotFiles = [];
  for (const scope of scopes) {
    const snapshot = createObjective(scope, context, measurementsByScope[scope.name] ?? {});
    snapshots.push(snapshot);
    snapshotFiles.push("snapshots/" + scope.name + ".json");
  }
  const refs = {};
  for (const [key, value] of Object.entries(evidenceRefs)) addEvidence(refs, key, value);
  const registryPath = "scope-registry.json";
  for (const snapshot of snapshots) {
    for (const metric of Object.values(snapshot.metrics)) {
      addEvidence(refs, metric.slot_ref, artifactRef(registryPath));
      if (metric.source) addEvidence(refs, metric.source.run_ref, evidenceRefs[metric.source.run_ref]
        ?? artifactRef("evidence/collection-overhead.json"));
      for (const ref of metric.source?.feature_refs ?? []) addEvidence(refs, ref, artifactRef(registryPath));
      for (const ref of metric.population?.included_refs ?? []) {
        if (!refs[ref]) throw new TypeError("population evidence reference is unresolved: " + ref);
      }
      for (const ref of metric.population?.excluded_refs ?? []) {
        if (!refs[ref]) throw new TypeError("excluded population evidence reference is unresolved: " + ref);
      }
      if (metric.payload?.profile_refs) {
        for (const ref of metric.payload.profile_refs) {
          if (!refs[ref]) throw new TypeError("coverage profile evidence reference is unresolved: " + ref);
        }
      }
      if (metric.payload?.workflow_ref) addEvidence(refs, metric.payload.workflow_ref, artifactRef("evidence/ci-workflow.json"));
      if (metric.payload?.expected_job_refs) {
        for (const ref of metric.payload.expected_job_refs) addEvidence(refs, ref, artifactRef("evidence/ci-job-inventory.json"));
      }
      if (metric.payload?.run_inventory_ref) {
        if (!refs[metric.payload.run_inventory_ref]) throw new TypeError("CI run inventory reference is unresolved");
      }
      for (const run of metric.payload?.runs ?? []) {
        if (!refs[run.run_ref]) throw new TypeError("CI run evidence reference is unresolved");
        for (const job of run.jobs) {
          if (!refs[job.job_ref]) addEvidence(refs, job.job_ref, artifactRef("evidence/ci-job-inventory.json"));
        }
      }
    }
  }
  const files = new Map(Object.entries(evidenceFiles));
  files.set(registryPath, config);
  const overheadPath = "evidence/collection-overhead.json";
  if (!files.has(overheadPath)) files.set(overheadPath, { elapsed_seconds: null, measured: false });
  for (const path of files.keys()) if (!safeArtifactPath(path)) throw new TypeError("report file path is not safe");
  for (const [key, value] of Object.entries(refs)) {
    if (value.kind === "artifact-file" && !files.has(value.path)) throw new TypeError("artifact evidence points to a missing file: " + key);
    if (value.kind === "source" && (!safeArtifactPath(value.path) || !Number.isSafeInteger(value.line) || value.line < 1)) {
      throw new TypeError("source evidence is malformed");
    }
    if (value.kind === "github-run" && (!Number.isSafeInteger(value.run_id) || value.run_id < 1 || !Number.isSafeInteger(value.attempt) || value.attempt < 1)) {
      throw new TypeError("GitHub run evidence is malformed");
    }
    if (!["artifact-file", "source", "github-run"].includes(value.kind)) throw new TypeError("unsupported evidence kind");
  }
  const manifest = {
    contract_version: "1.0",
    repo_owner: "Magnus-Gille",
    repo_name: "sagascript",
    commit_sha: context.commitSha,
    snapshots: snapshotFiles,
    evidence_index: "evidence-index.json",
  };
  const evidenceIndex = { version: "1.0", refs };
  return { manifest, evidenceIndex, snapshots, snapshotFiles, files };
}

export function assertNoAbsoluteArtifactPaths(bundle) {
  const values = [
    bundle.manifest,
    bundle.evidenceIndex,
    ...bundle.snapshots,
    ...bundle.files.values(),
  ];
  const visit = (value, location = "$report") => {
    if (typeof value === "string") {
      if (/^(\/|[A-Za-z]:[\\/])/.test(value) || value.includes("/Users/") || value.includes("/private/tmp/")
        || value.includes("/var/folders/") || value.includes("/private/var/") || /^https?:\/\//i.test(value)) {
        throw new TypeError(`report contains an absolute local path at ${location}`);
      }
      return;
    }
    if (Array.isArray(value)) {
      value.forEach((item, index) => visit(item, `${location}[${index}]`));
    } else if (value && typeof value === "object") {
      for (const [key, item] of Object.entries(value)) visit(item, `${location}.${key}`);
    }
  };
  values.forEach(visit);
}

export function repoSourcePath(filePath, repoRoot) {
  const result = safeRepoPath(filePath, repoRoot);
  if (!result) throw new TypeError("source path is outside the repository");
  return result;
}
