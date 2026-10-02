# Swedish diarization benchmark (issue #284)

Evaluation is **Swedish only** for now: the default clustering threshold is chosen from Swedish
recordings alone. English and other languages are **not yet evaluated**. A French and a Norwegian
set is kept as a clearly separated, non-gating secondary check.

All Swedish audio is Riksdag webb-tv (Riksdag open data: "free to use and redistribute with source
attribution", Källa: Sveriges riksdag). Speech start/length and speaker come from the page JSON of
each video. Manifests (URL, licence, SHA-256, durations, speaker names), reference RTTMs and the
complete sweep results (`grid.json`) are in `docs/benchmarks/data/diarization-sv/`. Audio is
downloaded to a scratch directory and is not in git.

## The Swedish set (14 recordings, plus telephone-band copies)

| Recording | Kind | Minutes | Speakers | Source |
| --- | --- | ---: | ---: | --- |
| SoU40 | plenary debate (committee report) | 74 | 7 | riksdagen.se webb-tv |
| JuU41 | plenary debate (committee report) | 49 | 4 | riksdagen.se webb-tv |
| SoU39 | plenary debate (committee report) | 59 | 7 | riksdagen.se webb-tv |
| SoU38 | plenary debate (committee report) | 53 | 6 | riksdagen.se webb-tv |
| UU24 | plenary debate (committee report) | 45 | 4 | riksdagen.se webb-tv |
| IP_hd10256 | interpellation | 24 | 2 | riksdagen.se webb-tv `hd10256` |
| IP_hc10606 | interpellation | 13 | 2 | riksdagen.se webb-tv `hc10606` |
| IP_hb10760 | interpellation | 15 | 2 | riksdagen.se webb-tv `hb10760` |
| IP_hb10618 | interpellation | 22 | 2 | riksdagen.se webb-tv `hb10618` |
| IP_hd10562 | interpellation | 25 | 3 | riksdagen.se webb-tv `hd10562` |
| IP_hd10115 | interpellation | 29 | 3 | riksdagen.se webb-tv `hd10115` |
| FS_20260611 | fragestund | 40 | 18 | riksdagen.se webb-tv `hdc120260611fs` |
| FS_20260604 | fragestund | 40 | 19 | riksdagen.se webb-tv `hdc120260604fs` |
| PL_20260610 | partiledardebatt | 37 | 6 | riksdagen.se webb-tv `hdc120260610pd` |

Interpellation debates give the two-speaker case (minister and member alternating; four debates),
plus two three-speaker ones. Question time (frågestund) has 18-19 speakers with short turns, the
party-leader debate six speakers. The first five rows are the original chamber debates, which were
also used to find the root cause and to sweep the threshold, so **they are in-sample**. SoU40 is
the full debate; the others over 40-60 min are cropped after a complete speech.

Looked for and not available: open committee hearings and seminars (`sam-st`, `sam-se`) and press
conferences (`sam-pk`) are on webb-tv, but their page JSON carries no speaker or turn metadata, so
there is no reliable reference. No public Swedish speech corpus with dialogue turns was pursued.
Everything here is parliamentary speech (chamber microphones); no Swedish recording from another
room, a conversational meeting, or a real phone call is included.

Rebuild: `scripts/diarization_sv/build_refs.py` (the five debates), `build_refs_extra.py` (the rest),
`build_degraded.py` (telephone-band copies), `build_other.py` (secondary set).

## Reference and metric

- One reference turn per speech, labelled by speaker. Speeches under 5 s are dropped. Time between
  speeches (chair, votes) is unlabelled and **not scored**: the scored region is the union of
  speech intervals, with a 0.25 s collar around turn boundaries.
- **Confusion** (speech given to the wrong speaker; the part clustering controls) is the headline.
  DER = (missed + false alarm + confusion) / reference speech, frame based (10 ms), optimal 1:1
  speaker mapping. Missed speech comes from the segmentation model and is a 5-15 % floor on every
  Swedish recording at every threshold, so DER alone hides the clustering effect.
- Speakers "found" counts hypothesis speakers with at least 5 s of scored speech.
- **Chair.** The chair is not in the reference, so a hypothesis chair cluster costs nothing and
  "found" is truth or truth + 1. That is not accepted automatically: `eval_der.py` reports, for each
  unmatched hypothesis cluster, how much of its speech lies outside every reference speech. At the
  default, the extra clusters on the Swedish set have 56-100 % of their speech outside reference
  speech, except SoU38 (45 %) and the second, 11 s extra cluster of IP_hb10618 (28 %); part of
  those clusters is scored speech of real speakers, so truth + 1 is mostly, not always, the chair. The criterion cannot
  see a chair split in two (IP_hb10618 shows two extra clusters: 4 found for 2).

## Results (Swedish, per recording)

Cells are speakers found / truth, then confusion / DER. `main code, 0.75` is the code and default
before this PR; the other columns are this PR's code at the previous PR default 0.48 and at the new
default 0.36.

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 (default) |
| --- | --- | --- | --- |
| JuU41 | 1/4, 53.2 / 68.8 % | 5/4, 0.5 / 16.1 % | 5/4, 0.5 / 16.1 % |
| SoU38 | 2/6, 53.3 / 68.0 % | 6/6, 1.0 / 15.6 % | 7/6, 1.2 / 15.8 % |
| SoU39 | 1/7, 67.3 / 77.9 % | 8/7, 0.9 / 11.6 % | 8/7, 0.9 / 11.6 % |
| SoU40 | 1/7, 71.1 / 81.2 % | 7/7, 11.6 / 21.8 % | 8/7, 1.1 / 11.2 % |
| UU24 | 2/4, 44.6 / 54.9 % | 5/4, 0.9 / 11.2 % | 5/4, 0.9 / 11.2 % |
| FS_20260604 | 1/19, 77.7 / 82.4 % | 19/19, 7.4 / 12.2 % | 20/19, 5.5 / 10.3 % |
| FS_20260611 | 2/18, 64.0 / 72.0 % | 18/18, 5.5 / 13.4 % | 19/18, 3.8 / 11.7 % |
| IP_hb10618 | 1/2, 46.0 / 51.3 % | 4/2, 1.3 / 6.6 % | 4/2, 1.3 / 6.6 % |
| IP_hb10760 | 1/2, 40.2 / 50.4 % | 3/2, 1.0 / 11.2 % | 3/2, 1.0 / 11.2 % |
| IP_hc10606 | 1/2, 38.0 / 45.7 % | 3/2, 2.4 / 10.1 % | 3/2, 2.4 / 10.1 % |
| IP_hd10115 | 1/3, 44.6 / 54.0 % | 4/3, 0.9 / 10.2 % | 4/3, 0.9 / 10.2 % |
| IP_hd10256 | 1/2, 42.7 / 47.3 % | 3/2, 1.0 / 5.6 % | 3/2, 1.0 / 5.6 % |
| IP_hd10562 | 1/3, 48.8 / 57.3 % | 3/3, 0.2 / 8.8 % | 3/3, 0.2 / 8.8 % |
| PL_20260610 | 1/6, 68.7 / 82.3 % | 6/6, 0.0 / 13.7 % | 6/6, 0.0 / 13.7 % |

Mean confusion over the 14 recordings: main code at 0.75 about 54 %; new code at 0.48 2.5 %; at
0.36 1.5 %. On the two-speaker interpellations the old code found **one** speaker for two (38-46 %
confusion): the track cap fixed here is not only a many-speaker problem. At 0.36 the extra speaker
is mostly the chair in the interpellations and debates; question time over-splits by one (19-20 for
18-19).

### Telephone-band robustness (Swedish clips band-limited to 300-3400 Hz, 8 kHz)

Stand-in for call audio until real Swedish call recordings exist; reference RTTMs are those of the
source clips (`build_degraded.py`; ffmpeg band-pass, not a real codec or phone line). Separate row,
not part of the selection.

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 (default) |
| --- | --- | --- | --- |
| FS_20260611_tel | 1/18, 65.1 / 74.6 % | 16/18, 12.3 / 21.9 % | 20/18, 4.6 / 14.1 % |
| IP_hc10606_tel | 1/2, 37.5 / 47.3 % | 3/2, 2.4 / 12.1 % | 3/2, 2.4 / 12.1 % |
| IP_hd10256_tel | 1/2, 42.0 / 48.0 % | 3/2, 1.2 / 7.3 % | 3/2, 1.2 / 7.3 % |
| IP_hd10562_tel | 1/3, 47.9 / 58.1 % | 2/3, 32.2 / 42.5 % | 2/3, 32.2 / 42.5 % |
| SoU38_tel | 1/6, 64.9 / 81.7 % | 6/6, 1.1 / 17.9 % | 7/6, 1.3 / 18.1 % |
| UU24_tel | 2/4, 54.0 / 66.6 % | 5/4, 0.9 / 13.5 % | 5/4, 0.9 / 13.5 % |

Mean confusion at 0.36 is 7.1 % (the clean sources of the same clips: 1.6 %), driven by one
recording: `IP_hd10562_tel` finds 2 speakers for 3 (32 % confusion) at 0.36 and above, but 3 found
with 0.2 % confusion at 0.34 and below (mean over the six: about 1.7 %). The telephone-band set prefers 0.34 or lower. The default was not
selected on it, but **0.36 sits at that cliff for band-limited audio**; this is the main open risk
for call audio.

## Choosing the default (Swedish only)

Selection: leave-one-recording-out over the 14 clean Swedish recordings, objective mean confusion,
thresholds 0.10-0.70 in steps of 0.02 plus 0.75, `MIN_SPEAKER_SECONDS` 8 s. The near-tie tolerance is
0.25 percentage points of mean confusion (defined in `select_default.py`); a fold chooses the median
of the thresholds within tolerance of its minimum. The tables below are generated by
`scripts/diarization_sv/select_default.py docs/benchmarks/data/diarization-sv/grid.json` from the
committed sweep results.

### Leave-one-recording-out over 14 Swedish recordings (variant `shipped`, near-tie 0.25 pp)

| Held-out | Tied range (training) | Chosen | Held-out confusion |
| --- | --- | ---: | ---: |
| JuU41 | 0.26-0.46 | 0.36 | 0.5 % |
| SoU38 | 0.26-0.46 | 0.36 | 1.2 % |
| SoU39 | 0.22-0.46 | 0.34 | 0.9 % |
| SoU40 | 0.26-0.46 | 0.36 | 1.1 % |
| UU24 | 0.26-0.46 | 0.36 | 0.9 % |
| FS_20260604 | 0.26-0.46 | 0.36 | 5.5 % |
| FS_20260611 | 0.26-0.46 | 0.36 | 3.8 % |
| IP_hb10618 | 0.26-0.46 | 0.36 | 1.3 % |
| IP_hb10760 | 0.26-0.46 | 0.36 | 1.0 % |
| IP_hc10606 | 0.26-0.46 | 0.36 | 2.4 % |
| IP_hd10115 | 0.26-0.46 | 0.36 | 0.9 % |
| IP_hd10256 | 0.26-0.46 | 0.36 | 1.0 % |
| IP_hd10562 | 0.26-0.46 | 0.36 | 0.2 % |
| PL_20260610 | 0.26-0.46 | 0.36 | 0.0 % |

Sweep of the mean (selection set):

Mean held-out confusion 1.47 %.

| Threshold | mean confusion (selection set) |
| ---: | ---: |
| 0.10 | 14.88 % |
| 0.12 | 10.20 % |
| 0.14 | 6.55 % |
| 0.16 | 4.62 % |
| 0.18 | 3.70 % |
| 0.20 | 2.18 % |
| 0.22 | 1.81 % |
| 0.24 | 1.73 % |
| 0.26 | 1.52 % |
| 0.28 | 1.50 % |
| 0.30 | 1.47 % |
| 0.32 | 1.47 % |
| 0.34 | 1.47 % |
| 0.36 | 1.47 % |
| 0.38 | 1.47 % |
| 0.40 | 1.47 % |
| 0.42 | 1.47 % |
| 0.44 | 1.47 % |
| 0.46 | 1.60 % |
| 0.48 | 2.46 % |
| 0.50 | 3.77 % |
| 0.52 | 4.74 % |
| 0.54 | 8.74 % |
| 0.56 | 11.18 % |
| 0.58 | 13.39 % |
| 0.60 | 14.57 % |
| 0.62 | 15.95 % |
| 0.64 | 18.42 % |
| 0.66 | 25.05 % |
| 0.68 | 30.19 % |
| 0.70 | 34.88 % |
| 0.75 | 49.33 % |

The Swedish data show a flat plateau: mean confusion is 1.5 % for every threshold from 0.26 to 0.46,
rises at 0.48 (SoU40 loses a speaker: 1.1 % to 11.6 % confusion) and again from 0.50, and below 0.26
recordings over-split (0.22 and lower). The default is the plateau midpoint, **0.36**, with 0.10 of
margin on both sides. The 0.48 chosen earlier from a pooled Swedish/French/Norwegian sweep sat at
the plateau edge for Swedish. The French and Norwegian clips did not influence the choice.

### Absorption of small clusters and `MIN_SPEAKER_SECONDS`

`absorb_small_clusters` merges a cluster with under `MIN_SPEAKER_SECONDS` (8 s) of speech into the
nearest cluster that reaches 8 s, **only if** their centroid cosine distance is at most
`ABSORB_MAX_DISTANCE` (0.75); otherwise it stays a distinct speaker, and short clusters are never
collapsed into one speaker unconditionally (unit tests cover clearly separated short speakers).
Variants swept: `legacy` = no distance limit, `d50`/`d60`/`d90` = other limits, `ms4`/`ms12`/`ms20` =
other minimum seconds; each cell is mean confusion / DER and the number of recordings with truth
or truth + 1 speakers found.

### Variants at threshold 0.36

| Variant | Swedish (14) conf / DER, count ok | Telephone-band (6) | French/Norwegian (8, non-gating) |
| --- | --- | --- | --- |
| d50 | 1.5 / 11.1 %, 13/14 | 7.2 / 18.0 %, 4/6 | 15.3 / 21.2 %, 7/8 |
| d60 | 1.5 / 11.0 %, 13/14 | 7.1 / 18.0 %, 4/6 | 14.1 / 19.6 %, 7/8 |
| d90 | 1.5 / 11.0 %, 13/14 | 7.1 / 17.9 %, 4/6 | 14.5 / 19.6 %, 6/8 |
| legacy | 1.5 / 11.0 %, 13/14 | 7.1 / 17.9 %, 4/6 | 15.0 / 20.1 %, 6/8 |
| ms12 | 1.4 / 11.0 %, 14/14 | 7.0 / 17.8 %, 5/6 | 9.5 / 14.6 %, 7/8 |
| ms20 | 1.4 / 11.0 %, 14/14 | 7.0 / 17.8 %, 5/6 | 6.4 / 11.7 %, 7/8 |
| ms4 | 1.5 / 11.0 %, 13/14 | 7.2 / 18.0 %, 2/6 | 23.9 / 29.7 %, 3/8 |
| shipped | 1.5 / 11.0 %, 13/14 | 7.1 / 17.9 %, 4/6 | 14.2 / 19.3 %, 6/8 |

On the Swedish set none of them changes confusion by more than 0.1 pp (1.4-1.5 %). 12 and 20 s find
the right speaker count on 14/14 instead of 13/14 and are better on the secondary set, but the gain
is one recording and no Swedish recording has a real speaker under 20 s, so what the larger value
costs real short speakers is unmeasured; 8 s and 0.75 are kept. No reference speaker in any set
speaks for under 8 s, so the distance-limit behaviour is covered by unit tests, not by data.

## Secondary check (non-gating): French and Norwegian two-speaker clips

Not used for the choice. Seven simulated French emergency-dispatch calls
([medkit/simsamu](https://huggingface.co/datasets/medkit/simsamu), MIT, 8 kHz phone line) and
`nb_samtale_nb12` ([Sprakbanken/nb_samtale](https://huggingface.co/datasets/Sprakbanken/nb_samtale),
CC0-1.0, per-turn clips concatenated by `test-audio/diarization/fetch.sh`; its two reference
speakers overlap for about 2 s). Manifest and licences: `manifest-other.json`.

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 (default) |
| --- | --- | --- | --- |
| dj_2022_avc_16_ans | 2/2, 1.2 / 8.6 % | 2/2, 1.9 / 8.0 % | 3/2, 7.3 / 13.3 % |
| dj_2022_douleur_abdo | 2/2, 0.0 / 10.9 % | 3/2, 8.3 / 19.0 % | 3/2, 13.3 / 22.3 % |
| dj_2022_feu | 2/2, 0.0 / 4.0 % | 2/2, 10.9 / 10.9 % | 1/2, 39.3 / 39.4 % |
| dj_2022_grand_mere_battue | 2/2, 0.2 / 7.8 % | 2/2, 0.8 / 7.9 % | 3/2, 15.4 / 22.5 % |
| dj_2022_intox_med | 2/2, 0.3 / 4.1 % | 4/2, 9.0 / 11.5 % | 3/2, 6.8 / 9.3 % |
| dj_2022_mere_fievre | 2/2, 0.1 / 4.5 % | 2/2, 0.7 / 5.1 % | 2/2, 0.7 / 5.1 % |
| dj_2023_coups | 2/2, 0.2 / 6.1 % | 2/2, 1.4 / 6.6 % | 5/2, 30.8 / 36.0 % |
| nb_samtale_nb12 | 2/2, 0.0 / 7.0 % | 2/2, 0.0 / 7.0 % | 2/2, 0.0 / 7.0 % |

The Swedish-selected default **0.36 over-splits and merges these clips** (mean confusion 14.2 %,
versus 4.1 % at 0.48 and 0.3 % for the old code at 0.75): `dj_2022_feu` finds one speaker for two
and `dj_2023_coups` five for two. Simulated French dispatch calls are not Swedish call audio and
the embedding model is the same, so this is a warning, not a measurement of Swedish phone behaviour.

## Known limitations

- Parliamentary speech only; no Swedish conversational meeting, different room, overlapping
  speech or real phone audio. English and other languages are not evaluated.
- Band-limited audio: the cliff at 0.36 above. Short two-speaker phone-style audio can over-split.
- Existing meeting reviews will produce different speaker sets when reclustered with this build:
  the analysis cache stays valid (the clustering algorithm and default threshold are not part of the
  cache identity), but recluster re-runs clustering with the new algorithm. Stored reviews from the
  track-capped algorithm map differently; the migration flow requires conflict review rather than
  auto-applying speaker renames or merges.
- Zero or non-finite embeddings are unusable and fall back to their pyannote track instead of
  forming speakers (unit-tested).

## Diagnosis

1. Embeddings are good. WeSpeaker cosine distance between segments labelled with the same
   speaker has median 0.15-0.17 (95th percentile 0.45-0.58); between different speakers the
   median is 0.68-0.71 and the 5th percentile 0.44-0.50 (SoU38 and SoU40, segments of at
   least 1.5 s). 0.75 sits above nearly all different-speaker pairs, so it merges everything.
2. The threshold barely mattered on main because of the stabilisation step. Window stitching
   produces only 2-3 global tracks on a turn-taking debate (SoU38: 2 tracks for 6 speakers) and
   every track was forced into a single embedding cluster (`stabilize_clusters_by_track`,
   `diarization/mod.rs`). The number of speakers could never exceed the number of tracks, so
   lowering the threshold changed nothing (SoU38: 2 speakers at every threshold from 0.3 to 0.75),
   matching the report in #284.
3. Without that cap, clustering over-splits short noisy segments into small spurious clusters.

## Change

- `diarization/mod.rs`: each embedded segment keeps its own embedding cluster
  (`assign_speaker_labels`); tracks are only a fallback label for segments too short to embed or
  whose embedding is unusable.
- `diarization/clustering.rs`: `absorb_small_clusters` (see above), aggregates computed once.
- Default threshold 0.75 -> 0.36 (`DEFAULT_THRESHOLD` in core; the `transcribe --diarize-threshold`,
  `meeting reprocess` CLI and reprocessing UI literals are pinned by tests). Clustering stays
  threshold-only, so `--diarize-cache` reruns skip segmentation and embeddings as before. Speed:
  analysis (89 s for 53 min) is untouched; clustering stays about 0.02 s on the 74 min debate.

## Reproduce

```bash
cd src-tauri && cargo build --release -p sagascript-core --features diarization --example diarize_eval
python scripts/diarization_sv/build_refs.py && python scripts/diarization_sv/build_refs_extra.py
python scripts/diarization_sv/build_degraded.py && python scripts/diarization_sv/build_other.py
# One set, explicit thresholds (default: the shipped DEFAULT_THRESHOLD read from core).
# --tag names the cached analysis; use a new tag if the analysis stage changes.
python scripts/diarization_sv/eval_der.py --bin target/release/examples/diarize_eval \
  --scratch ~/.cache/sagascript-bench/diar-284 --set sv2 --tag main --thresholds 0.36,0.48
# Variants: --min-speaker 12, --absorb-max-distance 2.0 (no limit). Collect sweeps into grid.json with
# collect_grid.py; select_default.py prints the leave-one-out, per-recording and variant tables.
# End to end through the CLI (runs Whisper; slow), explicit threshold:
python scripts/diarization_sv/eval_der.py --cli target/release/sagascript --scratch <dir> --ids UU24 --thresholds 0.36
```

`diarize_eval` (`crates/sagascript-core/examples/diarize_eval.rs`) runs the diarization stages
without Whisper: `analyze <audio> <analysis.json>` and
`cluster <analysis.json> <threshold> <segments.json> [min_speaker_seconds] [absorb_max_distance]`.
