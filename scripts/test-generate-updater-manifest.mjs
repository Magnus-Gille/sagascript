import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';

const dir = mkdtempSync(join(tmpdir(), 'sagascript-updater-manifest-'));
const bundle = join(dir, 'Sagascript.app.tar.gz');
const signature = `${bundle}.sig`;
const output = join(dir, 'latest.json');
const script = new URL('./generate-updater-manifest.mjs', import.meta.url).pathname;
writeFileSync(bundle, 'fixture archive');
writeFileSync(signature, 'fixture signature\n');

function run(version = '1.3.3') {
  return spawnSync(process.execPath, [script, version, bundle, signature, output], { encoding: 'utf8' });
}

assert.equal(run().status, 0);
assert.deepEqual(JSON.parse(readFileSync(output, 'utf8')), {
  version: '1.3.3',
  platforms: {
    'darwin-aarch64': {
      url: 'https://github.com/Magnus-Gille/sagascript/releases/download/v1.3.3/Sagascript.app.tar.gz',
      signature: 'fixture signature',
    },
  },
});
assert.notEqual(run('1.3.3-beta.1').status, 0);
writeFileSync(signature, '');
assert.notEqual(run().status, 0);
