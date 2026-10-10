import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { validateObjective, validateObjectiveSeries } from '../../scripts/lib/code-health-objective.mjs';

const readJson = async path => JSON.parse(await readFile(new URL(path, import.meta.url), 'utf8'));
const schema = await readJson('../../docs/code-health-objective-v1.schema.json');
const positive = await readJson('../fixtures/code-health/objective-positive.json');
const casesFixture = await readJson('../fixtures/code-health/objective-cases.json');

function atPath(root, path) {
  return path.reduce((value, key) => value[key], root);
}
function parentAtPath(root, path) {
  return atPath(root, path.slice(0, -1));
}
function applyOperations(record, operations) {
  const result = structuredClone(record);
  for (const operation of operations) {
    if (operation.op === 'set') parentAtPath(result, operation.path)[operation.path.at(-1)] = operation.value;
    else if (operation.op === 'delete') delete parentAtPath(result, operation.path)[operation.path.at(-1)];
    else if (operation.op === 'splice') atPath(result, operation.path).splice(operation.index, 1);
    else if (operation.op === 'append-from') atPath(result, operation.path).push(structuredClone(atPath(result, operation.source_path)));
    else assert.fail(`unknown fixture operation: ${operation.op}`);
  }
  return result;
}

const valid = validateObjective(schema, positive);
assert.equal(valid.valid, true, `positive fixture rejected: ${JSON.stringify(valid)}`);
assert.deepEqual(valid.schemaErrors, []);
assert.deepEqual(valid.semanticErrors, []);

const aggregate = valid.aggregate;
assert.equal(aggregate.metrics.unused_candidates.status, 'measured');
assert.equal(aggregate.metrics.unused_candidates.candidate_count, 0, 'measured zero remains a numeric zero');
assert.equal(aggregate.metrics.unused_candidates.graph_status, 'unqualified', 'unqualified candidates remain candidate counts');
assert.deepEqual(aggregate.state_counts, { measured: 5, unknown: 0, unsupported: 0, failed: 0, stale: 0 });
assert.deepEqual({
  repository: aggregate.repository,
  commit: aggregate.commit,
  observed_at: aggregate.observed_at,
  contract_version: aggregate.contract_version,
  supersedes_ref: aggregate.supersedes_ref,
  correction_ref: aggregate.correction_ref
}, {
  repository: positive.repository,
  commit: positive.commit,
  observed_at: positive.observed_at,
  contract_version: positive.contract_version,
  supersedes_ref: positive.supersedes_ref,
  correction_ref: positive.correction_ref
}, 'aggregate retains exact envelope binding');
assert.notEqual(aggregate.metrics.complex_functions.source.run_ref, aggregate.metrics.coverage.source.run_ref);
assert.equal(aggregate.metrics.coverage.unit, 'lines');
assert.equal(aggregate.metrics.coverage.observed_at, '2026-10-09T09:30:00Z');
assert.deepEqual(aggregate.metrics.ci_first_attempt.counts_by_conclusion, {
  success: 1, failure: 1, infra_failure: 1, cancelled: 1, skipped: 1, pending: 1, unknown: 1
});
assert.deepEqual(aggregate.metrics.ci_first_attempt.job_counts_by_conclusion, {
  success: 8, failure: 1, infra_failure: 1, cancelled: 1, skipped: 1, pending: 1, unknown: 1
});
assert.equal(aggregate.metrics.ci_first_attempt.numerator, 1);
assert.equal(aggregate.metrics.ci_first_attempt.denominator, 3, 'infra failures count in the denominator; other terminal/incomplete outcomes stay separate');
assert.equal(aggregate.metrics.ci_first_attempt.fraction, 1 / 3);
assert.equal(aggregate.metrics.ci_first_attempt.expected_runs, 7, 'the aggregate retains the full expected run inventory');
assert.equal(positive.metrics.ci_first_attempt.payload.runs[1].attempt, 1);
assert.equal(positive.metrics.ci_first_attempt.payload.runs[1].latest_attempt, 2, 'a later retry does not overwrite the first-attempt record');
assert.deepEqual(aggregate.metrics.confirmed_regressions.severity_counts, { critical: 1, major: 1, minor: 1 });
assert.equal(aggregate.metrics.confirmed_regressions.eligible, 2);
assert.equal(aggregate.metrics.confirmed_regressions.sparse, true);

for (const testCase of casesFixture.cases) {
  const candidate = applyOperations(positive, testCase.operations);
  const result = validateObjective(schema, candidate);
  if (testCase.expected === 'valid') {
    assert.equal(result.valid, true, `${testCase.name} should pass: ${JSON.stringify(result)}`);
    if (testCase.name === 'measured-zero-is-not-unknown') {
      assert.deepEqual(result.aggregate.state_counts, { measured: 4, unknown: 1, unsupported: 0, failed: 0, stale: 0 });
      assert.equal(result.aggregate.metrics.unused_candidates.status, 'unknown');
      assert.equal(Object.hasOwn(result.aggregate.metrics.unused_candidates, 'candidate_count'), false);
    }
    if (testCase.name === 'nonmeasured-slot-with-no-source-or-population') {
      assert.equal(result.aggregate.metrics.unused_candidates.source, null);
      assert.equal(result.aggregate.metrics.unused_candidates.population, null);
    }
    if (testCase.name === 'infra-failure-and-cancellation-are-separated') {
      const counts = result.aggregate.metrics.ci_first_attempt.counts_by_conclusion;
      assert.equal(counts.infra_failure, 1);
      assert.equal(counts.cancelled, 1);
      assert.equal(counts.success + counts.failure + counts.infra_failure, 3);
    }
    if (testCase.name === 'failure-precedes-cancelled-dependent-job') {
      const counts = result.aggregate.metrics.ci_first_attempt.counts_by_conclusion;
      const jobCounts = result.aggregate.metrics.ci_first_attempt.job_counts_by_conclusion;
      assert.equal(counts.failure, 1, 'a known failure remains in the denominator when another expected job is cancelled');
      assert.equal(counts.cancelled, 1);
      assert.equal(jobCounts.cancelled, 2, 'both job outcomes remain visible');
    }
    if (testCase.name === 'infra-failure-precedes-unknown-dependent-job') {
      const counts = result.aggregate.metrics.ci_first_attempt.counts_by_conclusion;
      const jobCounts = result.aggregate.metrics.ci_first_attempt.job_counts_by_conclusion;
      assert.equal(counts.infra_failure, 1, 'infrastructure failure remains visible beside an unknown dependent job');
      assert.equal(counts.unknown, 1);
      assert.equal(jobCounts.infra_failure, 1);
      assert.equal(jobCounts.unknown, 2, 'both job outcomes remain visible');
    }
    if (testCase.name === 'all-excluded-ci-outcomes-have-no-fraction') {
      assert.equal(result.aggregate.metrics.ci_first_attempt.denominator, 0);
      assert.equal(result.aggregate.metrics.ci_first_attempt.fraction, null);
    }
    if (testCase.name === 'complete-zero-release-cohort-is-measured-sparse-zero') {
      assert.equal(result.aggregate.metrics.confirmed_regressions.status, 'measured');
      assert.equal(result.aggregate.metrics.confirmed_regressions.eligible, 0);
      assert.equal(result.aggregate.metrics.confirmed_regressions.regression_count, 0);
      assert.equal(result.aggregate.metrics.confirmed_regressions.sparse, true);
    }
    if (testCase.name === 'complete-declared-inventory-can-record-zero-coverage-with-no-emitted-files') {
      assert.equal(result.aggregate.metrics.coverage.numerator, 0);
      assert.equal(result.aggregate.metrics.coverage.denominator, positive.metrics.coverage.payload.eligible);
    }
  } else if (testCase.expected === 'schema') {
    assert.equal(result.valid, false, `${testCase.name} should fail schema validation`);
    assert.ok(result.schemaErrors.length > 0, `${testCase.name} must be classified as a schema failure`);
    assert.deepEqual(result.semanticErrors, [], `${testCase.name} should stop before semantic validation`);
  } else {
    assert.equal(testCase.expected, 'semantic');
    assert.equal(result.valid, false, `${testCase.name} should fail semantic validation`);
    assert.deepEqual(result.schemaErrors, [], `${testCase.name} must pass the shape schema`);
    assert.ok(result.semanticErrors.length > 0, `${testCase.name} must be classified as a semantic failure`);
  }
}

const duplicateSeries = validateObjectiveSeries(schema, [positive, structuredClone(positive)]);
assert.equal(duplicateSeries.valid, true, 'exact snapshot replay is idempotent');
assert.equal(duplicateSeries.records.length, 1);
assert.equal(duplicateSeries.replayCount, 1);
assert.equal(validateObjectiveSeries(schema, [null]).valid, false);
assert.equal(validateObjectiveSeries(schema, null).valid, false);
const selfSupersedes = structuredClone(positive);
selfSupersedes.supersedes_ref = selfSupersedes.snapshot_id;
assert.ok(validateObjective(schema, selfSupersedes).semanticErrors.some(error => error.includes('cannot refer to this snapshot')));

console.log(`validated ${casesFixture.cases.length + 1} objective fixtures and derived aggregates`);

const partialInventory = structuredClone(positive);
partialInventory.metrics.ci_first_attempt.payload.expected_run_count += 1;
assert.equal(validateObjective(schema, partialInventory).valid, false, 'omitted expected run cannot inflate reliability');
const futureWindow = structuredClone(positive);
futureWindow.metrics.ci_first_attempt.payload.window_end = '2026-10-30T00:00:00Z';
assert.equal(validateObjective(schema, futureWindow).valid, false, 'future CI window is not a historical cohort');
const collision = structuredClone(positive);
collision.commit = 'd'.repeat(40);
assert.equal(validateObjectiveSeries(schema, [positive, collision]).valid, false, 'same snapshot ID different payload is rejected');
const danglingCorrection = structuredClone(positive);
danglingCorrection.snapshot_id = 'ref:successor';
danglingCorrection.supersedes_ref = 'ref:missing';
danglingCorrection.correction_ref = 'ref:correction-evidence';
assert.equal(validateObjectiveSeries(schema, [danglingCorrection]).valid, false, 'correction requires its target in context');
const correction = structuredClone(positive);
correction.snapshot_id = 'ref:successor';
correction.supersedes_ref = positive.snapshot_id;
correction.correction_ref = 'ref:correction-evidence';
assert.equal(validateObjectiveSeries(schema, [positive, correction]).valid, true);
const missingCorrectionEvidence = structuredClone(positive);
delete missingCorrectionEvidence.correction_ref;
assert.equal(validateObjective(schema, missingCorrectionEvidence).valid, false, 'root correction evidence is required');
const absentCorrectionOnSuccessor = structuredClone(correction);
absentCorrectionOnSuccessor.correction_ref = null;
assert.equal(validateObjective(schema, absentCorrectionOnSuccessor).valid, false, 'superseding snapshots require correction evidence');
const unpairedCorrectionEvidence = structuredClone(positive);
unpairedCorrectionEvidence.correction_ref = 'ref:orphan-correction-evidence';
assert.equal(validateObjective(schema, unpairedCorrectionEvidence).valid, false, 'non-superseding snapshots cannot claim correction evidence');
const missingSlot = structuredClone(positive);
delete missingSlot.metrics.coverage.slot_ref;
assert.equal(validateObjective(schema, missingSlot).valid, false, 'unknown provenance still needs stable expected-slot identity');

const cycleStart = structuredClone(positive);
cycleStart.supersedes_ref = correction.snapshot_id;
cycleStart.correction_ref = 'ref:cycle-evidence';
assert.equal(validateObjectiveSeries(schema, [cycleStart, correction]).valid, false, 'correction cycles are rejected');
assert.equal(validateObjectiveSeries(schema, [correction], { context: [positive] }).valid, true, 'explicit context resolves incremental correction');
const wrongRepoCorrection = structuredClone(correction);
wrongRepoCorrection.repository.name = 'other-repo';
assert.equal(validateObjectiveSeries(schema, [wrongRepoCorrection], { context: [positive] }).valid, false, 'corrections cannot retarget repository');
