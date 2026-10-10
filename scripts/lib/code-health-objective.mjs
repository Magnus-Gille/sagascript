import { canonical, validateSchema } from './code-health-schema.mjs';

const STATUSES = ['measured', 'unknown', 'unsupported', 'failed', 'stale'];
const CI_CONCLUSIONS = ['success', 'failure', 'infra_failure', 'cancelled', 'skipped', 'pending', 'unknown'];
const CI_PRIORITY = ['failure', 'infra_failure', 'unknown', 'pending', 'cancelled', 'skipped', 'success'];
const METRIC_NAMES = ['complex_functions', 'unused_candidates', 'coverage', 'ci_first_attempt', 'confirmed_regressions'];

const isMeasured = metric => metric.status === 'measured';
const inWindow = (stamp, start, end) => Date.parse(stamp) >= Date.parse(start) && Date.parse(stamp) < Date.parse(end);

function semanticErrorsFor(objective) {
  const errors = [];
  const fail = (path, message) => errors.push(`${path}: ${message}`);
  const { metrics, snapshot_id: snapshotId, supersedes_ref: supersedesRef, correction_ref: correctionRef } = objective;

  if (supersedesRef === snapshotId) fail('$.supersedes_ref', 'cannot refer to this snapshot');
  if ((supersedesRef === null) !== (correctionRef === null)) {
    fail('$.correction_ref', 'must be present exactly when supersedes_ref is present');
  }
  for (const name of METRIC_NAMES) {
    const metric = metrics[name];
    if (Date.parse(metric.observed_at) > Date.parse(objective.observed_at)) {
      fail(`$.metrics.${name}.observed_at`, 'cannot be later than the objective observed_at timestamp');
    }
    if (metric.status !== 'measured' && metric.prior_snapshot_ref === snapshotId) {
      fail(`$.metrics.${name}.prior_snapshot_ref`, 'cannot refer to this snapshot');
    }
    if (metric.population !== null) {
      const included = new Set(metric.population.included_refs);
      for (const ref of metric.population.excluded_refs) {
        if (included.has(ref)) fail(`$.metrics.${name}.population.excluded_refs`, `reference ${ref} is also included`);
      }
    }
  }

  const complex = metrics.complex_functions;
  if (isMeasured(complex) && complex.payload.above_threshold_functions > complex.payload.eligible_functions) {
    fail('$.metrics.complex_functions.payload.above_threshold_functions', 'cannot exceed eligible_functions');
  }

  const unused = metrics.unused_candidates;
  if (isMeasured(unused)) {
    const p = unused.payload;
    if (BigInt(p.validated_count) + BigInt(p.false_positive_count) + BigInt(p.uncertain_count) !== BigInt(p.candidate_count)) {
      fail('$.metrics.unused_candidates.payload', 'candidate_count must equal the validated, false-positive, and uncertain partition');
    }
    if (p.validated_count > 0 && p.owner_validation_refs.length === 0) {
      fail('$.metrics.unused_candidates.payload.owner_validation_refs', 'validated candidates require owning-repository evidence references');
    }
    if (p.graph_status === 'qualified' && p.graph_qualification_ref === null) {
      fail('$.metrics.unused_candidates.payload.graph_qualification_ref', 'a qualified graph requires its qualification evidence reference');
    }
  }

  const coverage = metrics.coverage;
  if (isMeasured(coverage)) {
    const p = coverage.payload;
    if (coverage.unit !== p.measure) fail('$.metrics.coverage.unit', 'must equal payload.measure');
    if (p.eligible < 1) fail('$.metrics.coverage.payload.eligible', 'measured coverage requires at least one eligible item');
    if (p.covered > p.eligible) fail('$.metrics.coverage.payload.covered', 'cannot exceed eligible');
    if (p.covered > 0 && p.emitted_source_files === 0) fail('$.metrics.coverage.payload.emitted_source_files', 'positive coverage requires at least one emitted source file');
    if (p.eligible_source_files < 1) fail('$.metrics.coverage.payload.eligible_source_files', 'measured coverage requires at least one eligible source file');
    if (p.emitted_source_files > p.eligible_source_files) fail('$.metrics.coverage.payload.emitted_source_files', 'cannot exceed eligible_source_files');
    if (p.source_inventory === 'exported-only' && p.eligible > 0 && p.emitted_source_files === 0) {
      fail('$.metrics.coverage.payload.emitted_source_files', 'exported-only inventory requires emitted source files for a positive eligible denominator');
    }
  }

  const ciMetric = metrics.ci_first_attempt;
  if (isMeasured(ciMetric)) {
    const p = ciMetric.payload;
    if (Date.parse(p.window_start) >= Date.parse(p.window_end)) {
      fail('$.metrics.ci_first_attempt.payload.window_end', 'must be later than window_start');
    }
    if (p.expected_run_count !== p.runs.length) fail('$.metrics.ci_first_attempt.payload.expected_run_count', 'must equal the complete expected run inventory; represent missing attempts as unknown');
    if (Date.parse(p.window_end) > Date.parse(ciMetric.observed_at)) fail('$.metrics.ci_first_attempt.payload.window_end', 'cannot be after collection');
    if (Date.parse(p.window_end) - Date.parse(p.window_start) !== 28 * 86400000) fail('$.metrics.ci_first_attempt.payload.window_start', 'pilot cohort must span 28 days');
    const runRefs = new Set();
    p.runs.forEach((run, index) => {
      const path = `$.metrics.ci_first_attempt.payload.runs[${index}]`;
      if (runRefs.has(run.run_ref)) fail(`${path}.run_ref`, `duplicate run reference ${run.run_ref}`);
      runRefs.add(run.run_ref);
      if (!inWindow(run.created_at, p.window_start, p.window_end)) fail(`${path}.created_at`, 'must fall within the half-open observation window');
      const jobRefs = run.jobs.map(job => job.job_ref);
      if (new Set(jobRefs).size !== jobRefs.length) fail(`${path}.jobs`, 'job references must be unique within a run');
      const expected = [...p.expected_job_refs].sort();
      const actual = [...jobRefs].sort();
      if (canonical(expected) !== canonical(actual)) fail(`${path}.jobs`, 'must contain every expected job reference exactly once; use unknown for missing evidence');
      const derived = CI_PRIORITY.find(conclusion => run.jobs.some(job => job.conclusion === conclusion));
      if (run.overall_conclusion !== derived) fail(`${path}.overall_conclusion`, `must be derived as ${derived}`);
    });
  }

  const regressions = metrics.confirmed_regressions;
  if (isMeasured(regressions)) {
    const p = regressions.payload;
    const path = '$.metrics.confirmed_regressions.payload';
    if (p.as_of !== regressions.observed_at) fail(`${path}.as_of`, 'must equal this metric slot’s observed_at timestamp');
    if (Date.parse(p.cohort_start) >= Date.parse(p.cohort_end)) {
      fail(`${path}.cohort_end`, 'must be later than cohort_start');
    }
    if (Date.parse(p.as_of) < Date.parse(p.cohort_end)) fail(`${path}.as_of`, 'must be on or after the closed cohort end');
    if (p.eligible_releases !== p.releases.length) {
      fail(`${path}.eligible_releases`, 'must equal the complete published release inventory length');
    }
    const releaseByRef = new Map();
    p.releases.forEach((release, index) => {
      const releasePath = `${path}.releases[${index}]`;
      if (releaseByRef.has(release.release_ref)) fail(`${releasePath}.release_ref`, 'duplicate release reference');
      releaseByRef.set(release.release_ref, release);
      if (!inWindow(release.published_at, p.cohort_start, p.cohort_end)) {
        fail(`${releasePath}.published_at`, 'must fall within the declared half-open release cohort');
      }
      const expectedEnd = Date.parse(release.published_at) + 14 * 24 * 60 * 60 * 1000;
      if (Date.parse(release.window_end) !== expectedEnd) fail(`${releasePath}.window_end`, 'must be exactly 14 days after publication');
      if (Date.parse(release.window_end) > Date.parse(p.as_of)) fail(`${releasePath}.window_end`, 'the complete release window must have ended by as_of');
    });
    const seenIssues = new Set();
    p.regressions.forEach((item, index) => {
      const itemPath = `${path}.regressions[${index}]`;
      const release = releaseByRef.get(item.release_ref);
      if (!release) fail(`${itemPath}.release_ref`, 'must match a release with a complete observed window');
      if (item.known_good_release_ref === item.release_ref) fail(`${itemPath}.known_good_release_ref`, 'must differ from the affected release');
      if (release && !inWindow(item.first_observed_at, release.published_at, release.window_end)) {
        fail(`${itemPath}.first_observed_at`, 'must fall within the introducing release’s half-open 14-day observation window');
      }
      if (seenIssues.has(item.issue_ref)) fail(`${itemPath}.issue_ref`, 'an issue may be counted only once in the release cohort');
      seenIssues.add(item.issue_ref);
    });
  }
  return errors;
}

function fraction(numerator, denominator) {
  return denominator === 0 ? null : numerator / denominator;
}

function aggregateObjective(objective) {
  const metrics = objective.metrics;
  const stateCounts = Object.fromEntries(STATUSES.map(status => [status, 0]));
  for (const name of METRIC_NAMES) stateCounts[metrics[name].status] += 1;
  const aggregateMetric = (name, derive) => {
    const metric = metrics[name];
    const provenance = {
      slot_ref: metric.slot_ref,
      observed_at: metric.observed_at,
      source: metric.source,
      population: metric.population,
      unit: metric.unit
    };
    if (!isMeasured(metric)) {
      return { ...provenance, status: metric.status, reason: metric.reason, prior_snapshot_ref: metric.prior_snapshot_ref ?? null };
    }
    return { ...provenance, status: 'measured', ...derive(metric.payload) };
  };
  const ciCounts = Object.fromEntries(CI_CONCLUSIONS.map(state => [state, 0]));
  const ciJobCounts = Object.fromEntries(CI_CONCLUSIONS.map(state => [state, 0]));
  const ci = metrics.ci_first_attempt;
  if (isMeasured(ci)) for (const run of ci.payload.runs) {
    ciCounts[run.overall_conclusion] += 1;
    for (const job of run.jobs) ciJobCounts[job.conclusion] += 1;
  }
  const ciDenominator = ciCounts.success + ciCounts.failure + ciCounts.infra_failure;
  const severityCounts = { critical: 0, major: 0, minor: 0 };
  const regressions = metrics.confirmed_regressions;
  if (isMeasured(regressions)) for (const item of regressions.payload.regressions) severityCounts[item.severity] += 1;

  return {
    contract_version: objective.contract_version,
    snapshot_id: objective.snapshot_id,
    supersedes_ref: objective.supersedes_ref,
    correction_ref: objective.correction_ref,
    repository: objective.repository,
    commit: objective.commit,
    observed_at: objective.observed_at,
    state_counts: stateCounts,
    metrics: {
      complex_functions: aggregateMetric('complex_functions', p => ({
        algorithm: p.algorithm, threshold: p.threshold, eligible: p.eligible_functions,
        numerator: p.above_threshold_functions, denominator: p.eligible_functions,
        fraction: fraction(p.above_threshold_functions, p.eligible_functions)
      })),
      unused_candidates: aggregateMetric('unused_candidates', p => ({
        graph_status: p.graph_status, eligible: p.candidate_count, candidate_count: p.candidate_count,
        validated_count: p.validated_count, false_positive_count: p.false_positive_count,
        uncertain_count: p.uncertain_count
      })),
      coverage: aggregateMetric('coverage', p => ({
        measure: p.measure, eligible: p.eligible, numerator: p.covered, denominator: p.eligible,
        fraction: fraction(p.covered, p.eligible), emitted_source_files: p.emitted_source_files,
        eligible_source_files: p.eligible_source_files, profile_ref: p.profile_refs[0],
        includes_inline_tests: p.includes_inline_tests, source_inventory: p.source_inventory
      })),
      ci_first_attempt: aggregateMetric('ci_first_attempt', p => ({
        eligible: ciDenominator, numerator: ciCounts.success, denominator: ciDenominator,
        fraction: fraction(ciCounts.success, ciDenominator), counts_by_conclusion: ciCounts,
        job_counts_by_conclusion: ciJobCounts,
        expected_runs: p.expected_run_count, sparse: ciDenominator < 20,
        window_start: p.window_start, window_end: p.window_end
      })),
      confirmed_regressions: aggregateMetric('confirmed_regressions', p => ({
        eligible: p.eligible_releases, regression_count: p.regressions.length,
        severity_counts: severityCounts, sparse: p.eligible_releases < 5,
        cohort_start: p.cohort_start,
        cohort_end: p.cohort_end, as_of: p.as_of, cohort_policy: p.cohort_policy
      }))
    }
  };
}

export function validateObjective(schema, objective) {
  const schemaErrors = validateSchema(schema, objective);
  if (schemaErrors.length) return { valid: false, schemaErrors, semanticErrors: [], aggregate: null };
  const semanticErrors = semanticErrorsFor(objective);
  return {
    valid: semanticErrors.length === 0,
    schemaErrors: [],
    semanticErrors,
    aggregate: semanticErrors.length === 0 ? aggregateObjective(objective) : null
  };
}

// This is batch admission, not permission to join incompatible metric series.
// Pass source records as context when validating incremental corrections.
export function validateObjectiveSeries(schema, objectives, { context = [] } = {}) {
  if (!Array.isArray(objectives) || !Array.isArray(context)) return { valid: false, errors: ['$: expected arrays'], records: [], replayCount: 0 };
  const errors = [];
  const byId = new Map();
  const records = [];
  const incomingIds = new Set();
  let replayCount = 0;
  for (const objective of [...context, ...objectives]) {
    const result = validateObjective(schema, objective);
    if (!result.valid) { errors.push(...result.schemaErrors, ...result.semanticErrors); continue; }
    const prior = byId.get(objective.snapshot_id);
    if (prior && canonical(prior) !== canonical(objective)) errors.push('snapshot ID collision');
    else byId.set(objective.snapshot_id, objective);
  }
  for (const objective of objectives) {
    if (!objective || !byId.has(objective.snapshot_id)) continue;
    if (incomingIds.has(objective.snapshot_id)) { replayCount++; continue; }
    incomingIds.add(objective.snapshot_id);
    records.push(validateObjective(schema, objective));
  }
  for (const objective of byId.values()) {
    const seen = new Set([objective.snapshot_id]);
    let next = objective;
    while (next.supersedes_ref !== null) {
      if (seen.has(next.supersedes_ref)) { errors.push('snapshot correction cycle'); break; }
      seen.add(next.supersedes_ref);
      const target = byId.get(next.supersedes_ref);
      if (!target) { errors.push('snapshot correction target missing from context'); break; }
      if (canonical(target.repository) !== canonical(next.repository) || target.commit !== next.commit)
        errors.push('snapshot correction must preserve repository and commit');
      if (Date.parse(next.observed_at) < Date.parse(target.observed_at)) errors.push('snapshot correction predates target');
      next = target;
    }
  }
  return { valid: errors.length === 0, errors, records, replayCount };
}
