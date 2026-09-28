import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const workflow = await readFile(new URL("../.github/workflows/test-build.yml", import.meta.url), "utf8");
const releaseWorkflow = await readFile(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
const buildScript = await readFile(new URL("../src-tauri/build.rs", import.meta.url), "utf8");
const steps = workflow.split(/(?=^      - name: )/m);

test("TEST label is validated before updater signing credentials enter the job", () => {
  const validation = steps.findIndex((step) => step.includes("name: Validate TEST artifact label"));
  const signing = steps.findIndex((step) => step.includes("name: Build, sign, notarize"));
  assert.ok(validation > 0 && signing > validation);
  assert.match(steps[validation], /TEST_LABEL: \$\{\{ inputs\.label \}\}/);
  assert.match(steps[validation], /\^\[A-Za-z0-9\]\[A-Za-z0-9\._-\]\{0,39\}\$/);
});

test("manual workflow input never enters a shell program through expression interpolation", () => {
  for (const step of steps) {
    const script = step.split(/\n        run: \|\n/)[1];
    if (script) assert.doesNotMatch(script, /\$\{\{ inputs\.label \}\}/);
  }
  const signing = steps.find((step) => step.includes("name: Verify macOS test artifact"));
  assert.ok(signing);
  assert.match(signing, /TEST_LABEL: \$\{\{ inputs\.label \}\}/);
  assert.match(signing, /label="\$TEST_LABEL"/);
});

test("signed upgrade builds the updated app with its generated updater config", () => {
  const smoke = steps.find((step) => step.includes("name: Gate real signed self-upgrade"))?.replace(/\r\n/g, "\n");
  assert.ok(smoke);
  assert.match(smoke, /generate-updater-config\.mjs/);
  assert.match(smoke, /scripts\/tauri-updater-smoke-new\.json "\$updated_smoke_config"/);
  assert.match(smoke, /--config "\$updated_smoke_config"/);
});

test("downloadable signed app uses generated secure config and is launched before upload", () => {
  const build = steps.find((step) => step.includes("name: Build, sign, notarize"));
  const launch = steps.findIndex((step) => step.includes("name: Gate signed app startup"));
  const upload = steps.findIndex((step) => step.includes("name: Upload macOS test artifacts"));
  assert.ok(build);
  assert.match(build, /generate-updater-config\.mjs/);
  assert.match(build, /tauri-updater-signed\.json/);
  assert.match(build, /--config "\$signed_config"/);
  assert.ok(launch > 0 && upload > launch);
  assert.match(steps[launch], /smoke-signed-app-start\.mjs/);
});

test("updater key requirement applies only to intentional signed distribution builds", () => {
  const signed = steps.find((step) => step.includes("name: Build, sign, notarize"));
  const release = releaseWorkflow.split(/(?=^      - name: )/m).find((step) => step.includes("name: Build, sign, notarize"));
  assert.match(signed, /SAGASCRIPT_REQUIRE_UPDATER_PUBKEY: ['"]?1['"]?/);
  assert.match(release, /SAGASCRIPT_REQUIRE_UPDATER_PUBKEY: ['"]?1['"]?/);
  assert.match(buildScript, /SAGASCRIPT_REQUIRE_UPDATER_PUBKEY/);
  assert.doesNotMatch(buildScript, /if is_macos_release && !updater_pubkey_configured/);
});

test("experimental Pianissimo Metal is opt-in to TEST, built before credentials, and signed-smoked", () => {
  assert.match(workflow, /pianissimo_banded_runtime:\s*\n(?:\s+[^\n]+\n)*?\s+type: boolean/);
  const validation = steps.find((step) => step.includes("name: Validate TEST artifact label"));
  assert.ok(validation?.includes("PIANISSIMO_BANDED_RUNTIME: ${{ inputs.pianissimo_banded_runtime }}"),
    "the optional build must require an explicit TEST label before credentials");
  assert.ok(validation?.includes('"$TEST_LABEL" != *banded*'),
    "experimental artifacts must be identifiable by name");
  const cpu = steps.findIndex((step) => step.includes("name: Build bundled Pianissimo CPU runtime"));
  const metal = steps.findIndex((step) => step.includes("name: Build experimental banded Pianissimo Metal runtime"));
  const reference = steps.findIndex((step) => step.includes("name: Stage pinned unpatched Pianissimo Metal reference"));
  const long = steps.findIndex((step) => step.includes("name: Gate experimental long Pianissimo Metal parity"));
  const certificate = steps.findIndex((step) => step.includes("name: Import Developer ID certificate"));
  const signedMetal = steps.findIndex((step) => step.includes("name: Gate signed Pianissimo Metal transcription"));
  const signedLong = steps.findIndex((step) => step.includes("name: Gate signed long Pianissimo Metal transcription"));
  assert.ok(cpu > 0 && metal > cpu && reference > metal && long > reference
    && certificate > long && signedMetal > certificate && signedLong > signedMetal,
    "both unsigned parity and signed long-path smoke must gate the experimental TEST");
  assert.match(steps[cpu], /if:.*!inputs\.pianissimo_banded_runtime/);
  assert.match(steps[metal], /if:.*inputs\.pianissimo_banded_runtime/);
  assert.match(steps[signedMetal], /if:.*inputs\.pianissimo_banded_runtime/);
  assert.match(steps[signedMetal], /--pianissimo-device metal/);
  assert.ok(steps[long].includes("make-pianissimo-long-smoke.py"));
  assert.ok(steps[long].includes("--quality-only"));
  assert.ok(steps[long].includes("--require-exact-text"));
  assert.doesNotMatch(steps[long], /--max-timestamp-(?:delta-ms|changed-words)\s+[1-9]/);
  assert.ok(steps[signedLong].includes("--pianissimo-device metal"));
  assert.doesNotMatch(releaseWorkflow, /build-pianissimo-banded-runtime/);
});
