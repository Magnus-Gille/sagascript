import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scriptPath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "report-windows-arm64-native.mjs",
);
const profiles = [
  ["debug"],
  ["release"],
  ["aarch64-pc-windows-msvc", "debug"],
  ["aarch64-pc-windows-msvc", "release"],
];

async function writeBuild(targetRoot, profile, suffix, flags, project = "whisper.cpp") {
  const buildRoot = join(
    targetRoot,
    ...profile,
    "build",
    `whisper-rs-sys-${suffix}`,
    "out",
    "build",
  );
  await mkdir(buildRoot, { recursive: true });
  await writeFile(
    join(buildRoot, "CMakeCache.txt"),
    `CMAKE_PROJECT_NAME:STATIC=${project}\nCMAKE_GENERATOR:INTERNAL=Ninja\n`,
  );
  await writeFile(
    join(buildRoot, "build.ninja"),
    `rule CXX_COMPILER\n  command = clang++ $FLAGS\nbuild object.o: CXX_COMPILER source.cc\n  FLAGS = ${flags}\n`,
  );
}

async function runReporter(setup) {
  const directory = await mkdtemp(join(tmpdir(), "sagascript-arm64-evidence-"));
  const targetRoot = join(directory, "target");
  await mkdir(targetRoot);
  await setup(targetRoot);
  try {
    const result = spawnSync(process.execPath, [scriptPath, targetRoot], {
      cwd: directory,
      encoding: "utf8",
      shell: false,
    });
    return {
      ...result,
      output: `${result.stdout ?? ""}${result.stderr ?? ""}`,
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("reports unique ARM64 compiler evidence from the four allowed profiles", async () => {
  const result = await runReporter(async (targetRoot) => {
    for (const [index, profile] of profiles.entries()) {
      await writeBuild(
        targetRoot,
        profile,
        `fixture-${index}`,
        "-mcpu=native+dotprod+i8mm+nosve+nosme",
      );
    }
    await writeBuild(targetRoot, ["debug"], "foreign", "-mcpu=host+dotprod+nosve+nosme", "other-project");
  });
  assert.equal(result.status, 0, result.output);
  assert.equal((result.stdout.match(/Evidence pass:/g) ?? []).length, 4);
  assert.match(result.stdout, /Unique -mcpu= flags: -mcpu=native\+dotprod\+i8mm\+nosve\+nosme\n/);
  assert.equal((result.stdout.match(/-mcpu=native/g) ?? []).length, 1);
});

test("rejects positive SVE and SME flags, including versioned forms", async () => {
  for (const positive of [
    "-mcpu=native+dotprod+i8mm+sve+nosme",
    "-mcpu=native+dotprod+i8mm+sve2+nosme",
    "-mcpu=native+dotprod+i8mm+nosve+sme",
    "-mcpu=native+dotprod+i8mm+nosve+sme2",
  ]) {
    const result = await runReporter((targetRoot) =>
      writeBuild(targetRoot, ["debug"], "positive", positive),
    );
    assert.notEqual(result.status, 0, result.output);
    assert.match(result.output, /positive (?:sve|sme)/i);
  }
});

test("rejects evidence missing native or explicit no-SVE/no-SME flags", async () => {
  for (const flags of [
    "-mcpu=generic+dotprod+i8mm+nosve+nosme",
    "-mcpu=native+dotprod+i8mm+nosme",
    "-mcpu=native+dotprod+i8mm+nosve",
    "-mcpu=native+dotprod+i8mm+nosve -mcpu=native+dotprod+i8mm+nosme",
  ]) {
    const result = await runReporter((targetRoot) =>
      writeBuild(targetRoot, ["release"], "missing", flags),
    );
    assert.notEqual(result.status, 0, result.output);
    assert.match(result.output, /(?:no actual -mcpu|required \+(?:native|nosve|nosme))/i);
  }
});

test("fails when no allowed Rust target profile contains whisper.cpp evidence", async () => {
  const result = await runReporter((targetRoot) =>
    writeBuild(targetRoot, ["other-target", "debug"], "out-of-scope", "-mcpu=native+dotprod+i8mm+nosve+nosme"),
  );
  assert.notEqual(result.status, 0, result.output);
  assert.match(result.output, /no whisper\.cpp CMake caches/i);
});

test("fails when a matching cache has no sibling build.ninja", async () => {
  const result = await runReporter(async (targetRoot) => {
    const buildRoot = join(targetRoot, "debug", "build", "whisper-rs-sys-missing", "out", "build");
    await mkdir(buildRoot, { recursive: true });
    await writeFile(join(buildRoot, "CMakeCache.txt"), "CMAKE_PROJECT_NAME:STATIC=whisper.cpp\n");
  });
  assert.notEqual(result.status, 0, result.output);
  assert.match(result.output, /no sibling build\.ninja/i);
});
