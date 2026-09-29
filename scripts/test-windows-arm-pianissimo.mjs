import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const read = (path) => readFile(new URL(path, import.meta.url), "utf8");
const workflow = (await read("../.github/workflows/windows-package.yml")).replace(/\r\n?/g, "\n");
const bundle = JSON.parse(await read("./tauri-windows-engine-host.json"));
const script = await read("./windows-arm-pianissimo-test.ps1");
const verifier = await read("./verify-windows-engine-host.ps1");
const docs = await read("../docs/windows-arm-pianissimo-testing.md");

const ORT_SHA256 = "a3ab2265e52d157ef1c4f4f82f66fc582ce12a780510aac240690ce194b58510";

function step(name) {
  const start = workflow.indexOf(`      - name: ${name}\n`);
  assert.ok(start >= 0, `missing step ${name}`);
  const next = workflow.indexOf("\n      - name:", start + 1);
  return workflow.slice(start, next < 0 ? undefined : next);
}

test("host build and ORT staging run for arm64 only, with a pinned SHA-256", () => {
  const build = step("Build and stage ARM64 ONNX engine host");
  assert.match(build, /if: matrix\.architecture == 'arm64'/);
  assert.match(build, /--manifest-path src-tauri\/engine-host\/ort\/Cargo\.toml --target aarch64-pc-windows-msvc/);
  assert.match(build, /onnxruntime-win-arm64-\$ortVersion\.zip/);
  assert.match(build, /github\.com\/microsoft\/onnxruntime\/releases\/download\/v\$ortVersion\//);
  assert.match(build, /\$ortVersion = '1\.28\.2'/);
  assert.ok(build.includes(ORT_SHA256));
  assert.match(build, /SHA256 mismatch/);
  assert.match(build, /onnxruntime\.dll/);
  assert.match(build, /verify-windows-engine-host\.ps1/);
  assert.ok(workflow.indexOf("name: Build and stage ARM64 ONNX engine host") <
    workflow.indexOf("name: Build unsigned internal installers"));
  assert.ok(workflow.indexOf("name: Build and stage ARM64 ONNX engine host") >
    workflow.indexOf("name: Verify native runner architecture"));
});

test("installer build applies the engine-host bundle config for arm64 only", () => {
  const installer = step("Build unsigned internal installers");
  assert.match(installer, /matrix\.architecture == 'arm64' && '--config scripts\/tauri-windows-engine-host\.json' \|\| ''/);
  assert.match(installer, /--config scripts\/tauri-ci-prebuilt\.json/);
  const verify = step("Verify ARM64 installer carries the ONNX engine host");
  assert.match(verify, /if: matrix\.architecture == 'arm64'/);
  assert.match(verify, /msiexec\.exe/);
  assert.match(verify, /engine-host/);
  assert.match(verify, /verify-windows-engine-host\.ps1/);
});

test("bundle config maps host and DLLs into engine-host/ only", () => {
  const resources = bundle.bundle.resources;
  assert.deepEqual(Object.values(resources).sort(), [
    "engine-host/onnxruntime.dll",
    "engine-host/onnxruntime_providers_shared.dll",
    "engine-host/sagascript-engine-host-ort.exe",
  ]);
  for (const source of Object.keys(resources)) assert.match(source, /^\.\.\/build\/engine-host\/win-arm64\//);
  assert.doesNotMatch(workflowOutsideArm64Steps(), /engine-host/);
});

function workflowOutsideArm64Steps() {
  // The x64 job must not reference the host or DLL outside arm64-gated steps.
  const gated = new Set([
    "Build and stage ARM64 ONNX engine host",
    "Verify ARM64 installer carries the ONNX engine host",
  ]);
  let rest = workflow;
  for (const name of gated) rest = rest.replace(step(name), "");
  rest = rest.replace(/--config scripts\/tauri-windows-engine-host\.json/, "");
  return rest.replace(/engine-host-conformance|sagascript-engine-host-ort\.exe"/g, "");
}

test("verifier checks version, hello and ping beside the DLL", () => {
  assert.match(verifier, /--version/);
  assert.match(verifier, /"op":"hello"/);
  assert.match(verifier, /"op":"ping"/);
  assert.match(verifier, /onnxruntime\.dll/);
});

test("owner test script covers every listed step", () => {
  for (const needle of [
    "Win32_OperatingSystem", "Win32_Processor", "'--version'", "'engine', 'status', '--json'",
    "'download-model', 'pianissimo-sv'", "'engine', 'doctor', '--json'", "'transcribe', '--model', 'pianissimo-sv'",
    "Measure-Command", "swedish-fleurs-hongkong.wav", "raw.githubusercontent.com", "$LongAudio", "$Audio",
    "660 MB", "windows-arm-pianissimo-result.json", "windows-arm-pianissimo-result.txt",
  ]) assert.ok(script.includes(needle), `script is missing ${needle}`);
  assert.doesNotMatch(script, /RunAs|Set-ExecutionPolicy|Add-MpPreference|reg add/i);
});

test("owner test script prefers -Cli, then known installs with a sibling host, then PATH", () => {
  const explicit = script.indexOf("if ($Cli)");
  const known = script.indexOf("$known +=");
  const onPath = script.indexOf("Get-Command sagascript");
  assert.ok(explicit >= 0 && known > explicit && onPath > known, "selection order must be -Cli, known installs, PATH");
  assert.ok(script.includes("engine-host\\sagascript-engine-host-ort.exe"));
  assert.ok(script.includes("Pass -Cli"), "ambiguity must ask for -Cli");
  assert.ok(script.includes("CLI chosen"), "must say which CLI was chosen and why");
  assert.ok(script.includes("onnxruntime_providers_shared.dll"));
});

test("docs cover artifact, SmartScreen, script and hand-back", () => {
  for (const needle of ["windows-arm64-unsigned-candidate", "SmartScreen", "Defender", "windows-arm-pianissimo-test.ps1", ORT_SHA256])
    assert.ok(docs.includes(needle), `docs missing ${needle}`);
});
