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
  default, the extra clusters on the Swedish set have 57-100 % of their speech outside reference
  speech, except SoU38 (45 %) and the second, 11 s extra cluster of IP_hb10618 (29 %); part of
  those clusters is scored speech of real speakers, so truth + 1 is mostly, not always, the chair. The criterion cannot
  see a chair split in two (IP_hb10618 shows two extra clusters: 4 found for 2).

## Results (Swedish, per recording)

Cells are speakers found / truth, then confusion / DER. `main code, 0.75` is the code and default
before this PR; the other columns are this PR's code at the earlier PR default 0.48, at the
script-selected midpoint 0.36 and at the shipped default **0.34** (owner decision, see "Choosing the
default").

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 | new code, 0.34 |
| --- | --- | --- | --- | --- |
| JuU41 | 1/4, 53.2 / 68.8 % | 5/4, 0.5 / 16.1 % | 5/4, 0.5 / 16.1 % | 5/4, 0.5 / 16.1 % |
| SoU38 | 2/6, 53.3 / 68.0 % | 6/6, 1.0 / 15.6 % | 7/6, 1.2 / 15.8 % | 7/6, 1.2 / 15.8 % |
| SoU39 | 1/7, 67.3 / 77.9 % | 8/7, 0.9 / 11.6 % | 8/7, 0.9 / 11.6 % | 8/7, 0.9 / 11.6 % |
| SoU40 | 1/7, 71.1 / 81.2 % | 7/7, 11.6 / 21.8 % | 8/7, 1.1 / 11.2 % | 8/7, 1.1 / 11.2 % |
| UU24 | 2/4, 44.6 / 54.9 % | 5/4, 0.9 / 11.2 % | 5/4, 0.9 / 11.2 % | 5/4, 0.9 / 11.2 % |
| FS_20260604 | 1/19, 77.7 / 82.4 % | 19/19, 7.4 / 12.2 % | 20/19, 5.5 / 10.3 % | 20/19, 5.5 / 10.3 % |
| FS_20260611 | 2/18, 64.0 / 72.0 % | 18/18, 5.5 / 13.4 % | 19/18, 3.8 / 11.7 % | 19/18, 3.8 / 11.7 % |
| IP_hb10618 | 1/2, 46.0 / 51.3 % | 4/2, 1.3 / 6.6 % | 4/2, 1.3 / 6.6 % | 4/2, 1.3 / 6.6 % |
| IP_hb10760 | 1/2, 40.2 / 50.4 % | 3/2, 1.0 / 11.2 % | 3/2, 1.0 / 11.2 % | 3/2, 1.0 / 11.2 % |
| IP_hc10606 | 1/2, 38.0 / 45.7 % | 3/2, 2.4 / 10.1 % | 3/2, 2.4 / 10.1 % | 3/2, 2.4 / 10.1 % |
| IP_hd10115 | 1/3, 44.6 / 54.0 % | 4/3, 0.9 / 10.2 % | 4/3, 0.9 / 10.2 % | 4/3, 0.9 / 10.2 % |
| IP_hd10256 | 1/2, 42.7 / 47.3 % | 3/2, 1.0 / 5.6 % | 3/2, 1.0 / 5.6 % | 3/2, 1.0 / 5.6 % |
| IP_hd10562 | 1/3, 48.8 / 57.3 % | 3/3, 0.2 / 8.8 % | 3/3, 0.2 / 8.8 % | 3/3, 0.2 / 8.8 % |
| PL_20260610 | 1/6, 68.7 / 82.3 % | 6/6, 0.0 / 13.7 % | 6/6, 0.0 / 13.7 % | 6/6, 0.0 / 13.7 % |

Mean confusion over the 14 recordings: main code at 0.75 about 54 %; new code at 0.48 2.5 %; at
0.36 and 0.34 both 1.5 %. On the two-speaker interpellations the old code found **one** speaker for two (38-46 %
confusion): the track cap fixed here is not only a many-speaker problem. At 0.34 the extra speaker
is mostly the chair in the interpellations and debates; question time over-splits by one (19-20 for
18-19).

### Telephone-band robustness (Swedish clips band-limited to 300-3400 Hz, 8 kHz)

Stand-in for call audio until real Swedish call recordings exist; reference RTTMs are those of the
source clips (`build_degraded.py`; ffmpeg band-pass, not a real codec or phone line). Separate row,
not part of the selection.

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 | new code, 0.34 |
| --- | --- | --- | --- | --- |
| FS_20260611_tel | 1/18, 65.1 / 74.6 % | 16/18, 12.3 / 21.9 % | 20/18, 4.6 / 14.1 % | 20/18, 4.6 / 14.1 % |
| IP_hc10606_tel | 1/2, 37.5 / 47.3 % | 3/2, 2.4 / 12.1 % | 3/2, 2.4 / 12.1 % | 3/2, 2.4 / 12.1 % |
| IP_hd10256_tel | 1/2, 42.0 / 48.0 % | 3/2, 1.2 / 7.3 % | 3/2, 1.2 / 7.3 % | 3/2, 1.2 / 7.3 % |
| IP_hd10562_tel | 1/3, 47.9 / 58.1 % | 2/3, 32.2 / 42.5 % | 2/3, 32.2 / 42.5 % | 3/3, 0.2 / 10.5 % |
| SoU38_tel | 1/6, 64.9 / 81.7 % | 6/6, 1.1 / 17.9 % | 7/6, 1.3 / 18.1 % | 7/6, 1.3 / 18.1 % |
| UU24_tel | 2/4, 54.0 / 66.6 % | 5/4, 0.9 / 13.5 % | 5/4, 0.9 / 13.5 % | 5/4, 0.9 / 13.5 % |

Mean confusion on the band-limited set is 1.76 % at 0.34 and 7.08 % at 0.36 (the clean sources of the
same clips: 1.6 %), driven by one recording: `IP_hd10562_tel` finds 3 of 3 speakers with 0.20 %
confusion at 0.34 and below, but 2 of 3 with 32.16 % at 0.36 and above. **0.34 is the last value
before that edge**, so it has no margin on the band-limited side; real call audio could sit on the
other side of it. This is the main open risk for call audio.

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
| UU24 | 0.24-0.46 | 0.36 | 0.9 % |
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
| 0.10 | 14.89 % |
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

Generated by `select_default.py ... --shipped 0.34`:

Full-set minimum 1.47 %; near-tie plateau 0.26-0.46; midpoint 0.36; margins 0.10 below, 0.10 above.

Script pick: 0.36. Shipped default: 0.34 (owner decision); margins on the clean set 0.08 below, 0.12 above; mean confusion there 1.47 %.

The Swedish data show a flat plateau: mean confusion is 1.5 % for every threshold from 0.26 to 0.46,
rises at 0.48 (SoU40 loses a speaker: 1.1 % to 11.6 % confusion) and again from 0.50, and below 0.26
recordings over-split (0.22 and lower). `select_default.py` picks the plateau midpoint 0.36 (0.10 of margin on both sides). The 0.48 chosen earlier from a pooled Swedish/French/Norwegian sweep sat at
the plateau edge for Swedish.

**The shipped default is 0.34, by owner decision**, not the script's 0.36. Reasons: on the clean
Swedish set every value from 0.30 to 0.44 gives the same mean confusion (Riksdag 0.9 %, new
recordings 1.78 %), so 0.36 was only the midpoint and 0.34 costs nothing measurable there; on the
band-limited Swedish copies 0.34 gives 1.76 % mean confusion against 7.08 % at 0.36; and
over-splitting is the cheaper error to correct in review. Margins at 0.34: 0.08 below (plateau
starts at 0.26) and 0.12 above on clean Swedish; none above on band-limited audio (see above). The
secondary French/Norwegian set barely moves between the two (mean confusion 14.0 % vs 13.0 %) and
did not influence the choice.

### Absorption of small clusters and `MIN_SPEAKER_SECONDS`

`absorb_small_clusters` merges a cluster with under `MIN_SPEAKER_SECONDS` (8 s) of speech into the
nearest cluster that reaches 8 s, **only if** their centroid cosine distance is at most
`ABSORB_MAX_DISTANCE` (0.75); otherwise it stays a distinct speaker, and short clusters are never
collapsed into one speaker unconditionally (unit tests cover clearly separated short speakers).
A rejected small cluster is reconsidered after every merge (a merge moves the target centroid).
Re-running the sweep after adding that changed no Swedish per-recording confusion at 0.36 or 0.48
(the secondary set did change: mean confusion at 0.36 14.2 % -> 13.0 %, 14.0 % at 0.34).
Variants swept: `legacy` = no distance limit, `d50`/`d60`/`d90` = other limits, `ms4`/`ms12`/`ms20` =
other minimum seconds; each cell is mean confusion / DER and the number of recordings with truth
or truth + 1 speakers found.

### Variants at threshold 0.34

| Variant | Swedish (14) conf / DER, count ok | Telephone-band (6) | French/Norwegian (8, non-gating) |
| --- | --- | --- | --- |
| d50 | 1.5 / 11.1 %, 13/14 | 1.8 / 12.7 %, 5/6 | 15.4 / 21.9 %, 6/8 |
| d60 | 1.5 / 11.0 %, 13/14 | 1.8 / 12.7 %, 5/6 | 13.7 / 19.7 %, 6/8 |
| d90 | 1.5 / 11.0 %, 13/14 | 1.8 / 12.6 %, 5/6 | 14.9 / 20.0 %, 5/8 |
| legacy | 1.5 / 11.0 %, 13/14 | 1.8 / 12.6 %, 5/6 | 14.9 / 20.0 %, 5/8 |
| ms12 | 1.4 / 11.0 %, 14/14 | 1.6 / 12.5 %, 6/6 | 9.6 / 14.8 %, 7/8 |
| ms20 | 1.4 / 11.0 %, 14/14 | 1.6 / 12.5 %, 6/6 | 6.3 / 11.6 %, 7/8 |
| ms4 | 1.5 / 11.0 %, 13/14 | 1.9 / 12.7 %, 3/6 | 24.2 / 30.0 %, 3/8 |
| shipped | 1.5 / 11.0 %, 13/14 | 1.8 / 12.6 %, 5/6 | 14.0 / 19.2 %, 5/8 |

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

| Recording | main code, 0.75 | new code, 0.48 | new code, 0.36 | new code, 0.34 |
| --- | --- | --- | --- | --- |
| dj_2022_avc_16_ans | 2/2, 1.2 / 8.6 % | 2/2, 1.9 / 8.0 % | 3/2, 7.3 / 13.3 % | 3/2, 7.3 / 13.3 % |
| dj_2022_douleur_abdo | 2/2, 0.0 / 10.9 % | 3/2, 7.8 / 18.6 % | 3/2, 13.3 / 22.3 % | 3/2, 12.1 / 21.2 % |
| dj_2022_feu | 2/2, 0.0 / 4.0 % | 2/2, 10.9 / 10.9 % | 1/2, 41.0 / 41.0 % | 1/2, 41.0 / 41.0 % |
| dj_2022_grand_mere_battue | 2/2, 0.2 / 7.8 % | 2/2, 0.8 / 7.9 % | 3/2, 4.5 / 11.6 % | 4/2, 10.3 / 17.3 % |
| dj_2022_intox_med | 2/2, 0.3 / 4.1 % | 4/2, 9.2 / 11.7 % | 3/2, 6.8 / 9.3 % | 3/2, 5.9 / 8.4 % |
| dj_2022_mere_fievre | 2/2, 0.1 / 4.5 % | 2/2, 0.5 / 4.8 % | 2/2, 0.5 / 4.8 % | 3/2, 5.2 / 9.6 % |
| dj_2023_coups | 2/2, 0.2 / 6.1 % | 2/2, 0.7 / 6.5 % | 5/2, 30.7 / 35.9 % | 5/2, 30.7 / 35.9 % |
| nb_samtale_nb12 | 2/2, 0.0 / 7.0 % | 2/2, 0.0 / 7.0 % | 2/2, 0.0 / 7.0 % | 2/2, 0.0 / 7.0 % |

The shipped default **0.34 over-splits and merges these clips** (mean confusion 14.0 %, 13.0 % at
0.36, 4.0 % at 0.48 and 0.3 % for the old code at 0.75): `dj_2022_feu` finds one speaker for two
and `dj_2023_coups` five for two. Simulated French dispatch calls are not Swedish call audio and
the embedding model is the same, so this is a warning, not a measurement of Swedish phone behaviour.

## Known limitations

- Parliamentary speech only; no Swedish conversational meeting, different room, overlapping
  speech or real phone audio. English and other languages are not evaluated.
- Band-limited audio: the edge just above 0.34 on band-limited audio (see above). Short two-speaker phone-style audio can over-split.
- Existing meeting reviews will produce different speaker sets when reclustered with this build:
  the analysis cache stays valid (the clustering algorithm and default threshold are not part of the
  cache identity), but recluster re-runs clustering with the new algorithm. Stored reviews from the
  track-capped algorithm map differently; the migration flow requires conflict review rather than
  auto-applying speaker renames or merges.
- Embedding robustness: zero or degenerate (near-zero norm) vectors fall back to their pyannote
  track instead of forming speakers (unit-tested). Persisted analyses with non-finite components
  are rejected by validation; live inference sanitises non-finite components.
- All 14 Swedish recordings share one recording environment (the chamber) and some share
  speakers (`IP_hb10760` and `IP_hc10606` have the same two speakers), so leave-one-recording-out
  does not show generalisation to unseen rooms or speakers. SoU40 supplies about 75 % of the mean
  improvement from 0.48 to 0.36, and the two question-time recordings supply about 45 % of the
  remaining confusion at 0.36 or 0.34. On the secondary set only 1 of 8 clips (2 of 8 at 0.36) get exactly the reference
  speaker count.

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
  whose embedding is degenerate (zero norm).
- `diarization/clustering.rs`: `absorb_small_clusters` (see above), aggregates computed once.
- Default threshold 0.75 -> 0.34 (`DEFAULT_THRESHOLD` in core; the `transcribe --diarize-threshold`,
  `meeting reprocess` CLI and reprocessing UI literals are pinned by tests). Clustering stays
  threshold-only, so `--diarize-cache` reruns skip segmentation and embeddings as before. Speed:
  analysis (89 s for 53 min) is untouched; clustering stays about 0.02 s on the 74 min debate.

## Speaker-count hint (issue #305, part 1)

`transcribe --diarize` and `meeting reprocess plan` accept `--speakers N`, or `--min-speakers N`
and/or `--max-speakers N`, plus `--force-speakers N`. Help text: count everyone who speaks,
including a chair or moderator; if unsure, use `--min-speakers`. Without a hint nothing changes:
the output is byte-identical to the threshold-only code on all 28 cached analyses (`cmp` against
the pre-hint `diarize_eval`) and a golden test pins the full serialized output of a fixture
produced by the pre-hint build.

**Rules.** The normal pipeline (threshold cut, then `absorb_small_clusters`) runs first. If its
speaker count already satisfies the hint, nothing changes.

- *Too few found* (below an exact N or `min`): the dendrogram is walked from the root to the first
  cut with N clusters of at least 8 s (`MIN_SPEAKER_SECONDS`) and the smaller clusters are absorbed
  into the nearest large one. Counting clusters rather than segments matters: a plain "cut into N
  clusters" on a 7-speaker debate gave one giant cluster plus six one-segment outliers (1 speaker
  found, 71 % confusion on SoU40; the first version of this change). N above the number of embedded
  segments is capped.
- *Too many found* (above an exact N or `max`): clusters under 8 s are folded into their nearest
  large cluster as far as the count needs, then the least-heard cluster is merged into its nearest
  neighbour, only across centroid distances up to `hint_merge_max_distance` (**0.9**). If the
  least-heard cluster has no neighbour within the limit the attempt is abandoned and nothing real is
  merged: the extra speakers stay, stderr says "requested N, delivered M", and the meeting JSON
  records `speaker_hint_satisfied: false` and `speaker_hint_delivered`. The first versions of this
  limit skipped to better-heard clusters or merged part-way, which merged real speakers; the
  least-heard-only, all-or-nothing rule is what removed that harm.
- `--force-speakers N` (an exact count with `force`) merges regardless of distance and can put
  different people under one name. It uses the same dendrogram cut as raising the count.
- Tracks with no embedded segment count against the hint; if that, too little data or the limit
  makes the hint unreachable the delivered count is reported the same way (tests cover
  unembedded tracks at or above the bound and all-unembedded input).
- Only clustering is affected, so `--diarize-cache` reruns and `meeting reprocess` recluster stay
  cheap (SoU40, 309 embeddings: 11 ms without a hint, 24 ms when the walk runs; the walk is
  O(n^2 log n) in the number of embedded segments and not a concern at these sizes). The hint and
  its outcome are recorded in the meeting JSON and the hint in the reprocessing plan.

**Evaluation.** Cached analyses, threshold 0.34, `eval_der.py --hint`; per-recording results and
parameters are committed in `data/diarization-sv/hint-grid.json` (code commit in its `meta`), and
these tables are printed by `collect_hint_grid.py tables`. T is the manifest's
`reference_speakers`, which does not count the chair; "exact" is speakers found (at least 5 s of
scored speech) equal to T. Each cell is exact count, mean confusion / mean DER.

| Variant | Riksdag debates (5) | Other Riksdag (9) | Swedish, band-limited (6) | French/Norwegian calls (8) |
| --- | --- | --- | --- | --- |
| threshold only | 0/5, 0.9 % / 13.2 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.8 % / 12.6 % | 1/8, 14.0 % / 19.2 % |
| forced exact T | 4/5, 7.3 % / 19.5 % | 6/9, 20.6 % / 28.6 % | 4/6, 17.7 % / 28.5 % | 8/8, 1.8 % / 7.1 % |
| forced exact T+1 | 1/5, 3.0 % / 15.3 % | 3/9, 1.9 % / 9.9 % | 2/6, 2.2 % / 13.1 % | 1/8, 11.7 % / 17.0 % |
| exact T, limit 0.6 | 5/5, 1.0 % / 13.2 % | 5/9, 2.1 % / 10.1 % | 5/6, 1.8 % / 12.6 % | 7/8, 5.8 % / 10.6 % |
| exact T, limit 0.75 | 5/5, 1.0 % / 13.2 % | 8/9, 1.9 % / 9.9 % | 6/6, 1.6 % / 12.5 % | 7/8, 5.8 % / 10.6 % |
| exact T, limit 0.9 | 5/5, 1.0 % / 13.2 % | 9/9, 1.7 % / 9.7 % | 6/6, 1.6 % / 12.4 % | 7/8, 5.8 % / 10.6 % |
| exact T, limit 1.0 | 5/5, 1.0 % / 13.2 % | 9/9, 1.7 % / 9.7 % | 6/6, 1.6 % / 12.4 % | 7/8, 5.8 % / 10.6 % |
| exact T+1, limit 0.6 | 0/5, 0.9 % / 13.1 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.6 % / 12.5 % | 0/8, 15.6 % / 20.7 % |
| exact T+1, limit 0.75 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 0/8, 15.6 % / 20.7 % |
| exact T+1, limit 0.9 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 0/8, 15.6 % / 20.7 % |
| exact T+1, limit 1.0 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 0/8, 15.6 % / 20.7 % |
| min T, limit 0.6 | 0/5, 0.9 % / 13.2 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.8 % / 12.6 % | 1/8, 14.0 % / 19.2 % |
| min T, limit 0.75 | 0/5, 0.9 % / 13.2 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.8 % / 12.6 % | 1/8, 14.0 % / 19.2 % |
| min T, limit 0.9 | 0/5, 0.9 % / 13.2 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.8 % / 12.6 % | 1/8, 14.0 % / 19.2 % |
| min T, limit 1.0 | 0/5, 0.9 % / 13.2 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.8 % / 12.6 % | 1/8, 14.0 % / 19.2 % |
| max T+1, limit 0.6 | 0/5, 0.9 % / 13.1 % | 2/9, 1.8 % / 9.8 % | 1/6, 1.6 % / 12.5 % | 1/8, 11.7 % / 16.8 % |
| max T+1, limit 0.75 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 1/8, 11.7 % / 16.8 % |
| max T+1, limit 0.9 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 1/8, 11.7 % / 16.8 % |
| max T+1, limit 1.0 | 0/5, 0.9 % / 13.1 % | 2/9, 1.7 % / 9.7 % | 1/6, 1.6 % / 12.5 % | 1/8, 11.7 % / 16.8 % |

Findings.

- **Forced exact T reproduces the harm of an undercount.** On chamber audio it merges two real
  speakers while leaving the chair alone (`data/diarization-sv/hint-merge-evidence.json`: in
  IP_hb10760 and IP_hc10606 the minister and the opposing MP, 344 s and 422 s, 244 s and 347 s, all
  inside reference speech, end up in one cluster, while the 52 s / 70 s cluster with 73-78 % of its
  speech outside every reference speech, the chair, keeps its own label; SoU39 and JuU41 show the
  same). Confusion goes from 0.9-1.8 % to 7-21 %. Forced T+1 is mostly harmless on chamber audio but
  wrong for calls, which have no chair (11.7 % vs 14.0 % threshold-only).
- **The distance-limited exact T keeps the chamber result and recovers most of the call benefit.**
  Limit 0.9 (plateau up to 1.0): Riksdag debates 5/5 exact at 1.0 % confusion (threshold only 0/5,
  0.9 %), other Riksdag 9/9 at 1.7 % (2/9, 1.8 %), band-limited 6/6 at 1.6 % (1/6, 1.8 %), calls 7/8
  at 5.8 % (1/8, 14.0 %; forced reaches 8/8 at 1.8 %). The one call it does not fix is `dj_2022_feu` (38.9 %; forced
  reaches 8.8 % with its dendrogram re-cut), and the calls overall keep 5.8 % where forced reaches
  1.8 %: the price of not merging across the limit. Chamber recordings move by at most about 1 pp
  (SoU39 0.9 to 2.1 %, FS_20260604 5.5 to 7.1 %, FS_20260611 3.8 to 4.9 %) and several improve.
- **Choice of limit.** 0.6 and 0.75 leave chamber recordings unsatisfied (5/9 and 8/9 exact on
  other Riksdag) with no confusion gain; 0.9 and 1.0 give identical results here, and 0.9 is the
  lower of the two (a lower limit is the safer one). The existing absorption limit
  0.75 would have left one other-Riksdag recording unsatisfied.
- `min T` never acts here (the pipeline already finds at least T), `max T+1` only trims the calls
  (14.0 % to 11.7 %); an overcounted exact T+1 hurts calls (15.6 %), so count everyone, no more.

Per recording (found / truth, confusion), threshold only vs exact T at limit 0.9 vs forced exact T,
rows where they differ:

| Recording | Threshold only | Exact T, limit 0.9 | Forced exact T |
| --- | --- | --- | --- |
| JuU41 | 5/4, 0.5 % | 4/4, 0.2 % | 4/4, 8.0 % |
| SoU38 | 7/6, 1.2 % | 6/6, 1.0 % | 6/6, 1.0 % |
| SoU39 | 8/7, 0.9 % | 7/7, 2.1 % | 7/7, 15.2 % |
| SoU40 | 8/7, 1.1 % | 7/7, 1.1 % | 6/7, 11.6 % |
| UU24 | 5/4, 0.9 % | 4/4, 0.5 % | 4/4, 0.5 % |
| FS_20260604 | 20/19, 5.5 % | 19/19, 7.1 % | 19/19, 7.4 % |
| FS_20260611 | 19/18, 3.8 % | 18/18, 4.9 % | 17/18, 5.4 % |
| IP_hb10618 | 4/2, 1.3 % | 2/2, 0.4 % | 2/2, 46.1 % |
| IP_hb10760 | 3/2, 1.0 % | 2/2, 0.4 % | 2/2, 40.5 % |
| IP_hc10606 | 3/2, 2.4 % | 2/2, 1.6 % | 2/2, 38.7 % |
| IP_hd10115 | 4/3, 0.9 % | 3/3, 0.7 % | 3/3, 0.7 % |
| IP_hd10256 | 3/2, 1.0 % | 2/2, 0.7 % | 2/2, 0.7 % |
| IP_hd10562 | 3/3, 0.2 % | 3/3, 0.1 % | 2/3, 32.8 % |
| PL_20260610 | 6/6, 0.0 % | 6/6, 0.0 % | 5/6, 13.1 % |
| FS_20260611_tel | 20/18, 4.6 % | 18/18, 5.6 % | 17/18, 10.5 % |
| IP_hc10606_tel | 3/2, 2.4 % | 2/2, 1.5 % | 2/2, 38.3 % |
| IP_hd10256_tel | 3/2, 1.2 % | 2/2, 0.8 % | 2/2, 0.8 % |
| IP_hd10562_tel | 3/3, 0.2 % | 3/3, 0.1 % | 2/3, 32.1 % |
| SoU38_tel | 7/6, 1.3 % | 6/6, 1.1 % | 6/6, 1.1 % |
| UU24_tel | 5/4, 0.9 % | 4/4, 0.5 % | 4/4, 23.1 % |
| dj_2022_avc_16_ans | 3/2, 7.3 % | 2/2, 1.9 % | 2/2, 1.9 % |
| dj_2022_douleur_abdo | 3/2, 12.1 % | 2/2, 2.1 % | 2/2, 0.9 % |
| dj_2022_feu | 1/2, 41.0 % | 1/2, 38.9 % | 2/2, 8.8 % |
| dj_2022_grand_mere_battue | 4/2, 10.3 % | 2/2, 0.8 % | 2/2, 0.4 % |
| dj_2022_intox_med | 3/2, 5.9 % | 2/2, 1.0 % | 2/2, 0.9 % |
| dj_2022_mere_fievre | 3/2, 5.2 % | 2/2, 0.1 % | 2/2, 0.5 % |
| dj_2023_coups | 5/2, 30.7 % | 2/2, 1.2 % | 2/2, 0.6 % |

Not evaluated: a real Swedish online-meeting recording (the issue's phase-3 acceptance) and the
two-track recording-type defaults (part 2 of the issue). The only recordings without a chair are
the French and Norwegian calls.

## Reproduce

```bash
cd src-tauri && cargo build --release -p sagascript-core --features diarization --example diarize_eval
python scripts/diarization_sv/build_refs.py && python scripts/diarization_sv/build_refs_extra.py
python scripts/diarization_sv/build_degraded.py && python scripts/diarization_sv/build_other.py
# One set, explicit thresholds (default: the shipped DEFAULT_THRESHOLD read from core).
# --tag names the cached analysis; use a new tag if the analysis stage changes.
python scripts/diarization_sv/eval_der.py --bin target/release/examples/diarize_eval \
  --scratch ~/.cache/sagascript-bench/diar-284 --set sv2 --tag main --thresholds 0.34,0.36,0.48
# Variants: --min-speaker 12, --absorb-max-distance 2.0 (no limit). Collect sweeps into grid.json with
# collect_grid.py; select_default.py prints the leave-one-out, per-recording and variant tables.
# End to end through the CLI (runs Whisper; slow), explicit threshold:
python scripts/diarization_sv/eval_der.py --cli target/release/sagascript --scratch <dir> --ids UU24 --thresholds 0.34
```

`diarize_eval` (`crates/sagascript-core/examples/diarize_eval.rs`) runs the diarization stages
without Whisper: `analyze <audio> <analysis.json>` and
`cluster <analysis.json> <threshold> <segments.json> [min_speaker_seconds] [absorb_max_distance] [hint] [hint_merge_max_distance]`,
where `hint` is `N`, `N!` (forced), `MIN-MAX`, `MIN-` or `-MAX`. `eval_der.py --hint truth|truth+1|range:A:B|force:truth [--hint-merge-distance D]` sets it
relative to each recording's `reference_speakers`; `collect_hint_grid.py` and `hint_evidence.py` build the committed JSON.
