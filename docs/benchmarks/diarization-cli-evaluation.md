# Diarization reference qualification and evaluation

The CLI keeps annotation qualification separate from model scoring. A reviewer first makes a
blind pass over supplied windows and records `verified`, `unknown`, or `candidate` intervals with
human evidence. When available, a second reviewer checks a random sample of at least 20% of the
windows. The reference manifest freezes the source hash, policy identity, split identity, and
window boundaries.

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
reference mask. Metric results remain measurement output; `quality_adoption_ready` stays false
until the public regression suite and its target policy are established.
