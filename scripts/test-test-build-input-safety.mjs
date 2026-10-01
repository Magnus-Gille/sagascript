import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const workflow = await readFile(new URL("../.github/workflows/test-build.yml", import.meta.url), "utf8");
const releaseWorkflow = await readFile(new URL("../.github/workflows/release-build-macos.yml", import.meta.url), "utf8");
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
