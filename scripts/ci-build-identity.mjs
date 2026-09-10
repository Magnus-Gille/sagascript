import { appendFileSync, lstatSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const ALLOWED_GENERATED_PATHS = Object.freeze([
  "src-tauri/gen/schemas/acl-manifests.json",
  "src-tauri/gen/schemas/capabilities.json",
  "src-tauri/gen/schemas/desktop-schema.json",
  "src-tauri/gen/schemas/windows-schema.json",
]);
const ALLOWED_GENERATED_SET = new Set(ALLOWED_GENERATED_PATHS);
const SHA_PATTERN = /^[0-9a-f]{40}$/;
const DATE_PATTERN = /^\d{4}-\d{2}-\d{2}$/;

function runGit(args) {
  const result = spawnSync("git", args, {
    cwd: process.cwd(),
    encoding: "buffer",
    stdio: ["ignore", "pipe", "ignore"],
  });
  if (result.error || result.status !== 0 || !result.stdout) {
    const detail = result.error && typeof result.error.code === "string"
      ? "error=" + result.error.code
      : "status=" + String(result.status);
    throw new Error("git command failed: " + args.join(" ") + " (" + detail + ")");
  }
  return result.stdout;
}

function headSha() {
  const value = runGit(["rev-parse", "HEAD"]).toString("utf8").trim();
  if (!SHA_PATTERN.test(value)) {
    throw new Error("git HEAD is not a full lowercase SHA-1");
  }
  return value;
}

function expectedSha() {
  const value = process.env.GITHUB_SHA;
  if (!SHA_PATTERN.test(value ?? "")) {
    throw new Error("GITHUB_SHA must be a full lowercase SHA-1");
  }
  return value;
}

function assertExpectedHead() {
  const expected = expectedSha();
  const head = headSha();
  if (expected !== head) {
    throw new Error("GITHUB_SHA does not match git HEAD");
  }
  return expected;
}

function statusEntries() {
  const bytes = runGit(["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
  const fields = [];
  let start = 0;
  for (let index = 0; index < bytes.length; index += 1) {
    if (bytes[index] === 0) {
      if (index > start) {
        fields.push(bytes.subarray(start, index));
      }
      start = index + 1;
    }
  }
  if (start !== bytes.length) {
    throw new Error("git status output was malformed");
  }

  const entries = [];
  for (let index = 0; index < fields.length; index += 1) {
    const record = fields[index].toString("utf8");
    if (record.length < 3 || record[2] !== " ") {
      throw new Error("git status output was malformed");
    }
    const xy = record.slice(0, 2);
    const path = record.slice(3);
    if (!path) {
      throw new Error("git status output contained an empty path");
    }
    let oldPath;
    if (xy.includes("R") || xy.includes("C")) {
      if (index + 1 >= fields.length) {
        throw new Error("git status rename output was malformed");
      }
      oldPath = fields[++index].toString("utf8");
      if (!oldPath) {
        throw new Error("git status rename output contained an empty path");
      }
    }
    entries.push({ xy, path, oldPath });
  }
  return entries;
}

function validDate(value) {
  if (!DATE_PATTERN.test(value ?? "")) {
    return false;
  }
  const parsed = new Date(value + "T00:00:00.000Z");
  return Number.isFinite(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value;
}

function outputPathIsRegular(relativePath) {
  try {
    return lstatSync(resolve(process.cwd(), relativePath)).isFile();
  } catch (error) {
    if (error && error.code === "ENOENT") {
      return false;
    }
    const code = error && typeof error.code === "string" ? error.code : "unknown";
    throw new Error("could not inspect generated output " + JSON.stringify(relativePath) + " (" + code + ")");
  }
}

function statusError(entry, reason) {
  return new Error(
    reason +
      " path=" +
      JSON.stringify(entry.path) +
      " xy=" +
      JSON.stringify(entry.xy),
  );
}

function verifyStatus(entries) {
  const observed = new Set();
  for (const entry of entries) {
    if (
      entry.xy.includes("R") ||
      entry.xy.includes("C") ||
      entry.xy.includes("D") ||
      [...entry.xy].some((code) => ![" ", "M", "A", "?"].includes(code))
    ) {
      throw statusError(entry, "working tree contains a deletion, rename, type change, conflict, or unsupported status");
    }
    if (!ALLOWED_GENERATED_SET.has(entry.path)) {
      throw statusError(entry, "working tree contains a modified or untracked source path");
    }
    if (entry.oldPath !== undefined || !outputPathIsRegular(entry.path)) {
      throw statusError(entry, "generated output is not a regular file");
    }
    observed.add(entry.path);
  }

  for (const relativePath of ALLOWED_GENERATED_PATHS) {
    try {
      if (!lstatSync(resolve(process.cwd(), relativePath)).isFile()) {
        throw new Error("generated output is not a regular file");
      }
    } catch (error) {
      if (error && error.code === "ENOENT") {
        continue;
      }
      if (error instanceof Error && error.message === "generated output is not a regular file") {
        throw error;
      }
      const code = error && typeof error.code === "string" ? error.code : "unknown";
      throw new Error("could not inspect generated output " + JSON.stringify(relativePath) + " (" + code + ")");
    }
  }
  return [...observed].sort();
}

function emit(mode, sha, buildDate, observed = []) {
  const payload = {
    mode,
    git_sha: sha,
    build_date: buildDate,
    allowed_generated_paths: ALLOWED_GENERATED_PATHS,
  };
  if (mode === "verify") {
    payload.observed_generated_paths = observed;
  }
  process.stdout.write(JSON.stringify(payload) + "\n");
}

function initialize() {
  const sha = assertExpectedHead();
  const entries = statusEntries();
  if (entries.length !== 0) {
    throw statusError(entries[0], "working tree must be clean before initialization");
  }
  const githubEnv = process.env.GITHUB_ENV;
  if (!githubEnv) {
    throw new Error("GITHUB_ENV is required");
  }
  const buildDate = new Date().toISOString().slice(0, 10);
  try {
    appendFileSync(
      githubEnv,
      "SAGASCRIPT_GIT_HASH=" + sha + "\nSAGASCRIPT_BUILD_DATE=" + buildDate + "\n",
      { encoding: "utf8" },
    );
  } catch (error) {
    const code = error && typeof error.code === "string" ? error.code : "unknown";
    throw new Error("could not append build identity to GITHUB_ENV (" + code + ")");
  }
  emit("initialize", sha, buildDate);
}

function verify() {
  const sha = assertExpectedHead();
  const exportedSha = process.env.SAGASCRIPT_GIT_HASH;
  if (!SHA_PATTERN.test(exportedSha ?? "") || exportedSha !== sha) {
    throw new Error("SAGASCRIPT_GIT_HASH does not match the validated source SHA");
  }
  const buildDate = process.env.SAGASCRIPT_BUILD_DATE;
  if (!validDate(buildDate)) {
    throw new Error("SAGASCRIPT_BUILD_DATE is not a valid UTC date");
  }
  emit("verify", sha, buildDate, verifyStatus(statusEntries()));
}

function main() {
  const args = process.argv.slice(2);
  if (args.length !== 1 || !["--initialize", "--verify"].includes(args[0])) {
    throw new Error("expected exactly one of --initialize or --verify");
  }
  if (args[0] === "--initialize") {
    initialize();
  } else {
    verify();
  }
}

try {
  main();
} catch (error) {
  const message = error instanceof Error ? error.message : "verification failed";
  console.error("ci-build-identity: " + message);
  process.exitCode = 1;
}
