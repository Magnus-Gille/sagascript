# Diarization reference qualification and evaluation

The CLI keeps annotation qualification separate from model scoring. A reviewer first makes a
blind pass over supplied windows and records `verified`, `unknown`, or `candidate` intervals with
human evidence. When available, a second reviewer checks a random sample of at least 20% of the
windows. The reference manifest freezes the source hash, policy identity, split identity, and
window boundaries.

For diarized `transcribe --progress-json`, each per-file progress event includes an additive
`decoder` object with the effective beam, temperature-fallback, VAD, and timestamp-method fields
used by the final native report. The snapshot is present from decode/resample through the
analyzing, clustering, finalizing, and completed phases, including cache-hit runs. Ordinary
transcription progress keeps its existing event shape; progress remains stage-boundary metadata
and does not invent an overall percentage or ETA. The outer cancellation event can omit
this file-scoped snapshot when no effective file configuration is available.

Evaluation receipts include uncollared short-reference-region and boundary measurements under
`strata`. They reuse the global DER speaker mapping and original UEM; the explicit
`strata_collar_seconds: 0.0` distinguishes them from the requested global DER collar. See
[the measurement definitions](diarization-strata.md).

## Independent metric cross-check

`scripts/diarization_sv/crosscheck_metrics.py` checks thirteen synthetic cases at collars 0 and
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
The zero-reference and collar-erased-reference cases check that DER can be null while
JER remains defined on the original UEM. Linux CI runs these checks and all reference/evidence
contract tests with the built batch CLI, including cross-language reference identity parity.

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
Well-formed JSON with invalid qualification inputs produces an `invalid-input` report and
a nonzero command exit. Uncomputed identities and coverage are null; file hashes identify
the rejected inputs. Invalid CLI flags, JSON syntax and file-read errors use the normal
CLI error stream. For `reference-qualify`, `--minimum-coverage` must be finite
and between 0.90 and 1.0.

Transcript evaluation requires a canonical acoustic report and rejects edited transcripts,
including the legacy top-level edit marker. Legacy meetings without such a report remain
importable, but lack durable edit provenance for transcript scoring. Acoustic evaluation
can still use preserved activity after transcript corrections.

For native references, the source hash and exact decoded-audio duration must match the
hypothesis. Use the decoded duration recorded by Sagascript; container duration can differ.
For RTTM, `reference_recording_id` binds the reference to its UEM only. The receipt keeps
`source_sha256` null, records `hypothesis_source_sha256` separately, and labels the binding
as `rttm_recording_id_and_uem`; it does not establish source identity between the two files.
RTTM scoring regions must fit within the hypothesis duration.

To score a frozen split, pass both `--manifest split.json` and `--split dev` or `--split eval` to
`diarization evaluate`. The command rejects stale or unqualified manifests, selects only verified
intervals in the requested frozen windows, and refuses an external `--uem` that could bypass the
reference mask. The default metric targets are DER ≤ 0.10 and confusion ≤ 0.05, expressed as
fractions of reference speaker-time; callers can bind exact alternatives with
`--maximum-der` and `--maximum-confusion`. The receipt records the observed fractions, thresholds,
and `--collar` value in seconds. A zero-reference-speaker evaluation cannot pass metric targets.
Metric results remain measurement output; `quality_adoption_ready` stays false pending the frozen
public regression suite and held-out decision gate.

## Diagnostics cost and decoder cache observations (2026-10-09)

The accepted `8265b2e222286dc6f69338e7a389d631afa3ec03` CLI was measured locally on the
784.512-second public `IP_hc10606.wav` clip (SHA-256
`be970461b0e0df1264cdde90fe0c50da980c2e99cbc4683745e33c49ae555086`). Every run used
Swedish, `kb-whisper-tiny`, beam 0, no VAD, threshold 0.34, no speaker-count hint,
and a separate clone of the same frozen analysis cache. Two warmups were excluded;
legacy JSON used six runs per condition in balanced off/on/on/off order, and native
meeting JSON used two per condition. The only flag difference was diagnostics.

| Measurement | Diagnostics off | Diagnostics on |
|---|---:|---:|
| Native meeting JSON bytes | 18,948 | 434,165 |
| Compact native report bytes | 4,338 | 256,322 |
| Analysis cache bytes | 428,197 | 428,197 |
| Legacy JSON wall-time median | 0.402 s | 0.399 s |
| Legacy JSON process peak-RSS median | 18,022,400 bytes | 19,791,872 bytes |

All 16 measured runs hit the cache. Source, original cache and CLI hashes stayed
unchanged. Within each output format, text segments, token order, speaker labels,
acoustic activity, decoder settings and other provenance stayed identical; only
opt-in diagnostic evidence and execution timing fields changed. Diagnostic reports
contained 49 region records, 1,334 attribution records and 130 ASR records. Native
and legacy serializers can represent the same floating-point fields differently.

The timing ranges overlap (off 0.348–0.493 s; on 0.392–0.489 s), so these observations
do not establish a speed effect. This is one host and one warm-cache public clip,
not fresh-inference latency or quality evidence. Darwin peak RSS is a process
high-water statistic, not an estimate of incremental diagnostic allocation.

A separate five-run functional CLI check reused a cloned cache, then changed beam
0→2 and VAD off→on. Each decoder change caused a miss and changed the raw cached
word/timing payload hash. Changing only threshold or an ordinary speaker-count
hint then hit the cache and retained the raw payload hash. VAD changed both text
and timing despite the same whitespace-token count. All decoder, result-performance
and cache identities agreed. Local models were already present and checksum-verified.
Those elapsed times were collected under concurrent compilation and are not a fair
performance ablation. These checks support propagation and cache contracts; they
do not recommend a decoder or satisfy the human-reference quality gate.
