# Pianissimo native conversion fidelity — issue #247

## Finding

The earlier Q8 and FP16 GGUF conversions omitted the model's mel filterbank.
The pinned NeMo-Speech.cpp converter tried to generate `preprocessor.fb` with
`librosa.filters.mel()`, but the Python 3.10 converter environment could not
import `_lzma` while resolving that call. The converter caught the nested
`ImportError`, printed `WARN: librosa not installed`, and emitted a runnable
GGUF without `preprocessor.fb`. The native runtime then used its documented
unit-peak triangle fallback instead of NeMo's Slaney-normalized filterbank.

This was primarily a **converter environment/fallback difference**, not a Q8
precision loss. The same conversion with a working mel filterbank changed only
one tensor: `preprocessor.fb`. All 700 common tensor names, types, and bytes
were identical between the old and corrected FP16 files; the same comparison
held for Q8. Both corrected files contain 701 tensors, including the new
filterbank. The native runtime reports no errors or warnings for them.

This is a local diagnostic result, not a new Sagascript release or default.
The corrected GGUF files remain outside Git and have not been published.

## Frozen inputs

| Input | Identity |
| --- | --- |
| Original checkpoint | `KlangAI/pianissimo-sv` revision `8f1f6d8f8bd7482a5ea1d2bfaf6ef5be61597138`; `.nemo` SHA-256 `ca340b827dc9e18d2019341fa7b6dc163f00284d84066ce2cbfffcaa129920cd` |
| Converter and native engine | `NVIDIA/NeMo-Speech.cpp` v0.1.0, commit `4f9676226f667d14608487df744f375db87127f8` |
| Audio | The 100 public Swedish 16 kHz mono clips and references from #245; manifest SHA-256 `169164f7290965beac797ee0e0a196f6006008261dc14c5df233a82cd103835a`; 1,014 reference words |
| Original inference | NeMo 2.7.3 / PyTorch 2.14.0, Python 3.12.13, MPS; existing #245 outputs |
| Old conversion | Python 3.10.13 / PyTorch 2.12.0 / `gguf` 0.19.0; `librosa.filters.mel()` raised `ModuleNotFoundError: No module named '_lzma'` |
| Corrected conversion | Python 3.12.13 / PyTorch 2.14.0 / `gguf` 0.19.0 / librosa 1.0.0; exact filterbank generation succeeded |

The original and corrected models used the same audio, greedy TDT decoding,
Swedish language setting, NeMo-Speech.cpp v0.1.0, one recognizer, and
concurrency 1. Output was scored with the #245 `score_text` corpus scorer.
The old and corrected Q8 files were additionally run on Metal in the same
local benchmark sequence. The corrected Q8 CPU and Metal transcripts matched
exactly on all 100 clips.

## Quality

| Model and path | Model bytes | Word edits / 1,014 | WER | Raw transcript agreement with original | Normalized agreement with original |
| --- | ---: | ---: | ---: | ---: | ---: |
| Original `.nemo` / NeMo | 2,509,322,240 | 58 | 5.72% | 100/100 | 100/100 |
| Old FP16 GGUF / native | 1,296,549,472 | 70 | 6.90% | 68/100 on Metal | 72/100 on Metal |
| Old Q8 GGUF / native | 714,325,088 | 71 | 7.00% | 67/100 | 72/100 |
| Corrected FP16 GGUF / native CPU | 1,296,681,120 | 56 | 5.52% | 96/100 | 99/100 |
| Corrected Q8 GGUF / native CPU and Metal | 714,456,704 | 56 | 5.52% | 95/100 | 97/100 |

The corrected FP16 differed from the original in only one normalized clip:
`i dag` versus `idag`. Q8 and FP16 differed in four normalized clips, with
equal corpus word-edit totals. Relative to old Q8, corrected Q8 reduced
word edits on 16 clips, increased them on 6, and tied on 78. These are
paired observations on a publisher-provided read-speech sample; they do not
establish accuracy on spontaneous dictation or independent speakers.

A diagnostic GGUF with all 700 tensors stored as F32 but **without** the
filterbank still made 69 word edits. FP16 on CPU and Metal both made 70
edits, with only three raw transcript differences. These controls further
narrow the large original-to-native gap to the missing feature-extraction
tensor rather than Q8 or the Metal backend.

## Size and speed

| Q8, Metal, 100 clips | Old | Corrected |
| --- | ---: | ---: |
| GGUF bytes | 714,325,088 | 714,456,704 |
| Wall time, including load | 17.71 s | 17.67 s |
| Maximum resident set size | 896,270,336 bytes | 895,827,968 bytes |
| Peak memory footprint (`time -l`) | 916,768,352 bytes | 916,407,880 bytes |

The 0.04-second and sub-megabyte differences are measurement noise. The
corrected Q8 model is 131,616 bytes larger. The native v0.1.0 macOS runtime
archive measured about 3.5 MB compressed and about 10 MB extracted in #245;
the Python/NeMo/PyTorch bundle in #246 is about 1.7 GB. The native runtime
does not need Python at inference time. These figures do not yet describe a
signed Sagascript app or its full installer size.

## Reproduce and guard

1. Verify the original checkpoint and frozen manifest hashes above. Use the
   pinned converter checkout and a Python environment with `torch`, `gguf`,
   `librosa`, and `_lzma` available.
2. Run `python -c 'import librosa; librosa.filters.mel(sr=16000, n_fft=512, n_mels=128)'`.
   An `import librosa` check alone was insufficient because the failing import
   was lazy.
3. Convert the same `.nemo` with `python convert_model.py model.nemo --outfile
   pianissimo-sv-q8_0.gguf --outtype q8_0`. Reject **any** converter warning.
   Check the GGUF with `GGUFReader`: tensor `preprocessor.fb` must exist with
   `data.shape == (128, 257)`. `nemo-speech model info` must report 701 tensors and no
   errors or warnings.
4. Transcribe the frozen WAV directory with `nemo-speech transcribe DIR --model
   MODEL --device metal --language sv --format json --output-dir OUT
   --concurrency 1`; score with #245's `scripts/dictation_eval/text_metrics.py`.
   Compare the fixed files and decoder settings with the original checkpoint.

The corrected local artifacts are:

- Q8: 714,456,704 bytes, SHA-256
  `56ed7a0199c2c6116b3254505296e36b8126275f5c2bce826613aeb1fca7b1b5`.
- FP16 diagnostic control: 1,296,681,120 bytes, SHA-256
  `53a2802c99129dfe9899b75f074545c20b52c8775e0474bac6b856325791149c`.

## Decision

**Fix the conversion and offer corrected Q8 as an optional file model after
native integration and signed-app verification.** Do not offer the old Q8 or
FP16 artifacts. Keep existing defaults. A separate, independent dictation
evaluation under #187 is still needed before recommending Pianissimo over
KB-Whisper Large or claiming broad Swedish accuracy. The app integration,
model artifact hosting, signed build, and release belong to follow-up work;
none occurs in #247.
