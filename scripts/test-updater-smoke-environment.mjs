import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { prepareUpdaterSmokeEnvironment } from './updater-smoke-environment.mjs';
test('signed upgrade starts onboarded with isolated settings, preserving the original settings', () => {
  const dir = mkdtempSync(join(tmpdir(), 'sagascript-smoke-env-test-'));
  try {
    const original = join(dir, 'original.json');
    writeFileSync(original, 'original settings');
    const base = { SAGASCRIPT_SETTINGS_PATH: original, PATH: '/synthetic' };
    const env = prepareUpdaterSmokeEnvironment(dir, base);
    assert.notEqual(env.SAGASCRIPT_SETTINGS_PATH, original);
    assert.ok(env.SAGASCRIPT_SETTINGS_PATH.startsWith(dir));
    assert.deepEqual(JSON.parse(readFileSync(env.SAGASCRIPT_SETTINGS_PATH, 'utf8')), { has_completed_onboarding: true, auto_paste: false });
    assert.equal(readFileSync(original, 'utf8'), 'original settings');
    assert.equal(base.SAGASCRIPT_SETTINGS_PATH, original);
    assert.equal(env.PATH, '/synthetic');
  } finally { rmSync(dir, { recursive: true }); }
});
