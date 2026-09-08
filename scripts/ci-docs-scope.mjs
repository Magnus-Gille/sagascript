import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const SHA_RE = /^[0-9a-f]{40}$/;
const ALLOWED_PATHS = [
  /^README\.md$/,
  /^docs\/release-readiness\.md$/,
  /^docs\/research\/(?:[^/]+\/)*[^/]+\.md$/,
];

export function isAllowedPath(path) {
  return typeof path === "string"
    && !path.includes("\\")
    && !path.split("/").some((segment) => segment === "." || segment === "..")
    && ALLOWED_PATHS.some((pattern) => pattern.test(path));
}

export function parseNameStatusZ(output) {
  const bytes = Buffer.isBuffer(output) ? output : Buffer.from(output);
  if (bytes.length === 0) {
    throw new Error("git diff returned no records");
  }

  const fields = [];
  let start = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    if (bytes[index] !== 0) continue;
    fields.push(bytes.subarray(start, index).toString("utf8"));
    start = index + 1;
  }
  if (start !== bytes.length || fields.length === 0) {
    throw new Error("git diff returned malformed NUL-delimited output");
  }

  const records = [];
  for (let index = 0; index < fields.length;) {
    const status = fields[index];
    index += 1;
    if (!status) throw new Error("git diff returned an empty status");

    const pathCount = status.startsWith("R") || status.startsWith("C") ? 2 : 1;
    const paths = fields.slice(index, index + pathCount);
    if (paths.length !== pathCount || paths.some((path) => !path)) {
      throw new Error(`git diff returned an incomplete ${status} record`);
    }
    records.push({ status, paths });
    index += pathCount;
  }
  return records;
}

export function classifyRecords(records) {
  if (!Array.isArray(records) || records.length === 0) {
    throw new Error("cannot classify an empty diff");
  }

  const paths = records.flatMap((record) => {
    if (!record || !Array.isArray(record.paths) || record.paths.length === 0) {
      throw new Error("cannot classify a malformed diff record");
    }
    return record.paths;
  });
  const supportedStatuses = records.every(({ status }) => /^(?:A|M|D|R[0-9]{1,3})$/.test(status));
  const docsOnly = supportedStatuses && paths.every(isAllowedPath);
  return {
    schema_version: 1,
    docs_only: docsOnly,
    scope_trusted: true,
    scope_classification: docsOnly ? "docs-only" : "full-required",
    reason: docsOnly
      ? "all-changed-paths-are-allowlisted"
      : supportedStatuses ? "changed-path-outside-allowlist" : "unsupported-change-status",
    changed_paths: paths,
  };
}

function classifyFull() {
  return {
    schema_version: 1,
    docs_only: false,
    scope_trusted: true,
    scope_classification: "full-required",
    reason: "push-event-requires-full-ci",
    changed_paths: [],
  };
}

function gitDiff(base, head) {
  for (const [name, value] of [["base", base], ["head", head]]) {
    if (!SHA_RE.test(value)) {
      throw new Error(`${name} must be a 40-character lowercase commit SHA`);
    }
  }

  const range = `${base}...${head}`;
  const result = spawnSync(
    "git",
    ["diff", "--name-status", "--find-renames=50%", "-z", range, "--"],
    { encoding: "buffer", stdio: ["ignore", "pipe", "pipe"] },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const details = result.stderr?.toString("utf8").trim();
    throw new Error(`git diff failed${details ? `: ${details}` : ""}`);
  }
  return classifyRecords(parseNameStatusZ(result.stdout));
}

function writeGithubOutput(result) {
  const outputPath = process.env.GITHUB_OUTPUT;
  if (!outputPath) return;
  appendFileSync(
    outputPath,
    [
      `docs_only=${result.docs_only}`,
      `scope_trusted=${result.scope_trusted}`,
      `scope_classification=${result.scope_classification}`,
    ].join("\n") + "\n",
  );
}

function parseArguments(args) {
  if (args.length === 1 && args[0] === "--full") return classifyFull();
  if (args.length === 4 && args[0] === "--base" && args[2] === "--head") {
    return gitDiff(args[1], args[3]);
  }
  throw new Error("usage: ci-docs-scope.mjs --full | --base <sha> --head <sha>");
}

export function main(args = process.argv.slice(2)) {
  const result = parseArguments(args);
  process.stdout.write(`${JSON.stringify(result)}\n`);
  writeGithubOutput(result);
  return result;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    process.stderr.write(`ci-docs-scope: ${error.message}\n`);
    process.exitCode = 1;
  }
}
