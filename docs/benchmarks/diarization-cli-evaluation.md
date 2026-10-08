# Diarization reference qualification and evaluation

The CLI keeps annotation qualification separate from model scoring. A reviewer first makes a
blind pass over supplied windows and records `verified`, `unknown`, or `candidate` intervals with
human evidence. When available, a second reviewer checks a random sample of at least 20% of the
windows. The reference manifest freezes the source hash, policy identity, split identity, and
window boundaries.

Evaluation receipts include uncollared short-reference-region and boundary measurements under
`strata`. They reuse the global DER speaker mapping and original UEM; the explicit
`strata_collar_seconds: 0.0` distinguishes them from the requested global DER collar. See
[the measurement definitions](diarization-strata.md).

## Independent metric cross-check

`scripts/diarization_sv/crosscheck_metrics.py` checks eleven synthetic cases at collars 0 and
0.25 seconds, including silence false alarms, overlapping speakers, identity switches, UEM
holes and a case where the DER and JER assignments differ. It calls only Sagascript's offline
evaluation command, pyannote.metrics' DER scorer, and the inspected numeric JER function from
[dscore](https://github.com/nryant/dscore/blob/e02f949ac6592279300a2c33d03daf9e0c12fd27/scorelib/metrics.py).
It never runs an external diarization model. The dscore file must match its pinned SHA-256.

In an environment with NumPy, SciPy and pyannote.metrics installed, run:

```sh
python scripts/diarization_sv/crosscheck_metrics.py --binary /path/to/sagascript \
  --dscore-source /path/to/pinned/metrics.py --scratch /path/to/new/check-output
```

The native collar is a half-width around each reference boundary; pyannote.metrics uses the
whole width, so the harness passes twice the native value to that scorer. JER uses original UEM
without a collar and its own IoU-optimal assignment. Synthetic comparisons use a tolerance of
1e-8 for fractions and seconds. Undefined metrics with zero reference speaker-time are kept
as null by Sagascript, rather than being treated as a passing quality target.

Use `sagascript diarization reference-identity reference.json` to obtain the deterministic
reference hash, then freeze that value in the manifest. Run:

```text
sagascript diarization reference-qualify reference.json --manifest split.json --minimum-coverage 0.9
```

The machine-readable report includes readiness, failure codes, identities, and reviewed,
verified, unknown, and candidate coverage by split, stratum, and speaker. A reference is eligible
for quality adoption only when all windows are completely partitioned, unknown intervals have
human reviewer, timestamp, and evidence, no candidate interval remains, every known speaker and
declared stratum appears in eval, and eval contains at least 90% human-identified speech. A
qualification report is a gate for the frozen sample; it is not a claim of full-film accuracy.

To score a frozen split, pass both `--manifest split.json` and `--split dev` or `--split eval` to
`diarization evaluate`. The command rejects stale or unqualified manifests, selects only verified
intervals in the requested frozen windows, and refuses an external `--uem` that could bypass the
reference mask. The default metric targets are DER ≤ 0.10 and confusion ≤ 0.05, expressed as
fractions of reference speaker-time; callers can bind exact alternatives with
`--maximum-der` and `--maximum-confusion`. The receipt records the observed fractions, thresholds,
and `--collar` value in seconds. A zero-reference-speaker evaluation cannot pass metric targets.
Metric results remain measurement output; `quality_adoption_ready` stays false pending the frozen
public regression suite and held-out decision gate.
