import { readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve } from "node:path";

function requiredNumber(value, label) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${label} must be a finite number`);
  }
  return value;
}

function optionalNumber(value, label) {
  if (value === undefined || value === null || value === "") return null;
  return requiredNumber(value, label);
}

function optionalString(value, label) {
  if (value === undefined || value === null || value === "") return null;
  if (typeof value !== "string") throw new TypeError(`${label} must be a string or null`);
  return value;
}

function timestamp(value, label) {
  if (value === undefined || value === null || value === "") return null;
  if (typeof value !== "string") throw new TypeError(`${label} must be an ISO timestamp or null`);
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) throw new TypeError(`${label} is not a valid timestamp: ${value}`);
  return milliseconds;
}

function interval(start, end, label) {
  const startMilliseconds = timestamp(start, `${label}.startedAt`);
  const endMilliseconds = timestamp(end, `${label}.completedAt`);
  if (startMilliseconds === null || endMilliseconds === null) return null;
  const seconds = (endMilliseconds - startMilliseconds) / 1000;
  if (seconds < 0) throw new RangeError(`${label} has a negative interval`);
  return seconds;
}

function summarizeStep(step, jobLabel) {
  if (step === null || typeof step !== "object" || Array.isArray(step)) {
    throw new TypeError(`${jobLabel}.steps entries must be objects`);
  }
  const number = requiredNumber(step.number, `${jobLabel}.steps[].number`);
  return {
    number,
    name: optionalString(step.name, `${jobLabel}.steps[${number}].name`),
    status: optionalString(step.status, `${jobLabel}.steps[${number}].status`),
    conclusion: optionalString(step.conclusion, `${jobLabel}.steps[${number}].conclusion`),
    durationSeconds: interval(step.startedAt, step.completedAt, `${jobLabel}.steps[${number}]`),
  };
}

function summarizeJob(job) {
  if (job === null || typeof job !== "object" || Array.isArray(job)) {
    throw new TypeError("jobs entries must be objects");
  }
  const id = requiredNumber(job.databaseId, "job.databaseId");
  if (!Array.isArray(job.steps)) throw new TypeError(`job ${id}.steps must be an array`);
  const label = `job ${id}`;
  const steps = job.steps.map((step) => summarizeStep(step, label));
  const stepNumbers = new Set();
  for (const step of steps) {
    if (stepNumbers.has(step.number)) throw new Error(`${label} has duplicate step number ${step.number}`);
    stepNumbers.add(step.number);
  }
  steps.sort((left, right) => left.number - right.number);
  return {
    id,
    name: optionalString(job.name, `${label}.name`),
    status: optionalString(job.status, `${label}.status`),
    conclusion: optionalString(job.conclusion, `${label}.conclusion`),
    durationSeconds: interval(job.startedAt, job.completedAt, label),
    steps,
    _startMilliseconds: timestamp(job.startedAt, `${label}.startedAt`),
    _endMilliseconds: timestamp(job.completedAt, `${label}.completedAt`),
  };
}

export function summarizeRun(run) {
  if (run === null || typeof run !== "object" || Array.isArray(run)) {
    throw new TypeError("run must be an object");
  }
  const id = optionalNumber(run.databaseId, "run.databaseId");
  if (!Array.isArray(run.jobs)) throw new TypeError("run.jobs must be an array");
  const jobs = run.jobs.map(summarizeJob);
  const jobIds = new Set();
  for (const job of jobs) {
    if (jobIds.has(job.id)) throw new Error(`duplicate job databaseId ${job.id}`);
    jobIds.add(job.id);
  }
  jobs.sort((left, right) => left.id - right.id);
  const completeJobs = jobs.length > 0 && jobs.every((job) => job.durationSeconds !== null);
  const wallSeconds = completeJobs
    ? (Math.max(...jobs.map((job) => job._endMilliseconds)) -
        Math.min(...jobs.map((job) => job._startMilliseconds))) /
      1000
    : null;
  const runnerSeconds = completeJobs
    ? jobs.reduce((total, job) => total + job.durationSeconds, 0)
    : null;
  return {
    id,
    headSha: optionalString(run.headSha, "run.headSha"),
    wallSeconds,
    runnerSeconds,
    jobs: jobs.map(({ _startMilliseconds, _endMilliseconds, ...job }) => job),
  };
}

function isDirectEntry() {
  if (!process.argv[1]) return false;
  const invoked = pathToFileURL(resolve(process.argv[1])).href;
  const module = pathToFileURL(fileURLToPath(import.meta.url)).href;
  return invoked === module;
}

function main() {
  try {
    const input = readFileSync(0, "utf8");
    if (input.trim() === "") throw new Error("stdin JSON is empty");
    let run;
    try {
      run = JSON.parse(input);
    } catch (error) {
      throw new Error(`stdin is not valid JSON: ${error.message}`);
    }
    process.stdout.write(`${JSON.stringify(summarizeRun(run), null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`ci-run-timings: ${error.message}\n`);
    process.exitCode = 1;
  }
}

if (isDirectEntry()) main();
