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

Headline metric: **confusion** (speech assigned to the wrong speaker; the part clustering
controls), then DER. Missed speech comes from the segmentation model, not from clustering, and
dominates DER on the debates (10-15 % on every debate at every threshold), so DER alone hides the
clustering effect. Default threshold is now 0.48 (see "Choosing the default").

**The Riksdag results are in-sample**: the debates were used to find the track-cap root cause and
to sweep the threshold. The out-of-domain clips below were not used to tune the algorithm, but
they were used afterwards to choose the threshold, so no number here is a clean held-out
estimate; the leave-one-out tables show how stable the choice is, not generalisation to unseen
domains.

Each cell is speakers found, then confusion / DER. `main (0.75)` is the previous code and default.

| Recording | Truth | main (0.75) | 0.40 | 0.45 | 0.48 (default) | 0.50 |
| --- | ---: | --- | --- | --- | --- | --- |
| SoU40 | 7 | 1, 71.1 / 81.2 % | 8, 1.1 / 11.2 % | 8, 1.1 / 11.2 % | 7, 11.6 / 21.7 % | 7, 11.6 / 21.7 % |
| JuU41 | 4 | 1, 53.2 / 68.8 % | 5, 0.5 / 16.1 % | 5, 0.5 / 16.1 % | 5, 0.5 / 16.1 % | 5, 0.5 / 16.1 % |
| SoU39 | 7 | 1, 67.3 / 77.9 % | 8, 0.9 / 11.5 % | 8, 0.9 / 11.5 % | 8, 0.9 / 11.5 % | 8, 0.9 / 11.5 % |
| SoU38 | 6 | 2, 53.3 / 67.9 % | 7, 1.2 / 15.8 % | 7, 1.2 / 15.8 % | 6, 1.0 / 15.6 % | 6, 1.0 / 15.6 % |
| UU24 | 4 | 2, 44.6 / 54.9 % | 5, 0.9 / 11.2 % | 5, 0.9 / 11.2 % | 5, 0.9 / 11.2 % | 5, 0.9 / 11.2 % |
| dj_2022_feu | 2 | 2, 0.0 / 4.0 % | 1, 43.1 / 43.1 % | 1, 43.1 / 43.1 % | 2, 8.8 / 8.9 % | 2, 8.8 / 8.9 % |
| dj_2022_grand_mere_battue | 2 | 2, 0.2 / 7.9 % | 2, 0.4 / 7.5 % | 3, 11.4 / 18.5 % | 2, 0.4 / 7.4 % | 2, 0.4 / 7.4 % |
| dj_2022_intox_med | 2 | 2, 0.3 / 4.1 % | 3, 6.5 / 9.0 % | 3, 5.6 / 8.1 % | 4, 8.1 / 10.6 % | 3, 5.1 / 7.6 % |
| dj_2023_coups | 2 | 2, 0.2 / 6.1 % | 5, 35.5 / 40.6 % | 2, 0.2 / 5.8 % | 2, 1.2 / 6.0 % | 2, 1.2 / 6.0 % |
| dj_2022_avc_16_ans | 2 | 2, 1.2 / 8.6 % | 3, 6.6 / 13.0 % | 2, 1.9 / 7.8 % | 2, 1.9 / 7.8 % | 2, 1.9 / 7.8 % |
| dj_2022_douleur_abdo | 2 | 2, 0.0 / 10.9 % | 3, 13.0 / 21.8 % | 4, 17.9 / 26.7 % | 3, 7.4 / 18.0 % | 3, 8.2 / 18.5 % |
| dj_2022_mere_fievre | 2 | 2, 0.1 / 4.6 % | 3, 8.3 / 12.2 % | 2, 0.1 / 4.1 % | 2, 0.1 / 4.1 % | 2, 0.1 / 4.1 % |
| nb_samtale_nb12 | 2 | 2, 0.0 / 7.0 % | 2, 0.0 / 7.0 % | 2, 0.0 / 7.0 % | 2, 0.0 / 7.0 % | 2, 0.0 / 7.0 % |

Mean confusion: Riksdag (5) main 57.9 %, 0.45 0.9 %, 0.48 3.0 %; out-of-domain (8) main 0.3 %,
0.45 10.0 %, 0.48 3.5 %.

**The change is a trade-off, not a free win.** It fixes the debates (1-2 speakers found for 4-7,
confusion 45-71 % down to 1-12 %). On the out-of-domain 2-speaker clips the previous code
(0.75 with the track cap) was better than any new setting: confusion 0.3 % mean versus 3.5 %
at 0.48, because a track cap is harmless when there really are two speakers. The new code
over-splits short telephone calls (`dj_2022_intox_med` 4 found for 2, `dj_2022_douleur_abdo` 3
for 2).

Speed: the analysis stage (segmentation + embeddings, 89 s for 53 min) is untouched and the
clustering stage stays at about 0.02 s on the 74 min debate, so cache reruns remain instant.

### Chair exclusion and speaker counts

Reference speeches under 5 s (chair announcements) are dropped and the time between speeches
(chair, votes) is not scored, so the chair is never in the reference. The hypothesis does contain
the chair as an extra cluster, and it costs nothing in DER because it falls outside the scored
region. Consequences: "found" is counted only on scored speech, so a correct result is truth (the
chair cluster has under 5 s of scored speech) or truth + 1; at 0.48 the debates show 6/6, 7/7
and 5/4, 8/7, 5/4. The criterion is lenient: it cannot see the chair being split in two, and
truth + 1 can also mean one real speaker was lost while the chair was kept, which is why
confusion is reported next to the speaker count (SoU40 at 0.48: 7 found, 11.6 % confusion).

### Choosing the default

Per-recording confusion, threshold swept 0.30-0.60 in steps of 0.02 (`MIN_SPEAKER_SECONDS` = 8 s):
the debates are flat (0.9 % mean confusion) up to 0.46 and degrade from 0.48 (SoU40 loses a
speaker, 11.6 % confusion) and lose speakers outright from 0.52 (SoU40 found below truth at 0.52;
SoU40 and SoU38 at 0.54). The out-of-domain clips
are the opposite: they over-split at 0.46 and below (mean confusion 10-21 %) and settle from 0.48
(3.5 %). There is no setting that is best on both: the sets have a cliff on opposite sides of
about 0.47.

Leave-one-debate-out on the five Riksdag debates (select the threshold that minimises mean
confusion on the other four, held-out result for the fifth). The training objective is flat
across a range, so the middle of the tied range is taken:

| Held-out | Tied range chosen from | Chosen | Held-out confusion / DER | Found / truth |
| --- | --- | ---: | --- | --- |
| SoU40 | 0.30-0.50 | 0.40 | 1.1 % / 11.2 % | 8 / 7 |
| JuU41 | 0.30-0.46 | 0.38 | 0.5 % / 16.1 % | 5 / 4 |
| SoU39 | 0.30-0.46 | 0.38 | 0.9 % / 11.5 % | 8 / 7 |
| SoU38 | 0.30-0.46 | 0.38 | 1.2 % / 15.8 % | 7 / 6 |
| UU24 | 0.30-0.46 | 0.38 | 0.9 % / 11.2 % | 5 / 4 |

On Riksdag alone the folds select 0.38-0.40 (the plateau is wide, 0.30-0.46, so this says the
debates do not discriminate within it; it says nothing about the upper side). Leave-one-recording-out
over all 13 recordings (Riksdag plus out-of-domain) selects the same range in every fold:
0.48-0.52 (mean held-out confusion 3.1 %; per-recording choices are 0.50 in all 13 folds after
taking the middle of the tied range, with SoU40 11.6 %, `dj_2022_feu` 8.8 % and
`dj_2022_douleur_abdo` 8.2 % the worst held-out results). The set-balanced mean confusion is
lowest at 0.50 (3.1) and 0.48 (3.2), versus 5.4 at 0.46 and 7.5 at 0.40.

The 0.40 proposed after review is therefore not supported: it is no better than 0.45 on the
debates and clearly worse on the clips (14.2 % mean confusion). **The default is 0.48**: the
lowest-confusion region of the pooled sweep, 0.02 below the point where the debates lose a second
speaker (0.52) and 0.02 above the out-of-domain over-split cliff (0.46). It costs the debates
confusion on SoU40 (1.1 % to 11.6 %) compared with 0.45. If Swedish debates are the only target,
0.45 is the better setting for them; the threshold is a single constant
(`DEFAULT_THRESHOLD`) plus three literals (transcribe CLI, reprocess CLI, reprocessing UI), kept
in sync by tests.

### `MIN_SPEAKER_SECONDS` sweep (0.48)

| Seconds | Riksdag conf / DER | Out-of-domain conf / DER | Exact count (13) |
| ---: | --- | --- | --- |
| 4 | 3.0 / 15.2 % | 6.9 / 12.8 % | 5 / 13 |
| 8 (kept) | 3.0 / 15.2 % | 3.5 / 8.7 % | 8 / 13 |
| 12 | 3.0 / 15.2 % | 6.3 / 11.3 % | 9 / 13 |
| 20 | 3.0 / 15.2 % | 10.0 / 14.7 % | 8 / 13 |

The debates are insensitive to it (every real speaker speaks far more than 20 s). On the clips 8 s
has the lowest confusion; 12 s finds the exact count more often but merges a real speaker in
one clip. Not clearly better, so it is unchanged. The same pattern holds at 0.50.

### Known limitations

- Short or telephone two-speaker audio can be over-split (`dj_2022_intox_med` 4 for 2,
  `dj_2022_douleur_abdo` 3 for 2 at the default). The earlier note that `dj_2022_feu` regressed
  from 8.9 % to 14.9 % (3 found for 2) was measured at threshold 0.45 before the merge targets were restricted to clusters of at least `MIN_SPEAKER_SECONDS`; with the
  current merge rule it is 2 for 2 / 8.9 % at 0.48 but 1 for 2 (43 % confusion) at 0.45, so it is
  on a cliff. The old code (0.75) handles these two-speaker clips better; a threshold that
  suits both domains does not exist with this embedding model.
- The out-of-domain set is two corpora: 7 simulated French emergency calls (one regulator and one
  caller on a phone line, 2 speakers each) and one Norwegian conversation built from per-turn
  clips. It contains no multi-party meeting, overlapping speech, or Swedish non-parliament audio.
- Existing meeting reviews will produce different speaker sets when reclustered with this
  build: the analysis cache stays valid (the clustering algorithm and its default threshold are
  not part of the cache identity), but recluster re-runs clustering with the new algorithm.
  Stored reviews built with the old track-capped algorithm map differently; the migration flow
  requires conflict review rather than auto-applying speaker renames or merges.

### Out-of-domain set

`docs/benchmarks/data/diarization-sv/manifest-other.json` records source URL, licence and SHA-256
per clip. Seven clips from [medkit/simsamu](https://huggingface.co/datasets/medkit/simsamu) (MIT
licence, simulated French medical dispatch calls with reference RTTMs, 8 kHz m4a) and
`nb_samtale_nb12` from [Sprakbanken/nb_samtale](https://huggingface.co/datasets/Sprakbanken/nb_samtale)
(CC0-1.0; Norwegian conversation rebuilt by `test-audio/diarization/fetch.sh`). Rebuild with
`scripts/diarization_sv/build_other.py`; audio stays in the scratch directory.

End to end through the CLI (`transcribe --diarize --meeting-json`, KB-Whisper Tiny, UU24, earlier
build at threshold 0.45): 5 speakers found, DER 5.0 % (miss 4.0 / confusion 1.0); the meeting JSON
only covers transcribed speech, which is why it scores below the raw diarization segments. The
full numbers above use the diarization-only helper because Whisper on 4 h of audio was impractical.

## Diagnosis

1. Embeddings are good. WeSpeaker cosine distance between segments labelled with the same
   speaker has median 0.15-0.17 (95th percentile 0.45-0.58); between different speakers the
   median is 0.68-0.71 and the 5th percentile 0.44-0.50 (SoU38 and SoU40, segments of at
   least 1.5 s). A threshold around 0.45-0.5 separates them; 0.75 sits above nearly all
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
  total speech (`MIN_SPEAKER_SECONDS`) into the nearest cluster that reaches that minimum, by
  centroid cosine (`cosine_similarity` normalises, so summed centroids are fine); if no cluster
  reaches it (very short audio) the smallest merges into the nearest other until one remains.
- Default threshold 0.75 -> 0.48 (core, `transcribe --diarize-threshold`, `meeting
  reprocess` CLI, reprocessing UI). Clustering stays threshold-only, so `--diarize-cache`
  reruns skip segmentation and embeddings as before.

## Reproduce

```bash
cd src-tauri && cargo build --release -p sagascript-core --features diarization --example diarize_eval
python scripts/diarization_sv/build_refs.py
python scripts/diarization_sv/eval_der.py --bin target/release/examples/diarize_eval \
  --scratch ~/.cache/sagascript-bench/diar-284 --thresholds 0.35,0.45,0.48,0.55,0.75
python scripts/diarization_sv/build_other.py
python scripts/diarization_sv/eval_der.py --bin target/release/examples/diarize_eval \
  --scratch ~/.cache/sagascript-bench/diar-284 --set other --thresholds 0.45,0.48 [--min-speaker 12]
# end to end, through the CLI (runs Whisper; slow):
python scripts/diarization_sv/eval_der.py --cli target/release/sagascript --scratch <dir> --ids UU24
```

`diarize_eval` (`crates/sagascript-core/examples/diarize_eval.rs`) runs the diarization
stages without Whisper so thresholds can be swept quickly.
