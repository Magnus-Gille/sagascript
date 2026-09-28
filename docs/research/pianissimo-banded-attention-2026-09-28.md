# Pianissimo local-attention prototype — 2026-09-28

## Decision boundary

This is an **experimental opt-in file-transcription path**, not evidence to make
Pianissimo the default dictation engine or to publish a release. The signed
`d66c388` app in `/Applications` still uses its original CPU runtime. The
task worktree adds a local CLI `--pianissimo-device metal` selector; CPU
remains the default. Explicit Metal on the installed CPU-only runtime reports
a clear error instead of starting transcription; successful JSON includes
`requested_device` (the requested option, not a GPU-utilization measurement).
`test-build.yml` can build and signed-smoke the prototype
only with the explicit `pianissimo_banded_runtime` dispatch input. No such
signed run has been dispatched for this task branch.

For a local **unsigned CLI evaluation** after building the opt-in runtime and
downloading the Q8 model:

```sh
SAGASCRIPT_PIANISSIMO_EXECUTABLE=/path/to/experimental-runtime/bin/nemo-speech \
  src-tauri/target/release/sagascript transcribe recording.wav \
  --language sv --model pianissimo-sv --pianissimo-device metal --json
```

## Reproducible inputs and patch

- Model: corrected Pianissimo Q8 GGUF, 714,456,704 bytes, SHA-256
  `56ed7a0199c2c6116b3254505296e36b8126275f5c2bce826613aeb1fca7b1b5`.
- Engine: NVIDIA NeMo-Speech.cpp v0.1.0 source
  `4f9676226f667d14608487df744f375db87127f8` with ggml submodule
  `c03b4e2bcece5134827881af90242086daf75be5` and SentencePiece
  `17d7580d6407802f85855d2cc9190634e2c95624`.
- `scripts/pianissimo-local-attention-v0.1.0.patch` tiles offline query
  attention when the regular local mask has one input, finite left/right
  context, and more than 2,048 encoder frames. Keys and relative positions
  outside the allowed band are excluded *before* their matrix products; the
  original mask is sliced within each tile to preserve additional exclusions.
  Small inputs, nonlocal/chunked masks, batching and CUDA retain the original
  path. The global mask is still dense, so memory scaling is not fully solved.
- `scripts/build-pianissimo-banded-runtime.sh OUTPUT_DIRECTORY` checks exact
  source/submodule revisions, applies that patch, builds a generic `arm64`
  Metal runtime without system package installation, and stages license
  notices. The output directory must not exist and its parent must exist.
  A fresh source build with the checked-in script completed on the test Mac;
  its Metal doctor check passed, and the resulting runtime passed the strict
  long parity and unsigned CLI transcription gates.
- `scripts/eval-pianissimo-native.py` checks paired text, word timings,
  first-use time, warm time and speed without printing or saving transcript
  text. Local audio inputs are explicit and remain private. Five paired
  repetitions are required for a decision; first use is separate from warm
  timing. The content-free reference/scoring protocol is #187.

## Local Apple M4 results (32 GiB)

The same 278-second Swedish recording and corrected Q8 model were run
sequentially, alternating baseline and candidate. No VAD or batching. The
original audio and raw output were neither committed nor sent to a reviewer.

| Path, five runs | Baseline warm median | Prototype warm median | Observed warm p95 baseline → prototype |
| --- | ---: | ---: | ---: |
| Native Metal, baseline vs generic-arm64 patched runtime | 20.74 s | 11.69 s | 23.25 → 12.68 s |
| Sagascript release CLI, current signed CPU runtime vs generic-arm64 patched Metal runtime | 18.11 s | 11.43 s | 25.12 → 12.96 s |

The first row isolates the runtime patch on the same device. The second is
closer to file-product behavior but also changes device/backend; it includes
model verification and audio decode in separate CLI processes. The CLI's
decoding took about 0.3 s and model SHA-256 verification about 1.3–1.8 s in
stable runs. Host times varied substantially during the session, especially
when the Mac had low battery or concurrent builds; early 100-second CPU
observations did not reproduce when rerun serially on AC power. These are
single-recording timing distributions, **not p95 across independent users or
utterances**.

On additional development controls the native prototype preserved exact text
and word timestamps at encoder lengths 2,049 and 2,304 and on a 557-second
auto-split input. A 95-second digital-silence control stayed empty. At encoder
length 2,305, all 321 words remained identical, while **one word end time
shifted by 80 ms** on Metal; this was deterministic over three paired runs.
The CPU variant matched exactly at that boundary. An explicit developmental
tolerance of at most one encoder frame for no more than 0.5% of words can
evaluate this case; it has **not** been established on independent speech.
An 11-second tracked public JFK WAV repeated 21 times produced a 231-second
input above the tile threshold (462 words locally). The pinned unpatched and
patched Metal runtimes gave identical raw text and all word timestamps for
that input. The opt-in TEST workflow now generates this input, enforces exact
text and timestamps against the unpatched Metal reference *before signing*,
then runs it through the signed installed-CLI symlink with explicit Metal.
The repetitions are an implementation regression fixture, not 21 independent
utterances or a substitute for #187's Swedish quality gate.

First execution from a new runtime path is not represented by the warm
medians: a fresh unsigned native copy sometimes took 8–12 s on either CPU
or Metal, while a copied signed CPU runtime took 5.6 s first and 1.0 s next.
This is not proven to be Metal shader compilation. First-use timing in a
**signed patched app** is still unknown.
The existing nested-runtime signing helper signed and verified eight Mach-O
files in an **ad-hoc signed copy** of the patched runtime, followed by a local
Metal transcription smoke test. This establishes helper compatibility, not
Developer ID, notarization, entitlement or first-launch acceptance.

## Remaining gates

1. **Quality / adoption (#187):** At least 40 independent held-out Swedish
   utterances from two or more speakers, with checked references, specialist
   terms, numbers/negations and silence, are required before changing default
   dictation. Publisher read speech and a few local unreferenced recordings
   cannot establish WER or the required cold/warm end-to-end p95 thresholds.
2. **Signed opt-in TEST:** A future exact-SHA build must load and run the
   patched Metal dylib inside the notarized app, verify CPU fallback and
   first-use latency on real hardware, and receive owner-assisted acceptance. It requires
   separate just-in-time authorization. The production release workflow is
   unchanged and retains the CPU runtime.
3. **Older macOS support:** Generic-arm64 source builds target macOS 13.0,
   but this SDK warns that a ggml BLAS call is available only from 13.3.
   Do not claim 13.0–13.2 compatibility without an actual old-OS test or a
   controlled fallback/build change.

Independent read-only patch review by actual `claude-fable-5-1` (medium
requested; effective runtime effort not exposed) found no P1. It suggested
tile-boundary tests, which found the 80 ms case. A separate plan review
confirmed CLI opt-in should precede a signed Metal test. These reviews were
advisory; the tests above establish only their stated scopes. Its follow-up
about a CPU-only binary accepting `--pianissimo-device metal` was addressed by
an explicit runtime-library check and a CLI regression test against the
currently installed CPU-only bundle.
