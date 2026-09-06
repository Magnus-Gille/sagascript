# Live dictation latency measurement (#163)

## Decision and boundaries

Add an offline, CLI-first `sagascript latency-report --input <copied.jsonl>`
reporter for the existing `dictation_phase_timings` events. It does not start
the app, capture audio, load models, inspect the default log directory, change
settings, or contact a remote service. Input must be selected explicitly.
Keep private recordings, transcripts and real benchmark logs outside Git.

The timer starts when the app enters its stop handler: push-to-talk key-up or
the second toggle press. OS/input dispatch before that handler is not measured.
The measured endpoint is **paste-call completion, not visibly rendered text**.
Neither this report nor CLI transcription throughput proves physical-key-up-
to-visible-text latency. Signed-app editor observation remains a separate gate.
No decoder/model defaults change without measured accuracy evidence.

The JSON report has schema version 1 and emits only fixed metric names,
validated configuration identifiers, numeric aggregates and fixed labels.
`reporterVersion` identifies the reporting executable, including its build ID.
Legacy phase events do not identify the source app build, so `sourceBuild` is
null: never mistake the reporter version for the measured app version. Keep
datasets from different app builds separate and record the tested app's
`sagascript --version` beside each private dataset before comparing releases.
It never echoes raw rows, transcripts, paths, session IDs, device names,
arbitrary metadata or invalid values in errors. Errors identify the line and
field, not the rejected value. Unknown events are ignored after valid JSON
parsing; malformed JSON or malformed relevant events fail the report.
Every input line must contain a JSON record: blank or whitespace-only lines are
malformed. Relevant `capture_stopped` and `dictation_phase_timings` records
must contain both correlation identifiers, `appSession` and
`dictationSession`; missing either identifier fails closed. An empty file is a
valid empty report, but it cannot pass an explicit budget because it has no
eligible samples.

## Cohorts and statistics

Never combine different model, language, warm state, beam size, temperature
fallback, VAD setting, outcome or paste outcome. Normalize known producer
display names to existing model IDs and language codes; reject unknown names.
Missing configuration in early no-speech events remains null, not a default.

Join capture and phase events only by the pair `(appSession, dictationSession)`.
Those identifiers are internal correlation keys and never appear in output.
Reject duplicate relevant events with the same pair; support either input
order. A capture without a phase is counted, not invented as a zero-time
dictation. A phase without a capture has unknown duration.

Use captured audio duration, not recording wall time, for length cohorts:
`short` is 0–5,000 ms inclusive, `medium` is over 5,000–15,000 ms inclusive,
`long` is over 15,000 ms, and `unknown` lacks a paired capture. Keep these
cohorts separate in every configuration group.

For each phase report count, numeric count, null count, missing count, p50 and
p95. Null/missing values never become zero. Use nearest-rank percentiles:
sort ascending, rank = ceil(percentile × count / 100), with one-based ranks.
Samples must be finite and non-negative. Output is deterministic across input
ordering. Input is bounded to 32 MiB total and 1 MiB of record content per
line, after stripping its optional LF or CRLF line ending.

## Explicit regression checks

A caller may select `--budget-length short|medium|long` together with
`--max-warm-p95-ms <threshold>` and optional `--min-samples <count>` (default
20). No machine-specific latency threshold is invented as a product default.
Both budget selectors are required together; counts must be positive and the
threshold finite/non-negative.

Only successful, warm, successfully auto-pasted samples of the selected length
are eligible. Every eligible configuration group must have at least the
requested numeric samples and p95 at or below the explicit threshold. No
eligible groups or incomplete eligible metrics fails the check. Emit the JSON
report even on a budget failure, then exit nonzero. The budget object also
reports `excludedWarmSuccessSamples`: the number of selected-length samples
with a successful warm transcription but a non-success paste outcome. These
samples are excluded from eligibility and do not prevent a pass when all
eligible groups satisfy the requested budget. Other cohorts are reported but
are not claimed to meet that selected budget. Invalid input or arguments exit
nonzero without emitting JSON; a failed explicit budget is the exception and
emits its JSON report before exiting nonzero.

## Remaining physical acceptance

Collect separate controlled cold/warm sessions for short, medium and long
utterances, using the same model and decoder settings. Compare a Bluetooth
headset microphone with the built-in microphone without assuming causality.
Measure visible editor arrival separately; do not label paste-call return as
that endpoint. Record p50/p95 and sample counts, then choose explicit regression
thresholds from a repeatable baseline.

Concurrent independent processes can still compete for Metal resources; this
report does not introduce cross-process inference priority or streaming.
Avoid concurrent batch inference during a clean baseline, then measure that
contention separately. Physical latency, fallback accuracy and model-choice
acceptance remain pending until those controlled measurements exist.
