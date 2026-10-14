// A structural reader for the workflow files under test (#358): the YAML is parsed, never matched
// with regular expressions, so a test asserts what a job `needs`, which step runs what and with
// which inputs, and keeps passing when the file is reformatted, re-commented or reordered
// within what the assertion states.
//
// The parser is the `yaml` package. It is not a dependency of this repository: it is vite's
// optional peer, which npm installs from package-lock.json. The ci test group lists it in
// GROUP_NPM_PACKAGES (scripts/node-test-groups.mjs), so a checkout where it is missing fails
// once with a clear message. If vite ever drops it, add `yaml` to devDependencies.
//
// Not for tests that run before `npm ci` (the Windows build identity gates in ci.yml's
// test-windows): they must read text with node:fs only.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

export const REPO_ROOT = fileURLToPath(new URL("../../", import.meta.url));
export const readRepoFile = (path) => readFileSync(`${REPO_ROOT}${path}`, "utf8").replace(/\r\n?/g, "\n");

// A scalar `run` is a script; CRLF checkouts (core.autocrlf on Windows) must not leak into it.
const script = (value) => (typeof value === "string" ? value.replace(/\r\n?/g, "\n").trim() : "");
const list = (value) => (value === undefined ? [] : Array.isArray(value) ? value : [value]);

function normalizeStep(step, index) {
  return {
    index,
    name: step.name,
    id: step.id,
    uses: step.uses,
    run: script(step.run),
    with: step.with ?? {},
    env: step.env ?? {},
    workingDirectory: step["working-directory"],
    shell: step.shell,
    condition: step.if === undefined ? undefined : String(step.if),
    continueOnError: step["continue-on-error"],
  };
}

function normalizeJob(id, job) {
  return {
    id,
    needs: list(job.needs),
    condition: job.if === undefined ? undefined : String(job.if),
    runsOn: job["runs-on"],
    timeoutMinutes: job["timeout-minutes"],
    env: job.env ?? {},
    outputs: job.outputs ?? {},
    continueOnError: job["continue-on-error"],
    steps: (job.steps ?? []).map(normalizeStep),
  };
}

/** Parses workflow text into `{ doc, jobs, job(id) }`. */
export function parseWorkflow(text) {
  const doc = parse(text);
  const jobs = Object.entries(doc.jobs ?? {}).map(([id, job]) => normalizeJob(id, job));
  return {
    doc,
    jobs,
    jobIds: jobs.map((job) => job.id),
    job(id) {
      const found = jobs.find((job) => job.id === id);
      if (!found) throw new Error(`workflow must define the job ${id}`);
      return found;
    },
  };
}

export const loadWorkflow = (file = "ci.yml") => parseWorkflow(readRepoFile(`.github/workflows/${file}`));

/** A local composite action: `{ inputs, steps }`. */
export function loadAction(name) {
  const doc = parse(readRepoFile(`.github/actions/${name}/action.yml`));
  return { doc, inputs: doc.inputs ?? {}, steps: (doc.runs?.steps ?? []).map(normalizeStep) };
}

/** The input a composite action step is called with, falling back to the action's default. */
export function actionInput(action, step, name) {
  return String(step.with[name] ?? action.inputs[name]?.default ?? "");
}

// Evaluates the only condition form the local actions use: `${{ inputs.<name> == '<value>' }}`.
function conditionHolds(condition, valueOf) {
  if (condition === undefined) return true;
  const match = /^\$\{\{\s*inputs\.([\w-]+) == '([^']*)'\s*\}\}$/.exec(condition.trim());
  if (!match) throw new Error(`unsupported condition in a local action: ${condition}`);
  return valueOf(match[1]) === match[2];
}

/**
 * The steps of a job with every `uses: ./.github/actions/<name>` step replaced by the `run` steps of
 * that action which its inputs enable (marked `viaAction`). Steps that `uses:` another action stay.
 */
export function expandedSteps(job) {
  return job.steps.flatMap((step) => {
    const local = /^\.\/\.github\/actions\/([\w-]+)$/.exec(step.uses ?? "");
    if (!local) return [step];
    const action = loadAction(local[1]);
    return action.steps
      .filter((inner) => inner.run !== "" && conditionHolds(inner.condition, (name) => actionInput(action, step, name)))
      .map((inner) => ({ ...inner, viaAction: local[1] }));
  });
}

/** The logical command lines of a step's script: continuations joined, blank and comment lines dropped. */
export function commandLines(step) {
  return step.run
    .replace(/\\\n\s*/g, " ")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"));
}

/** The `--os` and `--profile` a step passes to scripts/ci/cargo-gates.mjs, or null when it does not run it. */
export function gateInvocation(step) {
  const line = commandLines(step).find((candidate) => /^node scripts\/ci\/cargo-gates\.mjs(\s|$)/.test(candidate));
  if (line === undefined) return null;
  const words = line.split(/\s+/);
  const argument = (flag) => {
    const index = words.indexOf(flag);
    return index >= 0 ? words[index + 1] : undefined;
  };
  return { os: argument("--os"), profile: argument("--profile") ?? "main", line };
}

/** Every step of every job, as `{ job, step }`. */
export const allSteps = (workflow) => workflow.jobs.flatMap((job) => job.steps.map((step) => ({ job, step })));
