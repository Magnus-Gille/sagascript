# Swedish diarization benchmark (issue #284)

Eval set of five Riksdag chamber debates (August 2026) with ground truth from the Riksdag
open data (speech start/length and speaker per speech, from the riksdagen.se webb-tv page
JSON). Manifest, SHA-256 sums and reference RTTMs: `docs/benchmarks/data/diarization-sv/`.
Audio is downloaded to a scratch directory and is not in git.

Rebuild the set: `scripts/diarization_sv/build_refs.py`. Score:
`scripts/diarization_sv/eval_der.py` (Python with numpy + scipy).

## Reference and metric

- One reference turn per speech, labelled by speaker name. Speeches under 5 s (chair
  announcements) are dropped. Time between speeches (chair/Talman, votes) is unlabelled and
  not scored: the scored region is the union of speech intervals. The chair therefore
  appears as an extra hypothesis cluster that costs nothing, so "found" may be truth + 1.
- Debates over 75 min are cropped after a complete speech (SoU40 is the full 74 min).
- DER is frame based (10 ms) with a 0.25 s collar around every reference turn boundary and
  an optimal 1:1 speaker mapping: (missed + false alarm + confusion) / reference speech time.
  A 10-15 % "missed" floor remains on every debate because the segmentation model drops
  quiet or overlapped speech; confusion is the speaker-identity error.
- Speakers "found" counts hypothesis speakers with at least 5 s of scored speech.

| Debate | Duration | Speeches | Reference speakers |
| --- | ---: | ---: | ---: |
| SoU40 (full) | 73.9 min | 21 | 7 |
| JuU41 (crop) | 48.7 min | 7 | 4 |
| SoU39 (crop) | 58.9 min | 14 | 7 |
| SoU38 (crop) | 53.5 min | 10 | 6 |
| UU24 (crop) | 44.7 min | 10 | 4 |

## Results

Default settings, before (main, threshold 0.75) and after (threshold 0.45 plus the change below):

| Debate | Truth | Found before | DER before | Found after | DER after (miss / conf) |
| --- | ---: | ---: | ---: | ---: | ---: |
| SoU40 | 7 | 1 | 81.2 % | 8 | 11.2 % (10.1 / 1.1) |
| JuU41 | 4 | 1 | 68.8 % | 5 | 16.1 % (15.6 / 0.5) |
| SoU39 | 7 | 1 | 77.9 % | 8 | 11.6 % (10.6 / 0.9) |
| SoU38 | 6 | 2 | 67.9 % | 7 | 15.8 % (14.6 / 1.2) |
| UU24 | 4 | 2 | 54.9 % | 5 | 11.2 % (10.3 / 0.9) |

The extra speaker after the change is the chair in every case (within +1 of truth).
Speed: the analysis stage (segmentation + embeddings, 89 s for 53 min) is untouched and
the clustering stage stays at about 0.02 s on the 74 min debate, so cache reruns remain
instant.

Threshold sweep, speakers found / DER:

| Threshold | Before (main) SoU40 | Before UU24 | After SoU40 | After JuU41 | After SoU39 | After SoU38 | After UU24 |
| ---: | --- | --- | --- | --- | --- | --- | --- |
| 0.35 | - | - | 8 / 11.2 % | 5 / 16.1 % | 8 / 11.6 % | 7 / 15.8 % | 5 / 11.2 % |
| 0.45 | 2 / 68.6 % | 2 / 54.9 % | 8 / 11.2 % | 5 / 16.1 % | 8 / 11.6 % | 7 / 15.8 % | 5 / 11.2 % |
| 0.50 | - | - | 7 / 21.7 % | 5 / 16.1 % | 8 / 11.6 % | 6 / 15.6 % | 5 / 11.2 % |
| 0.55 | 3 / 71.9 % | 2 / 54.9 % | 5 / 29.6 % | 4 / 23.7 % | 7 / 25.9 % | 5 / 28.9 % | 5 / 11.2 % |
| 0.75 | 1 / 81.2 % | 2 / 54.9 % | 1 / 81.2 % | 1 / 68.8 % | 1 / 77.9 % | 2 / 64.3 % | 3 / 33.6 % |
| 1.00 | 1 / 81.2 % | 1 / 68.4 % | - | - | - | - | - |

End to end through the CLI (`transcribe --diarize --meeting-json`, KB-Whisper Tiny, new build, UU24, default threshold):
5 speakers found, DER 5.0 % (miss 4.0 / confusion 1.0); the meeting JSON only covers transcribed
speech, which is why it scores below the raw diarization segments. The full five-debate numbers above
use the diarization-only helper because Whisper on 4 h of audio was impractical on a loaded machine.

Short clips (the earlier tuning set, 2 speakers each; `test-audio/diarization/fetch.sh`):
`nb_samtale_nb12` 7.0 % DER, 2 speakers; `dj_2022_feu` (telephone audio) 8.9 % / 2 speakers
before, 14.9 % / 3 speakers after. Both stay within +1.

## Diagnosis

1. Embeddings are good. WeSpeaker cosine distance between segments labelled with the same
   speaker has median 0.15-0.17 (95th percentile 0.45-0.58); between different speakers the
   median is 0.68-0.71 and the 5th percentile 0.44-0.50 (SoU38 and SoU40, segments of at
   least 1.5 s). A threshold around 0.45 separates them; 0.75 sits above nearly all
   different-speaker pairs, so it merges everything.
2. The threshold barely mattered on main because of the stabilisation step. Window
   stitching produces only 2-3 global tracks on a turn-taking debate (SoU38: 2 tracks for 6
   speakers) and every track was forced into a single embedding cluster
   (`stabilize_clusters_by_track`, `diarization/mod.rs`). The number of speakers could
   never exceed the number of tracks, so lowering the threshold changed nothing (SoU38: 2
   speakers at every threshold from 0.3 to 0.75), matching the report in #284.
3. Without that cap, clustering at 0.45 over-splits short noisy segments (telephone audio)
   into small spurious clusters.

## Change

- `diarization/mod.rs`: each embedded segment keeps its own embedding cluster
  (`assign_speaker_labels`); tracks are only a fallback label for segments too short to embed.
- `diarization/clustering.rs`: `absorb_small_clusters` merges clusters with less than 8 s of
  total speech (`MIN_SPEAKER_SECONDS`) into the nearest cluster by centroid cosine.
- Default threshold 0.75 -> 0.45 (core, `transcribe --diarize-threshold`, `meeting
  reprocess` CLI, reprocessing UI). Clustering stays threshold-only, so `--diarize-cache`
  reruns skip segmentation and embeddings as before.

## Reproduce

```bash
cd src-tauri && cargo build --release -p sagascript-core --features diarization --example diarize_eval
python scripts/diarization_sv/build_refs.py
python scripts/diarization_sv/eval_der.py --bin target/release/examples/diarize_eval \
  --scratch ~/.cache/sagascript-bench/diar-284 --thresholds 0.35,0.45,0.55,0.75
# end to end, through the CLI (runs Whisper; slow):
python scripts/diarization_sv/eval_der.py --cli target/release/sagascript --scratch <dir> --ids UU24
```

`diarize_eval` (`crates/sagascript-core/examples/diarize_eval.rs`) runs the diarization
stages without Whisper so thresholds can be swept quickly.
