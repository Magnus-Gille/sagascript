import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scriptPath = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "verify-windows-x64-cpu-policy.ps1",
);
const pwsh = process.env.PWSH ?? "pwsh";
const pwshProbeDirectory = mkdtempSync(join(tmpdir(), "sagascript-pwsh-probe-"));
let pwshProbe;
try {
  pwshProbe = spawnSync(pwsh, ["-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"], {
    cwd: pwshProbeDirectory,
    encoding: "utf8",
    shell: false,
  });
} finally {
  rmSync(pwshProbeDirectory, { recursive: true, force: true });
}
const pwshAvailable = pwshProbe.status === 0;
const windowsHost = process.platform === "win32" || process.env.OS === "Windows_NT";
const testWindows = pwshAvailable || windowsHost
  ? test
  : (name, fn) => test(name, { skip: "pwsh unavailable; Windows-only verifier tests" }, fn);

const baseline = {
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

const flagTypes = Object.fromEntries(
  Object.keys(baseline).map((name) => [name, "BOOL"]),
);

function makeCache(policyPath, {
  projectName = "whisper.cpp",
  includeValue = policyPath,
  omit = [],
  overrides = {},
  duplicate = null,
} = {}) {
  const values = {
    CMAKE_PROJECT_NAME: projectName,
    CMAKE_PROJECT_INCLUDE: includeValue,
    ...baseline,
    ...overrides,
  };
  const lines = [
    "// This is a synthetic cache for the verifier test.",
    `CMAKE_PROJECT_NAME:STATIC=${values.CMAKE_PROJECT_NAME}`,
    `CMAKE_PROJECT_INCLUDE:FILEPATH=${values.CMAKE_PROJECT_INCLUDE}`,
    ...Object.entries(baseline)
      .filter(([name]) => !omit.includes(name))
      .map(([name]) => `${name}:${flagTypes[name]}=${values[name]}`),
  ];
  if (duplicate) {
    lines.push(`${duplicate}:${flagTypes[duplicate] ?? "BOOL"}=${values[duplicate]}`);
  }
  return `${lines.join("\n")}\n`;
}

async function runVerifier({ caches, unrelated = [], expectedStatus = 0, configure } = {}) {
  const directory = await mkdtemp(join(tmpdir(), "sagascript-cpu-policy-"));
  const targetRoot = join(directory, "target");
  const policyPath = join(directory, "policy", "windows-x64-portable.cmake");
  await mkdir(targetRoot, { recursive: true });
  await mkdir(dirname(policyPath), { recursive: true });
  await writeFile(policyPath, "# synthetic policy\n", "utf8");

  const cacheFiles = [];
  for (const spec of [...(caches ?? []), ...unrelated]) {
    const path = join(targetRoot, spec.relative);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, spec.content ?? makeCache(policyPath, spec), "utf8");
    cacheFiles.push(path);
  }
  const before = new Map(
    await Promise.all(cacheFiles.map(async (path) => [path, await readFile(path, "utf8")])),
  );

  try {
    const args = [
      "-NoProfile",
      "-File",
      scriptPath,
      "-TargetRoot",
      targetRoot,
      "-PolicyFile",
      policyPath,
    ];
    const result = spawnSync(pwsh, args, {
      cwd: directory,
      encoding: "utf8",
      shell: false,
    });
    const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
    assert.equal(result.status, expectedStatus, output);
    if (configure) configure({ output, targetRoot, policyPath });
    return { output, targetRoot, policyPath };
  } finally {
    await Promise.all(
      cacheFiles.map(async (path) => assert.equal(await readFile(path, "utf8"), before.get(path))),
    );
    await rm(directory, { recursive: true, force: true });
  }
}

testWindows("verifies debug and explicit-target whisper.cpp caches", async () => {
  await runVerifier({
    caches: [
      { relative: "debug/whisper-rs-sys/CMakeCache.txt" },
      { relative: "explicit-target/CMakeCache.txt" },
    ],
    unrelated: [
      { relative: "other/CMakeCache.txt", content: "foreign cache is ignored\n" },
    ],
    configure: ({ output }) => {
      assert.match(output, /Verified 2 whisper\.cpp CMake cache\(s\)/);
      assert.equal((output.match(/Policy pass:/g) ?? []).length, 2);
    },
  });
});

testWindows("ignores foreign project caches while requiring a matching cache", async () => {
  await runVerifier({
    caches: [{ relative: "whisper-rs-sys/CMakeCache.txt" }],
    unrelated: [{ relative: "foreign/CMakeCache.txt", projectName: "foreign" }],
    configure: ({ output }) => assert.match(output, /Verified 1 whisper\.cpp/),
  });
});

testWindows("fails closed when no matching cache exists", async () => {
  await runVerifier({
    unrelated: [{ relative: "foreign/CMakeCache.txt", projectName: "foreign" }],
    expectedStatus: 1,
  });
});

testWindows("fails closed when a required baseline flag is missing", async () => {
  await runVerifier({
    caches: [{ relative: "debug/CMakeCache.txt", omit: ["GGML_F16C"] }],
    expectedStatus: 1,
  });
});

testWindows("fails closed when an advanced instruction set is enabled", async () => {
  await runVerifier({
    caches: [{ relative: "debug/CMakeCache.txt", overrides: { GGML_AVX512: "ON" } }],
    expectedStatus: 1,
  });
});

testWindows("fails closed when all CPU variants are enabled", async () => {
  await runVerifier({
    caches: [{ relative: "debug/CMakeCache.txt", overrides: { GGML_CPU_ALL_VARIANTS: "ON" } }],
    expectedStatus: 1,
  });
});

testWindows("fails closed when the include path does not match exactly", async () => {
  await runVerifier({
    caches: [{ relative: "debug/CMakeCache.txt", includeValue: "/tmp/other-policy.cmake" }],
    expectedStatus: 1,
  });
});

testWindows("fails closed on duplicate cache entries", async () => {
  await runVerifier({
    caches: [{ relative: "debug/CMakeCache.txt", duplicate: "GGML_AVX2" }],
    expectedStatus: 1,
  });
});
