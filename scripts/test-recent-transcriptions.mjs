import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";
const source = await readFile(new URL("../src/lib/recent-transcriptions.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { rememberTranscription } = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);
const run = (path, profileId = null) => ({ path, profileId, prompt: "", diarize: false });
test("five recent files retain their own options, deduplicate and move re-runs to the front", () => {
  let history = [];
  for (let i = 0; i < 6; i++) history = rememberTranscription(history, run(`/audio/${i}.wav`, `p${i}`));
  assert.deepEqual(history.map(r => r.path), [5,4,3,2,1].map(i => `/audio/${i}.wav`));
  const old = history;
  history = rememberTranscription(history, { ...run('/audio/2.wav'), prompt: 'Names', diarize: true });
  assert.equal(history.length, 5);
  assert.equal(history[0].profileId, null);
  assert.equal(history[0].prompt, 'Names');
  assert.equal(history[0].diarize, true);
  assert.equal(old[0].path, '/audio/5.wav');
  assert.equal(history[1].profileId, 'p5');
});
test("same basenames in different directories remain distinct, paths are not trimmed", () => {
  let history = rememberTranscription([], run('/one/talk.wav'));
  history = rememberTranscription(history, run('/two/talk.wav'));
  history = rememberTranscription(history, run('/two/talk.wav '));
  assert.equal(history.length, 3);
  assert.equal(history[0].path, '/two/talk.wav ');
});
