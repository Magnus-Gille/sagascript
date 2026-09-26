import assert from "node:assert/strict";
import test from "node:test";
import { readFile } from "node:fs/promises";

const workflow = await readFile(new URL("../.github/workflows/test-build.yml", import.meta.url), "utf8");
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
  assert.match(smoke, /generate-updater-smoke-config\.mjs \\\n+\s+scripts\/tauri-updater-smoke-new\.json "\$updated_smoke_config"/);
  assert.match(smoke, /--config "\$updated_smoke_config"/);
});
