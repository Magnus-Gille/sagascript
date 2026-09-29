import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const read = (path) => readFileSync(fileURLToPath(new URL(`../${path}`, import.meta.url)), "utf8");
const exists = (path) => existsSync(fileURLToPath(new URL(`../${path}`, import.meta.url)));

const macWorkflows = [".github/workflows/test-build.yml", ".github/workflows/release.yml"];
const staleNeedles = [
  "nemo-speech",
  "PianissimoRuntime",
  "pianissimo-native-runtime",
  "tauri-pianissimo-bundle",
  "prepare-pianissimo-release-model",
  "sign-pianissimo-runtime",
  ".gguf",
];

test("NeMo/GGUF Pianissimo packaging is gone", () => {
  for (const path of [
    "scripts/build-pianissimo-native-runtime.sh",
    "scripts/build-pianissimo-runtime.sh",
    "scripts/build-pianissimo-model.sh",
    "scripts/prepare-pianissimo-release-model.sh",
    "scripts/sign-pianissimo-runtime.py",
    "scripts/verify-pianissimo-runtime.py",
    "scripts/tauri-pianissimo-bundle.json",
    "scripts/pianissimo-runtime-inventory.json",
  ]) {
    assert(!exists(path), `${path} must not exist`);
  }
  for (const path of [...macWorkflows, ".github/workflows/ci.yml", "scripts/verify-macos-release.sh"]) {
    const content = read(path);
    for (const needle of staleNeedles) {
      assert(!content.includes(needle), `${path} still references ${needle}`);
    }
  }
});

test("engine host bundle config maps the staged binary into Resources/EngineHost", () => {
  const config = JSON.parse(read("scripts/tauri-engine-host-bundle.json"));
  assert.deepEqual(config.bundle.macOS.files, {
    "Resources/EngineHost/sagascript-engine-host": "../build/engine-host/sagascript-engine-host",
  });
  assert.match(read("scripts/stage-engine-host.sh"), /build\/engine-host/);
});

test("signed macOS workflows stage, sign, bundle, verify, and smoke the engine host", () => {
  for (const path of macWorkflows) {
    const content = read(path);
    assert.match(content, /scripts\/stage-engine-host\.sh/, `${path} stages the host`);
    assert.match(
      content,
      /codesign --force --options runtime --timestamp --sign "\$APPLE_SIGNING_IDENTITY" build\/engine-host\/sagascript-engine-host/,
      `${path} signs the host with the hardened runtime`,
    );
    assert.match(content, /--config scripts\/tauri-engine-host-bundle\.json/, `${path} bundles the host`);
    assert.match(content, /verify-macos-release\.sh "\$app" "\$dmg" "\$version" "\$GITHUB_SHA"/, `${path} verifies the host revision`);
    assert.match(content, /scripts\/smoke-pianissimo-installed\.sh/, `${path} smokes Pianissimo`);
    assert.doesNotMatch(content, /pianissimo-sv[^\n]*\.gguf/, `${path} must not ship a GGUF`);
  }
});

test("release verifier checks the bundled host", () => {
  const content = read("scripts/verify-macos-release.sh");
  assert.match(content, /Contents\/Resources\/EngineHost\/sagascript-engine-host/);
  assert.match(content, /flags=0x10000\\\(runtime\\\)/);
  assert.match(content, /check-engine-host-identity\.sh/);
});

test("CI builds the host, runs swift test, and checks the revision", () => {
  const content = read(".github/workflows/ci.yml");
  assert.match(content, /scripts\/stage-engine-host\.sh/);
  assert.match(content, /swift test/);
  assert.match(content, /check-engine-host-identity\.sh[^\n]*GITHUB_SHA/);
});

test("Pianissimo smoke covers status, download, doctor and transcribe", () => {
  const content = read("scripts/smoke-pianissimo-installed.sh");
  for (const step of ["engine status --json", "download-model pianissimo-sv", "engine doctor --json", "transcribe --language sv --model pianissimo-sv --json"]) {
    assert(content.includes(step), `smoke script runs ${step}`);
  }
});
