# Diarization roadmap implementation

This change supplies the local CLI and evidence needed to work through [#329](https://github.com/Magnus-Gille/sagascript/issues/329).
It does not declare the private meeting reference qualified or adopt an experimental clustering policy.

| Issue | Implemented engineering | Remaining acceptance |
| --- | --- | --- |
| #322 | Blind audiovisual review tool, strict reference importer, frozen split manifest and native qualification command | Human review, complete partitions, voice/stratum representation and qualified held-out coverage |
| #323 | Native offline scoring with explicit UEM, separate acoustic/transcript layers, DER components, JER, overlap and per-window reports | Qualified private comparison, independent scorer checks and frozen public regression decision |
| #324 | Effective greedy/beam and VAD configuration, cache invalidation, saved decoder provenance | Calibration comparison and latency acceptance before choosing a different preset |
| #325 | Versioned acoustic activity report; opt-in region, embedding, attribution and ASR evidence | Diagnose failures against human truth; distance/support is evidence, not calibrated confidence |
| #326 | Default-off exclusive speech extraction for embeddings; activity support and explicit missing embedding evidence | Ablation and qualified quality/performance gates; no experimental default adoption |
| #327 | Aggregate union support per identity for word attribution; explicit ties, fallback gaps and invalid timing; original acoustic output survives transcript edits | Human word/boundary comparison before choosing continuity or bounded fallback policy |
| #328 | Native simultaneous speaker activity, RTTM export and review display without duplicated words | Verified overlap comparison; text exports still carry one speaker per transcript segment |

## Local CLI workflow

Produce a native meeting document with the shipped default threshold and no count hint:

```sh
sagascript transcribe recording.mp4 --language sv --model kb-whisper-medium \
  --diarize --beam 0 --no-vad --diarize-cache analysis.json \
  --meeting-json > meeting.json
sagascript diarization inspect meeting.json
sagascript diarization export-activity meeting.json --format rttm > activity.rttm
```

Add `--diarize-diagnostics` when detailed evidence is needed. The ordinary report retains the
acoustic activity, source identity, models and effective configuration, while avoiding the larger
per-region/per-word arrays. Detailed evidence does not copy transcript text or raw voice vectors.
`--diarize-exclusive-speech-embeddings` enables the experimental extraction policy and invalidates
both analysis and transcript cache entries. It is off by default.

Diarized decoding uses greedy search (`--beam 0`) unless beam search is explicitly selected or a
compatible saved beam setting applies. Beam 1 is rejected; supported beam search sizes are 2–8.
VAD is now effective. Diarized VAD results use source-mapped segment timestamps, which are coarser
than word DTW timestamps; compare text changes separately from speaker assignment changes.

Freeze an independently reviewed reference and its split before selecting settings:

```sh
sagascript diarization reference-validate reference.json
sagascript diarization reference-identity reference.json
sagascript diarization reference-qualify reference.json --manifest split.json
sagascript diarization evaluate --reference reference.json --hypothesis meeting.json \
  --manifest split.json --split dev --layer acoustic --collar 0.25 > dev-acoustic.json
sagascript diarization evaluate --reference reference.json --hypothesis meeting.json \
  --manifest split.json --split dev --layer transcript --collar 0 > dev-transcript.json
```

Only after a calibration decision is frozen should the same configuration be applied to `--split eval`.
Candidate and unknown intervals never become truth. Qualification requires human evidence and
rejects stale identities, split leakage, incomplete partitions and insufficient reviewed coverage.
An explicit external UEM cannot override the qualified native reference mask.

RTTM comparisons require `--uem regions.uem`. Those measurements lack the native reference
qualification contract. UEM should include verified silence to expose false speech; a historical
reference-speech-only mask produces a different metric and must be labelled as such.

## Output and preservation

Native meetings containing acoustic evidence use schema 2; legacy schema 1 documents remain
readable. The report binds the source hash and duration. Renaming or merging human-visible
speakers does not rewrite the original acoustic speaker IDs. Transcript edits mark provenance:
acoustic evaluation can still use the original timeline, while transcript evaluation rejects a
document marked as modified. This marker is local provenance, not a cryptographic attestation.

Plain text, Markdown, SRT and VTT retain one speaker for each transcript segment and cannot fully
express concurrent acoustic speakers. Use the native report or RTTM activity export to preserve
overlap. Review shows concurrent IDs separately and does not duplicate transcript words.

Nearest-gap attribution retains its legacy behavior while exposing the gap. Tied support and
invalid timestamps remain explicit diagnostic reasons. No continuity smoothing, count forcing,
new embedding model or global threshold change is adopted without the roadmap's reference and
regression gates. Metric target success alone does not authorize quality adoption.
