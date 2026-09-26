import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const script = fileURLToPath(new URL('./generate-updater-smoke-config.mjs', import.meta.url));
const source = fileURLToPath(new URL('./tauri-updater-smoke-old.json', import.meta.url));

test('smoke updater config includes the current public key and loopback allowance', () => {
  const directory = mkdtempSync(join(tmpdir(), 'sagascript-updater-config-'));
  try {
    const output = join(directory, 'smoke.json');
    execFileSync(process.execPath, [script, source, output], {
      env: { SAGASCRIPT_UPDATER_PUBKEY: 'test-public-key' },
    });
    const config = JSON.parse(readFileSync(output, 'utf8'));
    assert.equal(config.plugins.updater.pubkey, 'test-public-key');
    assert.equal(config.plugins.updater.dangerousInsecureTransportProtocol, true);
    assert.equal(config.version, '1.3.3');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
