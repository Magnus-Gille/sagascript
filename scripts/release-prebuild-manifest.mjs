#!/usr/bin/env node
// Manifest for prebuilt release artifacts (#266).
//   create: hash every file under --dir and write {version, sha, tree_sha, files}.
//   verify: fail unless the manifest matches the expected version/sha/tree and
//           the directory holds exactly the listed files with matching hashes.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const SHA40 = /^[0-9a-f]{40}$/;

export function listFiles(dir, exclude = []) {
  const root = resolve(dir);
  const skip = new Set(exclude.map((path) => resolve(path)));
  const found = [];
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const full = join(current, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.isFile() && !skip.has(full)) found.push(relative(root, full).split(sep).join("/"));
    }
  };
  walk(root);
  return found.sort();
}

// Manifest entries come from a downloaded artifact, so treat them as untrusted:
// only names that stay inside --dir are accepted. Names are relative,
// "/"-separated paths from listFiles; reject absolute paths, backslashes,
// empty/"."/".." segments.
export function unsafeEntryName(name) {
  if (typeof name !== "string" || name === "") return "empty or non-string name";
  if (name.startsWith("/") || /^[A-Za-z]:/.test(name) || isAbsolute(name)) return "absolute path";
  if (name.includes("\\") || name.includes("\0")) return "backslash or NUL in name";
  if (name.split("/").some((segment) => segment === "" || segment === "." || segment === "..")) {
    return "empty, '.' or '..' path segment";
  }
  return null;
}

function describe(dir, name) {
  const bytes = readFileSync(join(dir, name));
  return { name, sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length };
}

export function createManifest({ dir, version, sha, tree }) {
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error(`Not a stable version: ${version}`);
  if (!SHA40.test(sha)) throw new Error(`Expected a 40-character commit SHA, got: ${sha}`);
  if (!SHA40.test(tree)) throw new Error(`Expected a 40-character tree SHA, got: ${tree}`);
  const files = listFiles(dir).map((name) => describe(dir, name));
  if (!files.length) throw new Error(`No files found under ${dir}`);
  return { version, sha, tree_sha: tree, files };
}

export function verifyManifest({ dir, manifest, version, sha, tree, exclude = [] }) {
  const problems = [];
  for (const [field, expected, actual] of [
    ["version", version, manifest.version],
    ["sha", sha, manifest.sha],
    ["tree_sha", tree, manifest.tree_sha],
  ]) {
    if (actual !== expected) problems.push(`manifest ${field} is ${actual}, expected ${expected}`);
  }
  if (!Array.isArray(manifest.files) || !manifest.files.length) problems.push("manifest lists no files");
  const listed = new Set();
  for (const entry of manifest.files ?? []) {
    const unsafe = unsafeEntryName(entry.name);
    if (unsafe) {
      problems.push(`${JSON.stringify(entry.name)}: unsafe manifest entry name (${unsafe})`);
      continue;
    }
    listed.add(entry.name);
    let actual;
    try {
      actual = describe(dir, entry.name);
    } catch (error) {
      problems.push(`${entry.name}: ${error.code ?? error.message}`);
      continue;
    }
    if (actual.sha256 !== entry.sha256) problems.push(`${entry.name}: sha256 ${actual.sha256} != ${entry.sha256}`);
    if (actual.size !== entry.size) problems.push(`${entry.name}: size ${actual.size} != ${entry.size}`);
  }
  for (const name of listFiles(dir, exclude)) {
    if (!listed.has(name)) problems.push(`${name}: present but not in the manifest`);
  }
  return problems;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [command, ...rest] = process.argv.slice(2);
  const { values } = parseArgs({
    args: rest,
    options: {
      dir: { type: "string" },
      manifest: { type: "string" },
      out: { type: "string" },
      version: { type: "string" },
      sha: { type: "string" },
      tree: { type: "string" },
    },
  });
  for (const key of ["dir", "version", "sha", "tree"]) {
    if (!values[key]) throw new Error(`--${key} is required`);
  }
  if (command === "create") {
    if (!values.out) throw new Error("--out is required");
    const manifest = createManifest(values);
    writeFileSync(values.out, `${JSON.stringify(manifest, null, 2)}\n`);
    console.log(`Wrote ${values.out}: ${manifest.files.length} files for ${values.version} @ ${values.sha}`);
  } else if (command === "verify") {
    if (!values.manifest) throw new Error("--manifest is required");
    const manifest = JSON.parse(readFileSync(values.manifest, "utf8"));
    const problems = verifyManifest({
      dir: values.dir,
      manifest,
      version: values.version,
      sha: values.sha,
      tree: values.tree,
      exclude: [values.manifest],
    });
    if (problems.length) {
      console.error(`Prebuild verification FAILED:\n- ${problems.join("\n- ")}`);
      process.exit(1);
    }
    console.log(`Verified ${manifest.files.length} prebuilt files for ${values.version} @ ${values.sha} (tree ${values.tree})`);
  } else {
    throw new Error("Usage: release-prebuild-manifest.mjs create|verify --dir D ...");
  }
}
