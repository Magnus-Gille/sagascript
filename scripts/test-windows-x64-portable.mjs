import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import test from "node:test";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const hookPath = resolve(scriptDir, "cmake/windows-x64-portable.cmake");

const baselineFlags = {
  GGML_NATIVE: "OFF",
  GGML_AVX: "ON",
  GGML_SSE42: "ON",
  GGML_AVX2: "ON",
  GGML_BMI2: "ON",
  GGML_FMA: "ON",
  GGML_F16C: "ON",
  GGML_AVX_VNNI: "OFF",
  GGML_AVX512: "OFF",
  GGML_AVX512_VBMI: "OFF",
  GGML_AVX512_VNNI: "OFF",
  GGML_AVX512_BF16: "OFF",
  GGML_AMX_TILE: "OFF",
  GGML_AMX_INT8: "OFF",
  GGML_AMX_BF16: "OFF",
  GGML_CPU_ALL_VARIANTS: "OFF",
  GGML_BACKEND_DL: "OFF",
  GGML_LLAMAFILE: "OFF",
};

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

async function runFixture({
  projectName = "whisper.cpp",
  systemName = "Windows",
  processor = "AMD64",
  pointerSize = 8,
  preseed = {},
  directoryPrefix = "sagascript-cmake-",
}) {
  const directory = await mkdtemp(join(tmpdir(), directoryPrefix));
  const fixturePath = join(directory, "fixture.cmake");
  const outputPath = join(directory, "result.txt");
  const assignments = [
    `set(PROJECT_NAME ${cmakeBracketArgument(projectName)})`,
    `set(CMAKE_SYSTEM_NAME ${cmakeBracketArgument(systemName)})`,
    `set(CMAKE_SYSTEM_PROCESSOR ${cmakeBracketArgument(processor)})`,
  ];
  const pointerSizeSet = pointerSize !== undefined && pointerSize !== null;
  if (pointerSizeSet) {
    assignments.push(`set(CMAKE_SIZEOF_VOID_P ${pointerSize})`);
  }
  for (const [name, value] of Object.entries(preseed)) {
    assignments.push(`set(${name} ${value} CACHE BOOL "fixture preseed" FORCE)`);
  }
  const names = Object.keys(baselineFlags);
  const outputLines = names
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
      const output = await readFile(outputPath, "utf8");
      values = parseFixtureOutput(output);
    } catch {
      // A failing hook is expected not to produce the result file.
    }
    return {
      ...result,
      combinedOutput: `${result.stdout ?? ""}${result.stderr ?? ""}`,
      values,
      pointerSizeSet,
    };
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

test("fixture output parser treats LF and CRLF equally", () => {
  const lines = ["GGML_NATIVE=OFF", "GGML_AVX=ON"];
  const expected = { GGML_NATIVE: "OFF", GGML_AVX: "ON" };
  assert.deepEqual(parseFixtureOutput(`${lines.join("\n")}\n`), expected);
  assert.deepEqual(parseFixtureOutput(`${lines.join("\r\n")}\r\n`), expected);
});

test("CMake bracket arguments choose safe delimiters and normalize paths", () => {
  for (const value of ["plain", "a]b", "a]]b", "a]=]b", String.raw`a\\b]]c`]) {
    const normalized = value.replaceAll("\\", "/");
    const match = /^\[(=*)\[/.exec(cmakeBracketArgument(value));
    assert.ok(match);
    const equals = match[1];
    const formatted = cmakeBracketArgument(value);
    assert.equal(formatted, `[${equals}[${normalized}]${equals}]`);
    assert.equal(normalized.includes(`]${equals}]`), false);
  }
});

test("portable hook handles bracketed temporary fixture paths", async () => {
  const result = await runFixture({ directoryPrefix: "sagascript-cmake-]]-" });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.deepEqual(result.values, baselineFlags);
});

test("portable hook forces the shipped Windows x64 baseline", async () => {
  const result = await runFixture({
    preseed: {
      GGML_NATIVE: "ON",
      GGML_AVX2: "OFF",
      GGML_AVX512: "ON",
      GGML_AVX512_VNNI: "ON",
      GGML_AMX_TILE: "ON",
      GGML_CPU_ALL_VARIANTS: "ON",
      GGML_BACKEND_DL: "ON",
      GGML_LLAMAFILE: "ON",
    },
  });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.deepEqual(result.values, baselineFlags);
});

test("portable hook accepts case variants and an unspecified pointer size", async () => {
  const result = await runFixture({
    systemName: "wInDoWs",
    processor: "x86_64",
    pointerSize: null,
  });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.equal(result.pointerSizeSet, false);
  assert.deepEqual(result.values, baselineFlags);
});

test("portable hook is a no-op for unrelated projects", async () => {
  const result = await runFixture({
    projectName: "unrelated-project",
    systemName: "Linux",
    processor: "aarch64",
    pointerSize: 4,
    preseed: {
      GGML_NATIVE: "ON",
      GGML_AVX2: "OFF",
      GGML_AVX512: "ON",
    },
  });
  assert.equal(result.status, 0, result.combinedOutput);
  assert.equal(result.values.GGML_NATIVE, "ON");
  assert.equal(result.values.GGML_AVX2, "OFF");
  assert.equal(result.values.GGML_AVX512, "ON");
});

test("portable hook rejects a non-Windows target", async () => {
  const result = await runFixture({ systemName: "Linux" });
  assert.notEqual(result.status, 0);
  assert.match(result.combinedOutput, /CMAKE_SYSTEM_NAME must be Windows/i);
});

test("portable hook rejects a non-x64 target", async () => {
  const result = await runFixture({ processor: "ARM64" });
  assert.notEqual(result.status, 0);
  assert.match(result.combinedOutput, /AMD64 or x86_64/i);
});

test("portable hook rejects a non-64-bit pointer size", async () => {
  const result = await runFixture({ pointerSize: 4 });
  assert.notEqual(result.status, 0);
  assert.match(result.combinedOutput, /8-byte pointers/i);
});
