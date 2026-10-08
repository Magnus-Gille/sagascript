# Diarization reference qualification

The audiovisual review file can retain richer evidence annotations. Before freezing a split,
export its validated truth fields to the strict CLI format:

```sh
python scripts/diarization_sv/reference_dataset.py export-native review.json > reference.json
sagascript diarization reference-validate reference.json
sagascript diarization reference-identity reference.json
```

Use a new output path and preserve `review.json` with its evidence artifacts. The export keeps
source, windows, intervals, speaker IDs, review provenance and candidate/unknown status, but
omits the derived summary and extra audiovisual evidence annotations. Native reference identity
binds those exported fields; review-only metadata is retained in the original and does not turn
an automated proposal into truth. Freeze the manifest after this export and human review.

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
  "reference_sha256": "<sha256 of normalized native truth fields>",
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
  --split split.json --output qualification-report.json \
  --policy-id human-review-v1
```

The report carries identities for the reference, source, frozen policy, and
frozen split. It reports reviewed coverage, human-identified speech seconds,
verified speaker-time and verified time, unknown and candidate speech
exclusions, and coverage by `train`/`dev`/`eval`, supplied stratum, and known
speaker. Readiness is false for a source hash mismatch, stale policy or split,
split leakage, zero verified intervals, missing frozen dev/eval separation, low
eval coverage, a missing eval speaker, a missing supplied eval stratum, or an
incomplete/overlapping window partition. Gold qualification requires at least
90% eval reviewed speech
coverage after explicit unknown and candidate exclusions, every known voice in
eval, every supplied stratum represented in eval, and complete disjoint window
partitions. Candidate intervals prevent readiness; unknown intervals must carry
human reviewer, timestamp, and evidence to count as reviewed.

Human review starts with a blind first pass over the selected windows. A second
reviewer checks a 20% sample when available; disagreements remain explicit and
are resolved by a named human. Reviewers should report sampled coverage and
uncertainty, rather than claiming full-film accuracy from a partial review.
Keep silence in UEM, retain unknown holes, and represent overlap as one
multi-speaker interval (exported as one RTTM turn per speaker). Candidate
intervals are never exported to RTTM. File-writing commands refuse overwrite;
`export-native` writes JSON to stdout.
