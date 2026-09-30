import assert from 'node:assert/strict';
import test from 'node:test';
import ts from 'typescript';
import { readFile } from 'node:fs/promises';
const source = await readFile(new URL('../src/lib/update-preparation.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { drainUpdateWork } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`);
const deferred = () => { let resolve; const promise = new Promise(r => resolve = r); return { promise, resolve }; };

test('waits for terminal result delivery and rendering before snapshot', async () => {
  let busy = true, rendered = false, done = false;
  const work = drainUpdateWork({ busy: () => busy, settle: async () => { if (!busy) rendered = true; }, timeoutMs: 200, pollMs: 1 }).then(() => done = true);
  await new Promise(r => setTimeout(r, 5));
  assert.equal(done, false);
  busy = false;
  await work;
  assert.equal(rendered, true);
});

test('waits for an in-flight correction even when transcription is idle', async () => {
  const edit = deferred(); let snapshotted = false;
  const work = drainUpdateWork({ busy: () => false, settle: () => edit.promise, timeoutMs: 200 }).then(() => snapshotted = true);
  await Promise.resolve();
  assert.equal(snapshotted, false);
  edit.resolve();
  await work;
  assert.equal(snapshotted, true);
});

test('blocked handoff and failed initialization fail closed', async () => {
  await assert.rejects(drainUpdateWork({ busy: () => true, settle: async () => {}, timeoutMs: 10, pollMs: 1 }), /finish/);
  await assert.rejects(drainUpdateWork({ busy: () => false, settle: () => new Promise(() => {}), timeoutMs: 10 }), /finish/);
  await assert.rejects(drainUpdateWork({ busy: () => false, settle: async () => { throw new Error('review failed'); } }), /review failed/);
});
