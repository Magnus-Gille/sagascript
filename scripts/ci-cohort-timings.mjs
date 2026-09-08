import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { summarizeRun } from "./ci-run-timings.mjs";

function hasOwn(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function requiredString(value, label) {
  if (typeof value !== "string" || value.length === 0) throw new TypeError(`${label} must be a non-empty string`);
  return value;
}

function requiredId(value, label) {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${label} must be a non-negative safe integer`);
  }
  return value;
}

function optionalAttempt(value, label) {
  if (value === undefined || value === null || value === "") return null;
  return requiredId(value, label);
}

function parseTimestamp(value, label) {
  if (value === undefined || value === null || value === "") return null;
  if (typeof value !== "string") throw new TypeError(`${label} must be an ISO timestamp or null`);
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) throw new TypeError(`${label} is not a valid timestamp: ${value}`);
  return milliseconds;
}

function interval(start, end, label) {
  if (start === null || end === null) return null;
  const seconds = (end - start) / 1000;
  if (seconds < 0) throw new RangeError(`${label} has a negative interval`);
  return seconds;
}

function nearestRank(values, percentile) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const rank = Math.max(1, Math.ceil(percentile * sorted.length));
  return sorted[rank - 1];
}

function median(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

function stats(values) {
  if (values.length === 0) return { n: 0, p50: null, p90: null, min: null, max: null };
  return {
    n: values.length,
    p50: median(values),
    p90: nearestRank(values, 0.9),
    min: Math.min(...values),
    max: Math.max(...values),
  };
}

function aggregate(valuesByName) {
  return [...valuesByName.entries()]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([name, values]) => ({ name, ...stats(values) }));
}

function validateRun(run, index) {
  const label = `runs[${index}]`;
  if (run === null || typeof run !== "object" || Array.isArray(run)) throw new TypeError(`${label} must be an object`);
  for (const key of ["databaseId", "headSha", "workflowName", "event", "status", "conclusion", "jobs"]) {
    if (!hasOwn(run, key)) throw new TypeError(`${label}.${key} is required`);
  }
  const id = requiredId(run.databaseId, `${label}.databaseId`);
  const workflowName = requiredString(run.workflowName, `${label}.workflowName`);
  const event = requiredString(run.event, `${label}.event`);
  const status = requiredString(run.status, `${label}.status`);
  if (run.conclusion !== null && typeof run.conclusion !== "string") {
    throw new TypeError(`${label}.conclusion must be a string or null`);
  }
  if (status === "completed" && (run.conclusion === null || run.conclusion.length === 0)) {
    throw new TypeError(`${label}.conclusion is required for completed runs`);
  }
  const attempt = optionalAttempt(run.attempt, `${label}.attempt`);
  const createdMilliseconds = parseTimestamp(run.createdAt, `${label}.createdAt`);
  let normalizedRun;
  try {
    normalizedRun = summarizeRun(run);
  } catch (error) {
    throw new Error(`${label}: ${error.message}`, { cause: error });
  }
  const jobs = normalizedRun.jobs;
  const jobStarts = jobs.map((job) => parseTimestamp(run.jobs.find((candidate) => candidate.databaseId === job.id)?.startedAt, `${label}.jobs[${job.id}].startedAt`));
  const jobEnds = jobs.map((job) => parseTimestamp(run.jobs.find((candidate) => candidate.databaseId === job.id)?.completedAt, `${label}.jobs[${job.id}].completedAt`));
  const completeTiming = createdMilliseconds !== null && jobs.length > 0 && jobs.every((job, position) => (
    job.durationSeconds !== null && jobStarts[position] !== null && jobEnds[position] !== null
  ));
  const initialWaitSeconds = completeTiming ? interval(createdMilliseconds, Math.min(...jobStarts), `${label}.createdAt→first job`) : null;
  const elapsedSeconds = completeTiming ? interval(createdMilliseconds, Math.max(...jobEnds), `${label}.createdAt→last job`) : null;
  const eligible = status === "completed" && run.conclusion === "success" && completeTiming;
  return {
    id,
    attempt,
    workflowName,
    event,
    headSha: requiredString(run.headSha, `${label}.headSha`),
    status,
    conclusion: run.conclusion,
    createdAt: run.createdAt === undefined || run.createdAt === null ? null : requiredString(run.createdAt, `${label}.createdAt`),
    eligible,
    incomplete: status === "completed" && run.conclusion === "success" && !completeTiming,
    reason: eligible ? null : (status === "completed" && run.conclusion === "success" ? "incomplete-timing" : "unsuccessful"),
    normalizedRun,
    initialWaitSeconds,
    elapsedSeconds,
  };
}

function addOutcome(counts, run) {
  counts.total += 1;
  if (run.status !== "completed") counts.inProgress += 1;
  if (run.conclusion === "success") counts.success += 1;
  else if (run.conclusion === null) counts.noConclusion += 1;
  const outcome = run.conclusion ?? "none";
  counts.outcomes[outcome] = (Object.prototype.hasOwnProperty.call(counts.outcomes, outcome) ? counts.outcomes[outcome] : 0) + 1;
  if (run.incomplete) counts.incomplete += 1;
  if (run.eligible) counts.eligible += 1;
}

export function summarizeCohorts(runs) {
  if (!Array.isArray(runs)) throw new TypeError("stdin JSON must be an array of workflow runs");
  if (runs.length === 0) throw new Error("stdin JSON array is empty");
  const cohorts = new Map();
  const seen = new Set();
  for (let index = 0; index < runs.length; index += 1) {
    const run = validateRun(runs[index], index);
    const identity = `${run.id}/${run.attempt === null ? "null" : run.attempt}`;
    if (seen.has(identity)) throw new Error(`duplicate run databaseId/attempt ${identity}`);
    seen.add(identity);
    const key = `${run.workflowName}\u0000${run.event}\u0000${run.attempt === null ? "null" : run.attempt}`;
    if (!cohorts.has(key)) {
      cohorts.set(key, {
        workflowName: run.workflowName,
        event: run.event,
        attempt: run.attempt,
        ledger: [],
        counts: { total: 0, eligible: 0, incomplete: 0, success: 0, inProgress: 0, noConclusion: 0, outcomes: Object.create(null) },
        metricValues: { elapsedSeconds: [], initialWaitSeconds: [], wallSeconds: [], runnerSeconds: [] },
        jobValues: new Map(),
        stepValues: new Map(),
      });
    }
    const cohort = cohorts.get(key);
    addOutcome(cohort.counts, run);
    cohort.ledger.push({
      id: run.id,
      attempt: run.attempt,
      headSha: run.headSha,
      status: run.status,
      conclusion: run.conclusion,
      createdAt: run.createdAt,
      eligible: run.eligible,
      incomplete: run.incomplete,
      reason: run.reason,
      elapsedSeconds: run.elapsedSeconds,
      initialWaitSeconds: run.initialWaitSeconds,
      wallSeconds: run.normalizedRun.wallSeconds,
      runnerSeconds: run.normalizedRun.runnerSeconds,
    });
    if (!run.eligible) continue;
    cohort.metricValues.elapsedSeconds.push(run.elapsedSeconds);
    cohort.metricValues.initialWaitSeconds.push(run.initialWaitSeconds);
    cohort.metricValues.wallSeconds.push(run.normalizedRun.wallSeconds);
    cohort.metricValues.runnerSeconds.push(run.normalizedRun.runnerSeconds);
    for (const job of run.normalizedRun.jobs) {
      if (job.conclusion !== "success" || job.durationSeconds === null) continue;
      const jobName = job.name ?? String(job.id);
      if (!cohort.jobValues.has(jobName)) cohort.jobValues.set(jobName, []);
      cohort.jobValues.get(jobName).push(job.durationSeconds);
      for (const step of job.steps) {
        if (step.conclusion !== "success" || step.durationSeconds === null) continue;
        const stepName = step.name ?? `step-${step.number}`;
        const stepKey = `${jobName}\u0000${step.number}\u0000${stepName}`;
        if (!cohort.stepValues.has(stepKey)) cohort.stepValues.set(stepKey, { jobName, name: stepName, number: step.number, values: [] });
        cohort.stepValues.get(stepKey).values.push(step.durationSeconds);
      }
    }
  }
  return {
    cohorts: [...cohorts.values()].map((cohort) => {
      const metrics = Object.fromEntries(Object.entries(cohort.metricValues).map(([name, values]) => [name, stats(values)]));
      cohort.ledger.sort((left, right) => left.id - right.id || (left.attempt ?? -1) - (right.attempt ?? -1));
      return {
        workflowName: cohort.workflowName,
        event: cohort.event,
        attempt: cohort.attempt,
        ledger: cohort.ledger,
        counts: { ...cohort.counts, outcomes: { ...cohort.counts.outcomes } },
        elapsedSeconds: metrics.elapsedSeconds,
        initialWaitSeconds: metrics.initialWaitSeconds,
        wallSeconds: metrics.wallSeconds,
        runnerSeconds: metrics.runnerSeconds,
        jobs: aggregate(cohort.jobValues),
        steps: [...cohort.stepValues.values()]
          .sort((left, right) => left.jobName.localeCompare(right.jobName) || left.number - right.number || left.name.localeCompare(right.name))
          .map(({ jobName, name, number, values }) => ({ jobName, name, number, ...stats(values) })),
      };
    }).sort((left, right) => left.workflowName.localeCompare(right.workflowName) || left.event.localeCompare(right.event) || (left.attempt ?? -1) - (right.attempt ?? -1)),
  };
}

function isDirectEntry() {
  if (!process.argv[1]) return false;
  const invoked = pathToFileURL(resolve(process.argv[1])).href;
  const module = pathToFileURL(fileURLToPath(import.meta.url)).href;
  return invoked === module;
}

function printHelp() {
  process.stdout.write(`Usage: node scripts/ci-cohort-timings.mjs < runs.json\n\nRead a JSON array of GitHub workflow runs from stdin and group by workflowName, event, and attempt.\nBuild the array from gh run view RUN_ID --json databaseId,headSha,workflowName,event,attempt,status,conclusion,createdAt,jobs\n(or concatenate those objects from multiple gh run view calls). Required run fields: databaseId, headSha, workflowName, event,\nstatus, conclusion, jobs. attempt and timestamps may be null/missing; conclusion may be null while a run is in progress.\nSuccessful runs with incomplete timestamps stay in the ledger and counts but are excluded from latency aggregates.\np50 is the median; p90 uses nearest rank.\n`);
}

function main() {
  if (process.argv.includes("--help") || process.argv.includes("-h")) {
    printHelp();
    return;
  }
  try {
    const input = readFileSync(0, "utf8");
    if (input.trim() === "") throw new Error("stdin JSON is empty");
    let runs;
    try {
      runs = JSON.parse(input);
    } catch (error) {
      throw new Error(`stdin is not valid JSON: ${error.message}`);
    }
    process.stdout.write(`${JSON.stringify(summarizeCohorts(runs), null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`ci-cohort-timings: ${error.message}\n`);
    process.exitCode = 1;
  }
}

if (isDirectEntry()) main();
