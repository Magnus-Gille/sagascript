import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const smoke = fileURLToPath(new URL('./smoke-signed-app-start.mjs', import.meta.url));

function withFakeApp(source, check) {
  const directory = mkdtempSync(join(tmpdir(), 'sagascript-startup-test-'));
  try {
    const app = join(directory, 'Sagascript.app');
    const binaryDir = join(app, 'Contents', 'MacOS');
    mkdirSync(binaryDir, { recursive: true });
    const binary = join(binaryDir, 'sagascript');
    writeFileSync(binary, `#!${process.execPath}\n${source}\n`);
    chmodSync(binary, 0o755);
    const result = spawnSync(process.execPath, [smoke, app], {
      encoding: 'utf8',
      timeout: 10_000,
      env: { ...process.env, RUNNER_TEMP: directory },
    });
    check(result);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test('signed app gate accepts a process that completes startup and stays alive', { skip: process.platform === 'win32' }, () => {
  withFakeApp("console.log('Background launch complete'); setInterval(() => {}, 1000);", (result) => {
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Signed Sagascript app completed background startup/);
  });
});

test('signed app gate rejects an updater plugin crash before startup', { skip: process.platform === 'win32' }, () => {
  withFakeApp("console.error('PluginInitialization updater'); process.exit(101);", (result) => {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /PluginInitialization updater/);
  });
});

test('signed app gate rejects an immediate exit after startup', { skip: process.platform === 'win32' }, () => {
  withFakeApp("console.log('Background launch complete'); process.exit(0);", (result) => {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /App exited immediately after background startup/);
  });
});
