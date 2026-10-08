# Offline diarization evaluation CLI

The CLI provides bounded, offline inspection and scoring for native diarization
artifacts. It never opens audio, loads a model, reads Sagascript settings, or
writes an output file; results are written to stdout.

```text
sagascript diarization reference-validate REFERENCE.json
sagascript diarization reference-export REFERENCE.json --format rttm
sagascript diarization inspect REPORT.json
sagascript diarization export-activity REPORT.json --format json
sagascript diarization evaluate --reference REFERENCE.json --hypothesis REPORT.json
sagascript diarization evaluate --reference REFERENCE.rttm --uem RECORDING.uem \
  --hypothesis REPORT.json --collar 0.25 --layer acoustic
```

Native reference JSON is validated by the core reference contract. Exported
RTTM and UEM contain only verified reference intervals; validation does not
declare a reference gold. RTTM input is accepted for measurement, but requires
an explicit UEM so unreviewed time cannot become a scoring domain. RTTM and UEM
must contain one recording ID, and their timestamps must be finite,
non-negative, and within the reference duration.

The hypothesis may be a native `DiarizationReport` or a native
`MeetingTranscript`. Acoustic scoring uses report activity. A meeting JSON may
carry an embedded `diarization` report; otherwise the command reports that the
acoustic report is missing and suggests `--layer transcript`. Transcript
scoring uses immutable meeting segments and refuses corrected or modified
transcripts. Legacy transcripts without the immutable provenance marker are
reported as measurement-only.

Every evaluation receipt is schema version 1 and includes the exact SHA-256 of
the reference, hypothesis, and optional UEM files, the media source hash when
available, build identity, layer, collar, core metrics, and explicit reasons
why `quality_adoption_ready` is `false`. The receipt is a measurement artifact;
the caller owns any later quality-adoption policy.

Inputs are limited to 24 MiB and must be valid UTF-8 for text formats. Invalid
JSON, malformed records, duplicate RTTM intervals, multiple recording IDs,
overlapping UEM regions, and out-of-bounds timestamps fail before scoring.
