# Cross-product evaluation — 2026-09-06

Status: `INCONCLUSIVE`. This report records one bounded Norwegian file/HTTP
comparison and supports no adoption, ranking, default change, or general
quality claim. Issue #196 remains open; the associated PR is draft-only.

## What was actually measured

The run used the same 40 held-out human Norwegian NPSC utterances for each
completed configuration, with 907 reference words, five warm calls per
utterance, seed `187`, 2,000 paired resamples, and the frozen
`nfc-casefold-nfc-words-v1` scorer. The public corpus snapshot is pinned to
[NbAiLab/NPSC revision `e906435cc445e38961d86f3ebf53a7b38603b903`](https://huggingface.co/datasets/NbAiLab/NPSC/tree/e906435cc445e38961d86f3ebf53a7b38603b903).
The accuracy bootstrap used 2,000 paired resamples of the 40 utterances,
not speaker-clustered resampling; the summaries were checked with the
corrected scorer SHA-256 `4f37853c4770c6694e75c3e41912dd7ac7a4882c52c68e845ea59b5872ab5df5`.
The scorer test suite had 9 passing tests under Python 3.10.13 and Unicode
13.0.0, and summary v1/v2 were byte-identical for the reported aggregate.
The scratch scorer currently imports the reviewed #187 evaluation helpers from
a sibling local worktree; its hash is an audit identifier, not a standalone
reproduction package.
The held-out selection has 40 human utterances from 8 speakers, but its
environment is `unknown`: it has no verified quiet/noisy labels, silence
controls, or specialist/numbers/negation/ordinary coverage. English and
Swedish were not present. The corpus therefore fails the #187 eligibility gate
even though the completed rows are retained below.

Sagascript completed 120/120 planned rows with zero failures. Vibe completed
80/120; its decoder-disabled-fallback configuration was explicitly unsupported
for 40 rows, with zero failures. Unsupported rows were retained rather than
excluded from denominators or silently replaced.

The Sagascript run used source revision
[`5e1e7f77f9f5c0f9aac5b9ee4a03ec5ecfe7b441`](https://github.com/Magnus-Gille/sagascript/commit/5e1e7f77f9f5c0f9aac5b9ee4a03ec5ecfe7b441).
The executed Vibe artifact used revision
[`57c93c3c5a862d630459341cd71b0326a46a8a19`](https://github.com/thewh1teagle/vibe/commit/57c93c3c5a862d630459341cd71b0326a46a8a19).
The protocol's earlier Vibe capability/README pin
[`e1eba11ad37dec2ef8bb5ba181394567020302c8`](https://github.com/thewh1teagle/vibe/commit/e1eba11ad37dec2ef8bb5ba181394567020302c8)
is historical and remains unchanged except for a clarification link. It must
not be confused with the revision used for this executed artifact.

The executed Sagascript identity was `sagascript 1.1.3` with generated build
identity `git 5e1e7f7-dirty`, built 2026-09-06, from source revision
`5e1e7f77f9f5c0f9aac5b9ee4a03ec5ecfe7b441`. Its binary SHA-256 was
`a13adcf4ca47e43abdf38fbb10305ef52fb473f63e12c1c2da33f18bf8f038cb`.
This was the lean ARM64 CLI evaluator binary, not a signed desktop build; the
dirty suffix is disclosed and is not a clean-build attestation. The executed
Vibe server was `v0.6.9` at the revision above, with binary SHA-256
`4eafbea158b45d37d734b957a546a0fd6b57fe54a7eb15fffc347bc8093b1f7a`.

Both products used the same `nb-whisper-base` and `nb-whisper-tiny` q5_0 model
artifacts, but they used different Rust/Vibe runtimes and inference paths,
including different threshold/FlashAttention behavior. This is a
same-weights/different-runtime comparison, not strict backend parity. The
recorded artifact SHA-256 values are:

| Model | Public model-card revision | Artifact SHA-256 |
| --- | --- | --- |
| `nb-whisper-base` | [`2ab372b6baa181a22f54f18030cae3703402c59e`](https://huggingface.co/NbAiLab/nb-whisper-base/tree/2ab372b6baa181a22f54f18030cae3703402c59e) | `dcb9f3ab963cd288974c826c1519ff73b78b2372e80d388a6ce94f29c6a5b40f` |
| `nb-whisper-tiny` | [`8b38492d0e4111d5d6ad825e979cb082a2da013a`](https://huggingface.co/NbAiLab/nb-whisper-tiny/tree/8b38492d0e4111d5d6ad825e979cb082a2da013a) | `e5fb42192cdf31bea624a524d035e8895030b2bb4b31d4ea2a1ebf0ea8f57237` |

The model-card revisions identify the public model sources; the artifact
hashes identify the exact local q5_0 files used by both runtimes.

## Accuracy summary

The intervals are paired utterance intervals from the frozen seed and scorer.
They are descriptive only because the corpus eligibility gate failed.

| Configuration | Sagascript WER | Vibe WER | Vibe − Sagascript paired delta (95% interval) |
| --- | ---: | ---: | ---: |
| `nb-whisper-base`, fallback enabled | 14.553% | 14.333% | −0.221 pp (−2.442, +2.251) |
| `nb-whisper-tiny`, fallback enabled | 17.971% | 25.799% | +7.828 pp (+4.000, +12.572) |

The base paired interval spans zero; this sample establishes neither a
difference nor equivalence. The smaller-model point estimate differs in this
sample, but it does not establish general quality superiority. The
decoder-disabled-fallback row has no Vibe quality result because that setting
was unsupported. No result here changes a product default or establishes a
ranking.

## Timing boundary and limitations

Sagascript timing is `live_inference_call_not_visible_text`; its cold call is
the first call in a new backend process. Vibe timing is
`client_http_request_to_complete_body_not_visible_text`, i.e. HTTP wall time
to the completed response body, not visible text and not Sagascript internal
inference timing. Sagascript warm timings include its internal model/inference
path; Vibe model readiness occurred before the HTTP request, so
its boundary excludes that ready-process work. These cold definitions differ.

For reference only, Sagascript/Vibe baseline warm p50/p95 were
152.4/315.0 ms and 617.8/5631.1 ms; smaller-model warm p50/p95 were
97.8/224.5 ms and 113.6/273.5 ms. These numbers must not be used to rank
products or claim a cross-product latency gain because the endpoints differ.
The host was an M4 Mac with 32 GB RAM on AC power running macOS 26.6.2;
actual GPU utilization and thermal state were not attested, and no competing
local inference workload was run. A short local compile occurred during the
Sagascript held-out run, so no idle-CPU claim is made.

The report contains no transcript text, opaque utterance IDs, speaker IDs,
audio hashes, private paths, raw logs, or diagnostics. The copied aggregate
JSON files are content-free and preserve the runner's completed/unsupported
counts and metrics. Exact-copy SHA-256 values are `46cff67504e628a869bbb13b5605de1bc2752851d03dcd75bf5fc91eb1914f2e`
for Sagascript and `f8163d26565463d2f60a65e0dfcd9c484ea3af8d53191fa277850036c8275e2c`
for Vibe.

## Scope still missing

Handy and FluidVoice desktop measurements, the physical visible-text endpoint,
long-file evaluation, and independent silence controls were not executed.
Those missing gates, the NPSC coverage failure, differing timing boundaries,
and the unsupported Vibe decoder row make the overall result inconclusive.
No product is adopted, ranked, or made the default from this report.

The comparison protocol is documented in
[`cross-product-evaluation-protocol.md`](../cross-product-evaluation-protocol.md).
The aggregate files beside this README are exact copies of the bounded held-out
summaries used for this report:

- [`sagascript-no-heldout.json`](sagascript-no-heldout.json)
- [`vibe-no-heldout.json`](vibe-no-heldout.json)
