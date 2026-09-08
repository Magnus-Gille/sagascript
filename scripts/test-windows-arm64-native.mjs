import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const hookPath = resolve(scriptDir, "cmake/windows-arm64-native.cmake");

const machineSupportFlags = [
  "GGML_MACHINE_SUPPORTS_sve",
  "GGML_MACHINE_SUPPORTS_sme",
  "GGML_NATIVE",
  "GGML_MACHINE_SUPPORTS_dotprod",
  "GGML_MACHINE_SUPPORTS_i8mm",
  "GGML_MACHINE_SUPPORTS_nosve",
  "GGML_MACHINE_SUPPORTS_nosme",
];

function cmakeBracketArgument(value) {
  const content = String(value).replaceAll("\\", "/");
  let equals = "";
  while (content.includes(`]${equals}]`)) {
    equals += "=";
  }
  return `[${equals}[${content}]${equals}]`;
}

function parseFixtureOutput(output) {
  return Object.fromEntries(
    output.trim().split(/\r?\n/).map((line) => line.split("=")),
  );
}

async function runScriptFixture({
  projectName = "whisper.cpp",
  systemName = "Windows",
  processor = "aarch64",
  pointerSize = 8,
  preseed = {},
}) {
  const directory = await mkdtemp(join(tmpdir(), "sagascript-cmake-arm64-"));
  const fixturePath = join(directory, "fixture.cmake");
  const outputPath = join(directory, "result.txt");
  const assignments = [
    `set(PROJECT_NAME ${cmakeBracketArgument(projectName)})`,
    `set(CMAKE_SYSTEM_NAME ${cmakeBracketArgument(systemName)})`,
    `set(CMAKE_SYSTEM_PROCESSOR ${cmakeBracketArgument(processor)})`,
  ];
  if (pointerSize !== undefined && pointerSize !== null) {
    assignments.push(`set(CMAKE_SIZEOF_VOID_P ${pointerSize})`);
  }
  for (const [name, value] of Object.entries(preseed)) {
    assignments.push(`set(${name} ${value} CACHE BOOL "fixture preseed" FORCE)`);
  }
  const outputLines = machineSupportFlags
    .map((name) => `  "${name}=\${${name}}\\n"`)
    .join("\n");
  const script = [
    ...assignments,
    `include(${cmakeBracketArgument(hookPath)})`,
    `file(WRITE ${cmakeBracketArgument(outputPath)}\n${outputLines}\n)`,
  ].join("\n");
  await writeFile(fixturePath, `${script}\n`, "utf8");

  try {
    const result = spawnSync("cmake", ["-P", fixturePath], {
      encoding: "utf8",
      shell: false,
    });
    let values = null;
    try {
      values = parseFixtureOutput(await readFile(outputPath, "utf8"));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    return {
      ...result,
      combinedOutput: `${result.stdout ?? ""}${result.stderr ?? ""}`,
      values,
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

async function runConfigureFixture() {
  const directory = await mkdtemp(join(tmpdir(), "sagascript-cmake-arm64-configure-"));
  const sourceDirectory = join(directory, "source");
  const buildDirectory = join(directory, "build");
  const outputPath = join(directory, "result.txt");
  await mkdir(sourceDirectory);

  const cmakeLists = [
    "cmake_minimum_required(VERSION 3.16)",
    // Keeping CXX disabled proves the cached result prevents CheckCXXSourceRuns
    // from reaching its language/compiler checks or trying to compile the probe.
    "project(whisper.cpp LANGUAGES NONE)",
    "include(CheckCXXSourceRuns)",
    "set(CMAKE_SYSTEM_NAME Windows)",
    "set(CMAKE_SYSTEM_PROCESSOR aarch64)",
    "set(CMAKE_SIZEOF_VOID_P 8)",
    "set(GGML_MACHINE_SUPPORTS_sve ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_MACHINE_SUPPORTS_sme ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_NATIVE ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_MACHINE_SUPPORTS_dotprod ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_MACHINE_SUPPORTS_i8mm ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_MACHINE_SUPPORTS_nosve ON CACHE BOOL \"fixture preseed\" FORCE)",
    "set(GGML_MACHINE_SUPPORTS_nosme ON CACHE BOOL \"fixture preseed\" FORCE)",
    `include(${cmakeBracketArgument(hookPath)})`,
    "check_cxx_source_runs([=[",
    "#error This source must not be compiled when the hook pre-seeds SVE OFF",
    "int main() { return 0; }",
    "]=] GGML_MACHINE_SUPPORTS_sve)",
    "check_cxx_source_runs([=[#error SME probe must also be skipped]=] GGML_MACHINE_SUPPORTS_sme)",
    "if(GGML_MACHINE_SUPPORTS_sve OR GGML_MACHINE_SUPPORTS_sme)",
    "  message(FATAL_ERROR \"pre-seeded SVE probe unexpectedly ran\")",
    "endif()",
    `file(WRITE ${cmakeBracketArgument(outputPath)}\n${machineSupportFlags.map((name) => `  "${name}=\${${name}}\\n"`).join("\n")}\n)`,
  ].join("\n");
  await writeFile(join(sourceDirectory, "CMakeLists.txt"), `${cmakeLists}\n`, "utf8");

  try {
    const result = spawnSync("cmake", ["-S", sourceDirectory, "-B", buildDirectory], {
      encoding: "utf8",
      shell: false,
    });
    let values = null;
    try {
      values = parseFixtureOutput(await readFile(outputPath, "utf8"));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    return {
      ...result,
      combinedOutput: `${result.stdout ?? ""}${result.stderr ?? ""}`,
      values,
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("ARM64 hook accepts Windows and processor case variants", async () => {
  for (const [systemName, processor] of [
    ["Windows", "aarch64"],
    ["wInDoWs", "AARCH64"],
    ["WINDOWS", "arm64"],
    ["windows", "ARM64"],
  ]) {
    const result = await runScriptFixture({ systemName, processor });
    assert.equal(result.status, 0, result.combinedOutput);
    assert.deepEqual(result.values, {
      GGML_MACHINE_SUPPORTS_sve: "OFF",
      GGML_MACHINE_SUPPORTS_sme: "OFF",
      GGML_NATIVE: "",
      GGML_MACHINE_SUPPORTS_dotprod: "",
      GGML_MACHINE_SUPPORTS_i8mm: "",
      GGML_MACHINE_SUPPORTS_nosve: "",
      GGML_MACHINE_SUPPORTS_nosme: "",
    });
  }
});

test("ARM64 hook forces only SVE and SME off from an old positive cache", async () => {
  const preseed = Object.fromEntries(machineSupportFlags.map((name) => [name, "ON"]));
  const result = await runScriptFixture({ preseed });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.deepEqual(result.values, {
    GGML_MACHINE_SUPPORTS_sve: "OFF",
    GGML_MACHINE_SUPPORTS_sme: "OFF",
    GGML_NATIVE: "ON",
    GGML_MACHINE_SUPPORTS_dotprod: "ON",
    GGML_MACHINE_SUPPORTS_i8mm: "ON",
    GGML_MACHINE_SUPPORTS_nosve: "ON",
    GGML_MACHINE_SUPPORTS_nosme: "ON",
  });
});

test("ARM64 hook is a no-op for unrelated projects", async () => {
  const preseed = Object.fromEntries(machineSupportFlags.map((name) => [name, "ON"]));
  const result = await runScriptFixture({
    projectName: "unrelated-project",
    systemName: "Linux",
    processor: "x86_64",
    pointerSize: 4,
    preseed,
  });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.deepEqual(result.values, preseed);
});

test("ARM64 hook rejects wrong platform, processor, and pointer size", async () => {
  for (const options of [
    { systemName: "Linux", processor: "arm64", pointerSize: 8, diagnostic: /CMAKE_SYSTEM_NAME must be Windows/ },
    { systemName: "Windows", processor: "x86_64", pointerSize: 8, diagnostic: /ARM64 or aarch64 target/ },
    { systemName: "Windows", processor: "arm64", pointerSize: 4, diagnostic: /8-byte pointers/ },
  ]) {
    const result = await runScriptFixture(options);
    assert.notEqual(result.status, 0, result.combinedOutput);
    assert.match(result.combinedOutput, options.diagnostic);
    assert.equal(result.values, null);
  }
});

test("actual CheckCXXSourceRuns skips an invalid SVE probe pre-seeded OFF", async () => {
  const result = await runConfigureFixture();
  assert.equal(result.status, 0, result.combinedOutput);
  assert.deepEqual(result.values, {
    GGML_MACHINE_SUPPORTS_sve: "OFF",
    GGML_MACHINE_SUPPORTS_sme: "OFF",
    GGML_NATIVE: "ON",
    GGML_MACHINE_SUPPORTS_dotprod: "ON",
    GGML_MACHINE_SUPPORTS_i8mm: "ON",
    GGML_MACHINE_SUPPORTS_nosve: "ON",
    GGML_MACHINE_SUPPORTS_nosme: "ON",
  });
});
