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

### Explicit local transcript capture

The benchmark defaults to content-free stdout and the language's recommended
model. `--model`, `--beam-size` and `--disable-temperature-fallback` override only
that invocation; no saved settings, personal glossary or model downloads are
used. The selected model must already exist locally.

To capture evaluation text deliberately, choose a **new file** in an existing
private, non-synced directory:

```sh
sagascript benchmark-dictation fixture.wav --language en --iterations 5 \
  --quality-output /private/local-evaluation/new-run.json
```

The report contains plaintext cold and warm transcripts, selected decoder
settings, build identity, source-file SHA-256 and a domain-separated SHA-256 of
the decoded 16 kHz float samples. Source bytes are hashed before and after the
run; changed inputs abort export. Export fixtures are bounded to 128 MiB and
120 decoded seconds, each transcript to 100,000 characters and the report to
16 MiB. Existing destinations, including symlinks, are never overwritten.
Unix files are created with mode 0600; Windows inherits the chosen parent ACL.
An interrupted write may leave an incomplete file: discard it, never treat it
as evidence or append another run. Do not commit or upload private reports.

Schema v1 records `measurement_endpoint=live_inference_call_not_visible_text`
and `cold_definition=first_call_in_new_backend_not_system_cold`. The
`model_expected_sha256` and `model_expected_bytes` fields are registry metadata,
not a claim that a model was loaded or independently hashed during every run;
silence can return before model loading. Capture actual model-file integrity
separately for an experiment. `cli_checks_passed` covers only the CLI's
nonempty/expected-word/timing-budget checks, **not quality or adoption**. A
failed such check still exports collected rows and returns a failing exit code.
Inference, input-integrity or export-I/O failures do not produce a valid report.

Use `--allow-empty` only with `--quality-output` for silence controls. It permits
empty results without filtering hallucinated text; the scorer must judge both.
It cannot be combined with `--expect-word`. Normal stdout remains content-free
even when a separate quality report was explicitly requested.

Use the same distinct utterances for each configuration. Randomize run order,
record its seed/order, and collect at least five warm repetitions per utterance.
Predeclare the **first warm transcript** as the accuracy observation for paired
WER and specialist recall, never the best run. Retain every cold/warm text score
to audit instability and any number/negation/silence failures. Later timing
repetitions do not create additional independent accuracy observations.
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
verified. The public JFK smoke verified local report creation and privacy
boundaries, not a curated corpus, accuracy gain or adoption result.

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

## Local evaluation command

Requires Python 3.10 or newer, standard library only. CI explicitly selects
Python 3.12 through a commit-pinned setup action; clip scores record the actual
runtime Python/Unicode versions and the versioned normalization rule. No network, models,
settings or private recordings are read implicitly. Use local JSON/UTF-8 paths
explicitly; the command prints content-free JSON and does not write files.

```sh
python3 scripts/dictation_eval/evaluate.py --version
python3 scripts/dictation_eval/evaluate.py validate-manifest corpus.json
python3 scripts/dictation_eval/evaluate.py score-clip \
  --manifest corpus.json --utterance-id clip_001 \
  --report new-run.json --reference exact-reference.txt \
  --specialist-terms specialist-terms.json --control-terms control-terms.json
python3 -m unittest discover -s scripts/dictation_eval -p 'test_*.py'
```

Use `python` instead of `python3` on Windows. Term files are optional JSON
arrays of phrases. The specialist tag requires expected terms; number/negation
tags require controls. `score-clip` binds report language/source SHA-256 and
exact UTF-8 reference bytes to the selected manifest row: even a changed final
newline invalidates the reference hash. Non-silence references must contain
at least one normalized word; empty/whitespace/punctuation-only speech
references fail instead of silently producing undefined WER. The strict report reader rejects
unknown schemas/keys, duplicate JSON keys, nonfinite values, invalid text and
missing/reordered runs. Schema-v1 clip scores contain only allowlisted
configuration labels, timings, counts and per-run metrics, not IDs, hashes,
paths, build labels or text. Missing false-correction annotations remain null.
The clip decision is always `inconclusive`; successful exit means valid inputs
were scored, not that corpus coverage or adoption thresholds passed. Invalid
inputs return exit code 2 with a content-free diagnostic.

The pure `paired_plan.build_plan` helper validates up to three distinct
configuration roles per language, requires a baseline for every selected
language and deterministically shuffles every utterance/configuration pair
exactly once using a recorded seed. A plan binds canonical manifest/config
hashes, the declared source revision and binary SHA-256, and 5–20 warm repeats.
It contains opaque IDs/hashes and stays local, unlike content-free score
summaries. These are declared identities, not proof of binary provenance or
that a configuration labeled baseline equals product defaults: the experiment
operator must verify those before freezing and executing a plan. The helper
does not launch inference, read files, download models or make adoption claims.
Individual benchmark captures allow 2–30 warm runs for diagnostics; paired
experiments deliberately use the narrower 5–20-repeat range.

### Explicit private paired execution (evaluator 0.2.0)

`freeze-plan` writes a new private plan after hashing an explicitly selected
trusted executable. `run-plan` revalidates the exact frozen order and all input
maps, hashes the executable/audio before and after each child, and retains
every planned pair in a private ledger. It runs only `benchmark-dictation`
with literal argv: explicit language/model/beam/repeats, the selected fallback
flag, and `--allow-empty` only for silence. Models must already be installed.
The supplied binary is trusted operator input: a hash binds its bytes, not its
source provenance, and this runner is not a network/process sandbox.

```sh
python3 scripts/dictation_eval/evaluate.py freeze-plan \
  --manifest /absolute/corpus.json --configurations /absolute/configs.json \
  --split heldout --seed 187 --iterations 5 --source-revision FULL_SOURCE_SHA \
  --binary /absolute/sagascript --output /absolute/new-plan.json
python3 scripts/dictation_eval/evaluate.py run-plan \
  --manifest /absolute/corpus.json --plan /absolute/new-plan.json \
  --audio-map /absolute/audio-map.json --reference-map /absolute/reference-map.json \
  --terms /absolute/terms-map.json --binary /absolute/sagascript \
  --output-dir /absolute/new-evaluation-directory
```

Audio/reference maps contain exactly the selected split's opaque IDs mapped to
absolute local file paths. Terms maps contain those same IDs mapped to objects
with `specialist_terms` and `control_terms` arrays. Reference UTF-8 bytes are
hash-checked without trimming and frozen in memory. No paths or transcripts
are printed in successful summaries or processing-error messages.

Output must not exist and its parent must already exist. Unix directory/file
modes are 0700/0600; Windows inherits the operator-selected parent's ACL.
Each pair retains its raw transcript-bearing quality report, private stderr,
and scored result under numeric names. Stderr can contain paths/text and must
not be published. These diagnostics are not a public aggregate. Existing files
are never overwritten; interruption/disk failure can leave a partial directory,
which is retained without implicit resume or deletion. The default child
timeout is 900 seconds for the entire cold-plus-warm sequence of one pair
(explicit range 1–3600). Identity failure stops further
launches while recording remaining pairs as not attempted.

A nonzero child with a valid report keeps its scores and failure status rather
than disappearing from the experiment. Failed executions yield exit 1 with a
content-free summary; invalid preflight inputs yield exit 2. Output I/O failures
after creation yield exit 3 and explicitly require retaining partial output.
Binary, audio and reference inputs must be regular files, not symlinks; resolve
an executable symlink explicitly before freezing its plan. Summary success is
only execution success: `decision` remains `inconclusive`. Cold means the first
call in each new backend, not an OS-cold machine. First warm is fixed for WER;
later warm results remain available and must not become best-of selection.

Public-corpus acoustic conditions may be `unknown`. Unknown is counted and
cannot satisfy either required quiet or noisy coverage. Do not infer acoustic
labels from a dataset name. Freeze duration bucket cutoffs and provenance
before choosing clips; public read/parliamentary speech is not automatically
representative of personal dictation or unseen by model training.

The final deliverable still needs the actual paired corpus and aggregate result workflow,
baseline/candidate tables for each language, paired accuracy intervals, redacted
worst failures, adopt/adapt/reject/inconclusive decision, implementation map and
rollout/revert plan. Tooling tests are not issue completion.
