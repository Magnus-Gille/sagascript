import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const workflow = readFileSync(fileURLToPath(new URL('../.github/workflows/release-build-macos.yml', import.meta.url)), 'utf8');
const steps = workflow.split(/(?=^      - name: )/m);

test('signed release builds with updater config and starts app before publishing', () => {
  const build = steps.find((step) => step.includes('name: Build, sign, notarize'));
  const launch = steps.findIndex((step) => step.includes('name: Gate signed app startup'));
  const upload = steps.findIndex((step) => step.includes('name: Upload'));
  assert.ok(build);
  assert.match(build, /generate-updater-config\.mjs/);
  assert.match(build, /tauri-updater-signed\.json/);
  assert.match(build, /--config "\$signed_config"/);
  assert.ok(launch > 0 && upload > launch);
  assert.match(steps[launch], /smoke-signed-app-start\.mjs/);
});
