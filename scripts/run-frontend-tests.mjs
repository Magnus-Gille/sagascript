// Runs the Node test files (`node --test`) by group (#350).
//
//   node scripts/run-frontend-tests.mjs                  every group (npm run test:frontend)
//   node scripts/run-frontend-tests.mjs --group ui       one or more groups, comma-separated
//   node scripts/run-frontend-tests.mjs --list           the classification
//
// scripts/node-test-groups.mjs classifies every test file; an unclassified
// file under scripts/ (or a stale entry) is an error before anything runs.
// npm packages a group needs are checked first (one error, nothing is
// installed). The run has a time budget; on timeout or interruption the whole
// `node --test` process tree is stopped.
import { existsSync, readdirSync, realpathSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  formatBudget,
  runProcessTree,
  signalExitCode,
  trapTerminationSignals,
} from "./lib/process-tree.mjs";
import { findPackageDirectory } from "./lib/node-packages.mjs";

const DEFAULT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
export const MANIFEST = "scripts/node-test-groups.mjs";
const DEFAULT_TIMEOUT_MS = 20 * 60_000;
const TEST_FILE = /^test[-_].*\.mjs$/;
const SKIPPED_DIRECTORIES = new Set(["node_modules", "fixtures"]);

const USAGE = `Usage: node scripts/run-frontend-tests.mjs [--group <name>[,<name>]] [--list] [--timeout-ms <ms>]

Groups are defined in ${MANIFEST}. Without --group every group runs.
  --group       ui, ci, release, code-health (repeatable or comma-separated)
  --list        print the groups and their files; run nothing
  --timeout-ms  budget for the whole run (default ${DEFAULT_TIMEOUT_MS})
  --root        repository root to use (for the runner's own test fixtures)`;

class UsageError extends Error {}

/** `test-*.mjs` / `test_*.mjs` files anywhere under scripts/ (not fixtures/), as sorted repo-relative paths. */
export function discoverTestFiles(root) {
  const found = [];
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.isDirectory()) {
        if (!SKIPPED_DIRECTORIES.has(entry.name) && !entry.name.startsWith(".")) {
          walk(join(directory, entry.name));
        }
      } else if (entry.isFile() && TEST_FILE.test(entry.name)) {
        found.push(relative(root, join(directory, entry.name)).split(sep).join("/"));
      }
    }
  };
  walk(join(root, "scripts"));
  return found.sort();
}

/** Problems between the manifest and the files on disk; empty when they agree. */
export function classificationProblems(groups, discovered, root) {
  const owner = new Map();
  const problems = [];
  for (const [group, files] of Object.entries(groups)) {
    for (const file of files) {
      if (owner.has(file)) {
        problems.push(`${file} is listed in both "${owner.get(file)}" and "${group}"; keep one`);
      } else {
        owner.set(file, group);
      }
      if (!existsSync(join(root, file))) {
        problems.push(`${file} (group "${group}") does not exist; remove it from ${MANIFEST}`);
      }
    }
  }
  const names = Object.keys(groups).join(", ");
  for (const file of discovered) {
    if (!owner.has(file)) {
      problems.push(`${file} is not classified; add it to one group in ${MANIFEST} (${names})`);
    }
  }
  return problems;
}

/** "code-health" on the command line names the codeHealth group. */
export function groupKey(name) {
  return name.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
}

function displayName(key) {
  return key.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

// The tests import packages from scripts/, so resolve from there.
function isPackageResolvable(root, name) {
  return findPackageDirectory(join(root, "scripts"), name) !== null;
}

export function parseArgs(argv) {
  const options = { groups: [], list: false, timeoutMs: DEFAULT_TIMEOUT_MS, root: DEFAULT_ROOT, help: false };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    const equals = arg.startsWith("--") ? arg.indexOf("=") : -1;
    const flag = equals === -1 ? arg : arg.slice(0, equals);
    const value = () => {
      const next = equals === -1 ? argv[++index] : arg.slice(equals + 1);
      if (next === undefined || next === "") throw new UsageError(`${flag} needs a value`);
      return next;
    };
    switch (flag) {
      case "--group":
        options.groups.push(...value().split(",").map((name) => name.trim()).filter(Boolean));
        break;
      case "--list":
        options.list = true;
        break;
      case "--timeout-ms": {
        const raw = value();
        const timeoutMs = Number(raw);
        if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
          throw new UsageError(`--timeout-ms must be a positive number, got ${raw}`);
        }
        options.timeoutMs = timeoutMs;
        break;
      }
      case "--root":
        options.root = resolve(value());
        break;
      case "--help":
      case "-h":
        options.help = true;
        break;
      default:
        throw new UsageError(`unknown argument ${arg}`);
    }
  }
  return options;
}

export async function main(argv) {
  let options;
  try {
    options = parseArgs(argv);
  } catch (error) {
    if (!(error instanceof UsageError)) throw error;
    console.error(`run-frontend-tests: ${error.message}\n\n${USAGE}`);
    return 2;
  }
  if (options.help) {
    console.log(USAGE);
    return 0;
  }

  const { root } = options;
  const manifestPath = join(root, MANIFEST);
  if (!existsSync(manifestPath)) {
    console.error(`run-frontend-tests: ${MANIFEST} not found under ${root}`);
    return 2;
  }
  const manifest = await import(pathToFileURL(manifestPath).href);
  const groups = manifest.TEST_GROUPS;
  const npmPackages = manifest.GROUP_NPM_PACKAGES ?? {};

  const problems = classificationProblems(groups, discoverTestFiles(root), root);
  if (problems.length > 0) {
    console.error(
      `run-frontend-tests: ${MANIFEST} does not match the test files on disk:\n` +
        problems.map((problem) => `  - ${problem}`).join("\n"),
    );
    return 2;
  }

  if (options.list) {
    for (const [group, files] of Object.entries(groups)) {
      const description = manifest.GROUP_DESCRIPTIONS?.[group];
      console.log(`${displayName(group)} (${files.length} files)${description ? `: ${description}` : ""}`);
      for (const file of files) console.log(`  ${file}`);
    }
    return 0;
  }

  const selected = options.groups.length > 0 ? [...new Set(options.groups.map(groupKey))] : Object.keys(groups);
  const unknown = selected.filter((group) => !Object.hasOwn(groups, group));
  if (unknown.length > 0) {
    const known = Object.keys(groups).map(displayName).join(", ");
    console.error(`run-frontend-tests: unknown group ${unknown.map(displayName).join(", ")}; known groups: ${known}`);
    return 2;
  }

  const missing = [...new Set(selected.flatMap((group) => npmPackages[group] ?? []))].filter(
    (name) => !isPackageResolvable(root, name),
  );
  if (missing.length > 0) {
    const neededBy = selected.filter((group) => (npmPackages[group] ?? []).some((name) => missing.includes(name)));
    console.error(
      `run-frontend-tests: missing npm package${missing.length === 1 ? "" : "s"} ${missing.join(", ")} ` +
        `(needed by group ${neededBy.map(displayName).join(", ")}).\n` +
        `Run \`npm ci\` in ${root}, then run the tests again. Nothing was installed or run.`,
    );
    return 2;
  }

  const files = selected.flatMap((group) => groups[group]);
  console.log(
    `Node tests: ${selected.map((group) => `${displayName(group)} (${groups[group].length})`).join(", ")}; ` +
      `budget ${formatBudget(options.timeoutMs)}`,
  );

  // A run started from inside another node:test process (e.g. this runner's
  // own tests) would otherwise inherit its test context and run no files.
  const env = { ...process.env };
  delete env.NODE_TEST_CONTEXT;

  const controller = new AbortController();
  const untrap = trapTerminationSignals((signal) => controller.abort(signal));
  const result = await runProcessTree({
    command: process.execPath,
    args: ["--test", ...files.map((file) => join(root, file))],
    cwd: root,
    env,
    timeoutMs: options.timeoutMs,
    killGraceMs: 2000,
    abortSignal: controller.signal,
  });
  untrap();

  if (result.leftovers) {
    console.error("run-frontend-tests: stopped processes the tests left running");
  }
  if (result.abortedBy) {
    console.error(`run-frontend-tests: interrupted by ${result.abortedBy}; stopped the test process tree`);
    return signalExitCode(result.abortedBy);
  }
  if (result.timedOut) {
    console.error(
      `run-frontend-tests: timed out after ${formatBudget(options.timeoutMs)}; stopped the test process tree ` +
        "(the output above shows which file was still running)",
    );
    return 1;
  }
  if (result.error) {
    console.error(`run-frontend-tests: could not start node --test: ${result.error.message}`);
    return 1;
  }
  if (result.code === null) {
    console.error(`run-frontend-tests: node --test terminated by ${result.signal ?? "an unknown signal"}`);
    return 1;
  }
  return result.code;
}

// Node reports the real path in import.meta.url, so compare against the real
// path of argv[1]: a symlinked invocation must still run.
function invokedDirectly() {
  if (!process.argv[1]) return false;
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href;
  } catch {
    return import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
  }
}

if (invokedDirectly()) {
  // No top-level await: a config or manifest module may import this module.
  main(process.argv.slice(2)).then((code) => {
    process.exitCode = code;
  });
}
