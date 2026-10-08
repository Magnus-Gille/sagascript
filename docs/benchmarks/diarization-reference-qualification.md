# Diarization reference qualification

The reference JSON remains a small annotation document with `source_sha256`,
`duration_seconds`, known `speakers`, optional non-overlapping `windows`, and
`intervals`. Candidate and unknown intervals are annotation context. Only an
interval with a named human reviewer, review timestamp, and reviewer evidence
can become `verified`; model output or consensus is never gold by itself.

Qualification uses a separate frozen manifest. It records the exact
`reference_id`, source hash, frozen policy `{id, version, frozen: true}`,
`split_id`, and one assignment for every reference window:

```json
{
  "reference_id": "sv-reference-v1",
  "source_sha256": "…",
  "policy": {"id": "human-review-v1", "version": "1", "frozen": true},
  "split_id": "sv-split-v1",
  "frozen": true,
  "windows": [
    {"id": "clip-01", "start": 0, "end": 20, "split": "train", "stratum": "ordinary"},
    {"id": "clip-02", "start": 20, "end": 40, "split": "dev", "stratum": "difficult"},
    {"id": "clip-03", "start": 40, "end": 60, "split": "eval", "stratum": "difficult"}
  ]
}
```

Create deterministic assignments with `split`, then qualify without replacing
an existing report:

```sh
python3 scripts/diarization_sv/reference_dataset.py split reference.json \
  --output split.json --seed sv-reference-v1 --train 0.6 --dev 0.2 --eval 0.2
python3 scripts/diarization_sv/reference_dataset.py qualify reference.json \
  --split qualification.json --output qualification-report.json \
  --policy-id human-review-v1
```

The report carries identities for the reference, source, frozen policy, and
frozen split. It reports reviewed coverage, human-identified speech seconds,
verified speaker-time and verified time, unknown and candidate speech
exclusions, and coverage by `train`/`dev`/`eval`, supplied stratum, and known
speaker. Readiness is false for a source hash mismatch, stale policy or split,
split leakage, zero verified intervals, missing frozen dev/eval separation, low
eval coverage, a missing eval speaker, or a missing difficult/present eval
stratum. Gold qualification requires at least 90% eval reviewed speech
coverage after explicit unknown and candidate exclusions, every known voice in
eval, and each difficult stratum represented.

Human review starts with a blind first pass over the selected windows. A second
reviewer checks a 20% sample when available; disagreements remain explicit and
are resolved by a named human. Reviewers should report sampled coverage and
uncertainty, rather than claiming full-film accuracy from a partial review.
Keep silence in UEM, retain unknown holes, and represent overlap as one
multi-speaker interval (exported as one RTTM turn per speaker). Candidate
intervals are never exported, and all exports and reports refuse overwrite.
