import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const html = fs.readFileSync(new URL('./reference_review.html', import.meta.url), 'utf8');
const script = html.match(/<script>([\s\S]*)<\/script>/)[1];
const context = { console, globalThis: {} };
vm.runInNewContext(script, context);
const api = context.globalThis.ReferenceReview;
assert.ok(api, 'pure review helpers are exposed');

const evidence = { windows: [{ id: 'w1', start: 0, end: 3, frame_hz: 25, vad: [0, 0.5, 1], asd: [[0.2, 0.3, 0.4], [0.8, 0.7, 0.6]], ui: [[0, 1, 0], [1, 0, 1]], waveform: [0.1, 0.2, 0.3] }] };
assert.equal(api.validateEvidence(evidence).ok, true, 'finite equal-length evidence is valid');
const normalizedEvidence = api.normalizeEvidence(evidence);
assert.equal(normalizedEvidence.windows[0].visual_unavailable[0].length, 3, 'normalization supplies visual masks');
assert.equal(api.validateEvidence({ ...evidence, windows: [{ ...evidence.windows[0], frame_hz: Infinity }] }).ok, false, 'non-finite frame rate is rejected');
assert.equal(api.validateEvidence({ ...evidence, windows: [{ ...evidence.windows[0], vad: [0, 1] }] }).ok, false, 'mismatched series length is rejected');
assert.equal(api.validateEvidence({ ...evidence, windows: [{ ...evidence.windows[0], waveform: [0, 1, 2] }] }).ok, false, 'out-of-range scores are rejected');
assert.equal(api.fmtPrecise(0.04), '0:00.04', 'short interval start is visible');
assert.equal(api.fmtPrecise(0.12), '0:00.12', 'short interval end is visible');
assert.notEqual(api.fmtPrecise(0.04), api.fmtPrecise(0.12), 'short interval endpoints remain distinct');
assert.equal(api.fmtPrecise(Infinity), '—', 'non-finite timestamp is guarded');

const source = {
  source_sha256: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', duration_seconds: 20, speakers: ['S1', 'S2'],
  windows: [{ id: 'w1', start: 0, end: 20 }],
  intervals: [{ start: 2, end: 5, speakers: ['S1'], status: 'candidate', evidence: [{ kind: 'grid', score: 0.8 }], window_id: 'w1' }],
  top_level_extra: ['keep']
};
assert.equal(api.validateDataset(source).ok, true);
assert.equal(api.validateDataset({ ...source, intervals: [{ ...source.intervals[0], window_id: 'w1', start: 19, end: 21 }] }).ok, false, 'intervals cannot leave their window');
assert.equal(api.validateDataset({ ...source, intervals: [{ ...source.intervals[0], id: 'unsupported' }] }).ok, false, 'unsupported interval fields are rejected');
assert.equal(api.validateDataset({ ...source, intervals: [{ ...source.intervals[0], activity: 'silence' }] }).ok, false, 'speech candidates need speakers');
assert.equal(api.validateDataset({ ...source, intervals: [{ ...source.intervals[0], speakers: [], status: 'unknown' }] }).ok, true, 'unknown intervals have no speakers');
assert.equal(api.findContainingWindow(source.windows, 2, 5).id, 'w1');
assert.equal(api.findContainingWindow(source.windows, -1, 5), null, 'outside intervals have no containing window');
assert.equal('id' in api.createInterval({ start: 2, end: 5, speakers: ['S1'] }), false, 'new intervals use canonical fields only');
assert.equal(api.verifyInterval(source.intervals[0], ' ', api.VERIFY_CONFIRMATION), null, 'blank reviewer cannot verify');
assert.equal(api.verifyInterval(source.intervals[0], 'Reviewer', 'I listened'), null, 'missing confirmation cannot verify');
const verified = api.verifyInterval(source.intervals[0], 'Reviewer', api.VERIFY_CONFIRMATION, '2026-01-01T00:00:00.000Z');
assert.equal(verified.status, 'verified');
assert.equal(verified.reviewed_at, '2026-01-01T00:00:00.000Z');
assert.equal(verified.evidence.at(-1).kind, 'human_audio_video');
assert.equal(api.validateDataset({ ...source, intervals: [verified] }).ok, true, 'verified export remains canonical-compatible');
assert.equal(api.verifyInterval({ ...source.intervals[0], status: 'unknown', speakers: [] }, 'Reviewer', api.VERIFY_CONFIRMATION), null, 'unknown intervals cannot be verified');
assert.equal(api.verifyInterval(source.intervals[0], 'system', api.VERIFY_CONFIRMATION), null, 'nonhuman reviewer names cannot verify');
assert.equal(api.validateDataset({ ...source, intervals: [{ ...verified, reviewer: 'assistant' }] }).ok, false, 'nonhuman imported verification is rejected');
assert.equal(JSON.stringify(verified.evidence.slice(0, -1)), JSON.stringify(source.intervals[0].evidence));
const edited = api.editInterval(verified, { start: 3, end: 6, speakers: ['S2'], activity: 'speech', status: 'candidate' });
assert.equal(edited.status, 'candidate', 'ordinary edits cannot retain verified state');
assert.equal('reviewed_at' in edited, false, 'ordinary edits clear stale review time');
assert.equal('reviewer' in edited, false, 'ordinary edits clear stale reviewer');
assert.equal(edited.window_id, 'w1');
assert.equal(source.top_level_extra[0], 'keep');
assert.equal(JSON.stringify(api.coverageSummary({ intervals: [verified, { start: 4, end: 8, status: 'candidate' }] })), JSON.stringify({ verified: 3, candidate: 4 }));

const masked = { ...source, windows: [{ id: 'w1', start: 0, end: 10 }], intervals: [{ start: 0, end: 10, speakers: [], status: 'unknown', evidence: [{ kind: 'mask', score: 0.4 }], window_id: 'w1' }] };
const humanSubrange = api.verifyInterval({ start: 4, end: 6, speakers: ['S1'], status: 'candidate', evidence: [], window_id: 'w1' }, 'Reviewer', api.VERIFY_CONFIRMATION, '2026-01-01T00:00:00.000Z');
const split = api.replaceInterval(masked, humanSubrange);
assert.equal(split.ok, true, 'verified replacement inside unknown mask succeeds');
assert.equal(JSON.stringify(split.dataset.intervals.map(({ start, end, status }) => ({ start, end, status }))), JSON.stringify([
  { start: 0, end: 4, status: 'unknown' }, { start: 4, end: 6, status: 'verified' }, { start: 6, end: 10, status: 'unknown' }
]));
assert.equal(api.validateDataset(split.dataset).ok, true, 'split export remains canonical-compatible');
assert.equal(api.coverageSummary(split.dataset).verified, 2, 'coverage includes only the verified subrange');
assert.equal(JSON.stringify(split.dataset.intervals[0].evidence), JSON.stringify(masked.intervals[0].evidence), 'left remainder preserves evidence metadata');
assert.equal(JSON.stringify(split.dataset.intervals[2].evidence), JSON.stringify(masked.intervals[0].evidence), 'right remainder preserves evidence metadata');
assert.equal(api.replaceInterval(split.dataset, { start: 5, end: 7, speakers: ['S2'], status: 'candidate', evidence: [], window_id: 'w1' }).ok, false, 'verified overlap is rejected');
console.log('reference review helpers: ok');
