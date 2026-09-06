# Dictation evaluation protocol (#187)

Status: local evaluation tooling in progress, **no measured adoption result**.
This protocol does not change shipped defaults or authorize remote speech
processing. The original Ignis source has unresolved licensing; these helpers
are independent implementations of evaluation concepts, not copied code.

## Frozen scoring rules

Normalize NFC, Unicode casefold, then NFC. Words contain Unicode letters,
numbers and combining marks. Internal straight/curly apostrophes are equivalent;
other punctuation, including hyphens, separates words. Do not remove accents,
transliterate, expand numbers, or remove negation. Record the Python and Unicode
database versions with every result. Inputs are bounded before normalization.

WER is the sum of substitutions, deletions and insertions divided by the sum
of reference words, never the mean of individual clip WERs. Empty references
have undefined WER; their insertions and silence-hallucination status are still
reported separately. Minimum-edit alignment ties prefer diagonal, deletion,
then insertion, so the S/D/I decomposition is reproducible.

Specialist recall uses exact normalized, non-overlapping whole phrase
occurrences. Each expected term must exist in the reference. Recall credits
at most the reference occurrence count; repeated hallucinated terms cannot
improve recall. Duplicate normalized fixture terms are rejected.

Number/negation occurrence-count differences are a diagnostic proxy, not proof
of semantic correctness: moving a negation without changing its count can
change meaning. Fixed controls also require human adjudication of semantic
errors before an adoption recommendation. False glossary replacements require
explicit reviewed annotations; never infer them from WER or silently replace
missing annotations with zero. Report per 1,000 ordinary-control reference words.

## Timing and paired uncertainty

Reuse `sagascript benchmark-dictation` for local model/inference timing and
`sagascript latency-report` for the existing content-free session phase data.
Neither establishes physical key-release-to-visible-editor latency. Keep
observable text arrival, paste dispatch/completion and inference separate.

Use the same distinct utterances for each configuration. Randomize run order,
record its seed/order, and collect at least five warm repetitions per utterance.
Cold observations stay separate. For warm latency intervals, resample paired
utterance clusters with replacement and retain every repetition in each chosen
cluster. Repetitions are not independent speakers or utterances. Compute
nearest-rank p50/p95; report relative p95 gain as 1 - candidate / baseline and
the 95% percentile bootstrap interval. Record the seed and resample count.
Do not use an interval conditional on one speaker/corpus as proof of population
representativeness. Accuracy also needs utterance-paired uncertainty; the
latency helper must not be misapplied to per-clip WER averages.

## Required experiment evidence

The local v1 corpus manifest is a strict metadata-only input: opaque utterance
and speaker IDs, language, development/held-out split, audio/reference SHA-256,
human/synthetic/silence origin, duration and environment buckets, and coverage
tags. It rejects duplicate utterance IDs and audio hashes, including reuse
across development and held-out splits. Unknown keys (including paths and text)
are rejected. Keep private metadata local even when the schema excludes text.
Schema validation does not prove consent, licensing, human provenance, hash
authenticity, semantic coverage, or that differently encoded clips are distinct;
those still require corpus curation and integrity verification.

`corpus_manifest.coverage_report` contains only aggregate counts and missing
coverage. It requires the stated human counts and at least two human held-out
speakers per language; synthetic rows cannot satisfy speech coverage. Its
`eligible` field means only that declared corpus prerequisites are present,
never that a configuration is accurate, fast, safe to adopt, or independently
verified. No manifest or report with real speech has been produced yet.

- English, Swedish, Norwegian: at least 10 development and 40 distinct held-out
  human utterances per language, at least two speakers, short/medium/long,
  quiet/noisy, names/product terms, numbers/negation, ordinary controls and silence.
- At most three configurations per language: current defaults, one smaller
  already-supported model and one decoder variation. Tune development only;
  freeze configurations, corpus membership and thresholds before held-out use.
- Exact source revision and CLI identity, model file hashes and provenance,
  decoder settings, hardware/OS/power state, signed-app identity, cold/warm
  separation, memory, model size and physical visible-text timings.
- Private audio/transcripts remain local. Public manifests may carry opaque IDs,
  consent/provenance metadata and hashes, never private paths or text. Synthetic
  fixtures validate tooling only and cannot establish adoption.

Recommend a changed configuration only with at least 20% lower warm p95
end-to-end latency OR 15% relative WER reduction; no more than 1 percentage point
absolute WER increase or 2 percentage points specialist recall loss; no new
number/negation or silence failures in fixed controls. Treat uncertain gains,
missing physical timing/annotations, inadequate corpus or unsupported provenance
as **inconclusive**, never as passing. Do not relax thresholds after seeing data.

The final deliverable still needs the CLI-driven manifest/result workflow,
baseline/candidate tables for each language, paired accuracy intervals, redacted
worst failures, adopt/adapt/reject/inconclusive decision, implementation map and
rollout/revert plan. These initial pure helper tests are not issue completion.
