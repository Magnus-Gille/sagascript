// Reference-only pilot presentation policy; never a mutation or collection gate.
import { canonical, validateSchema } from './code-health-schema.mjs';
import { validateObjective } from './code-health-objective.mjs';

function slot(schema, snapshot, name) {
  if (!validateObjective(schema, snapshot).valid) throw new TypeError('invalid objective snapshot');
  if (!Object.hasOwn(snapshot.metrics, name)) throw new TypeError('unknown metric');
  return snapshot.metrics[name];
}

export function currentMetricStatus(schema, snapshot, name, asOf) {
  const metric = slot(schema, snapshot, name);
  if (validateSchema({ type: 'string', format: 'date-time' }, asOf).length) throw new TypeError('invalid as-of time');
  if (metric.status !== 'measured') return metric.status;
  const age = Date.parse(asOf) - Date.parse(metric.observed_at);
  if (age < 0) return 'unknown';
  const cap = (name === 'ci_first_attempt' ? 2 : 10) * 86400000;
  return age > cap ? 'stale' : 'measured';
}

export function seriesKey(schema, snapshot, name) {
  const metric = slot(schema, snapshot, name);
  if (metric.status !== 'measured') return null;
  const { run_ref: _run, attempt: _attempt, ...sourcePolicy } = metric.source;
  sourcePolicy.feature_refs = [...sourcePolicy.feature_refs].sort();
  const p = metric.payload;
  const semantics = {
    complex_functions: { algorithm: p.algorithm, threshold: p.threshold },
    unused_candidates: { graph_status: p.graph_status },
    coverage: { measure: p.measure, includes_inline_tests: p.includes_inline_tests, source_inventory: p.source_inventory },
    ci_first_attempt: { branch: p.branch, event: p.event, workflow_ref: p.workflow_ref,
      workflow_config_digest: p.workflow_config_digest, expected_job_refs: p.expected_job_refs ? [...p.expected_job_refs].sort() : undefined },
    confirmed_regressions: { cohort_policy: p.cohort_policy, observation_days: 14 }
  }[name];
  return canonical({ contract_version: snapshot.contract_version, repository: snapshot.repository,
    metric: name, slot_ref: metric.slot_ref, unit: metric.unit, sourcePolicy, semantics });
}
