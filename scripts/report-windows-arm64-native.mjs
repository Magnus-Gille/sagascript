#!/usr/bin/env node
import { lstat, readFile, readdir } from "node:fs/promises";
import { join, resolve } from "node:path";
const profiles = [
  ["debug"],
  ["release"],
  ["aarch64-pc-windows-msvc", "debug"],
  ["aarch64-pc-windows-msvc", "release"],
];
async function isDirectory(path) {
  try {
    const info = await lstat(path);
    return info.isDirectory() && !info.isSymbolicLink();
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return false;
  }
}
async function isFile(path) {
  try {
    const info = await lstat(path);
    return info.isFile() && !info.isSymbolicLink();
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    return false;
  }
}
function cleanToken(token) {
  return token.replace(/^["'`]+|["'`,;]+$/g, "");
}
async function isSafeDirectory(root, ...parts) {
  let path = root;
  for (const part of parts) {
    path = join(path, part);
    if (!(await isDirectory(path))) return false;
  }
  return true;
}
async function findCaches(targetRoot) {
  const selected = [];
  for (const profile of profiles) {
    const profileRoot = join(targetRoot, ...profile, "build");
    if (!(await isSafeDirectory(targetRoot, ...profile, "build"))) continue;
    for (const crate of await readdir(profileRoot, { withFileTypes: true })) {
      if (!crate.isDirectory() || crate.isSymbolicLink() || !crate.name.startsWith("whisper-rs-sys-")) {
        continue;
      }
      const cmakeRoot = join(profileRoot, crate.name, "out", "build");
      if (!(await isSafeDirectory(targetRoot, ...profile, "build", crate.name, "out", "build"))) continue;
      const cachePath = join(cmakeRoot, "CMakeCache.txt");
      if (!(await isFile(cachePath))) continue;
      const cache = await readFile(cachePath, "utf8");
      if (!cache.split(/\r?\n/).some((line) => line === "CMAKE_PROJECT_NAME:STATIC=whisper.cpp")) {
        continue;
      }
      const ninjaPath = join(cmakeRoot, "build.ninja");
      if (!(await isFile(ninjaPath))) {
        throw new Error(`matching cache has no sibling build.ninja: ${cachePath}`);
      }
      selected.push({ cachePath, ninjaPath, ninja: await readFile(ninjaPath, "utf8") });
    }
  }
  return selected;
}
function inspectNinja(ninja) {
  const lines = ninja.split(/\r?\n/).filter((line) => /^\s*FLAGS\s*=/.test(line));
  const tokens = lines.flatMap((line) => line.trim().split(/\s+/).flatMap((token) => token.split(",")));
  const mcpu = [...new Set(tokens.map(cleanToken).filter((token) => /^-mcpu=/i.test(token)))].sort();
  if (mcpu.length === 0) throw new Error("no actual -mcpu= flag in Ninja FLAGS lines");
  for (const flag of mcpu) {
    const components = flag.slice("-mcpu=".length).toLowerCase().split("+");
    if (components[0] !== "native") {
      throw new Error(`required +native base missing from -mcpu= flag '${flag}'`);
    }
    for (const feature of ["sve", "sme"]) {
      if (components.some((component) => new RegExp(`^${feature}(?:[0-9-]|$)`).test(component))) {
        throw new Error(`positive ${feature}/${feature}2 token in Ninja FLAGS lines`);
      }
    }
    for (const component of ["nosve", "nosme"]) {
      if (!components.includes(component)) {
        throw new Error(`required +${component} component missing from -mcpu= flag '${flag}'`);
      }
    }
  }
  return mcpu;
}
async function main() {
  if (process.argv.length !== 3) throw new Error("usage: report-windows-arm64-native.mjs TARGET_ROOT");
  const targetRoot = resolve(process.argv[2]);
  if (!(await isDirectory(targetRoot))) throw new Error(`target root is not a real directory: ${targetRoot}`);
  const candidates = await findCaches(targetRoot);
  if (candidates.length === 0) throw new Error(`no whisper.cpp CMake caches under ${targetRoot}`);
  const allMcpu = new Set();
  for (const candidate of candidates) {
    for (const flag of inspectNinja(candidate.ninja)) allMcpu.add(flag);
    console.log(`Evidence pass: ${candidate.cachePath}`);
  }
  console.log(`Unique -mcpu= flags: ${[...allMcpu].sort().join(", ")}`);
}
main().catch((error) => {
  console.error(`ARM64 native evidence failed: ${error.message}`);
  process.exitCode = 1;
});
