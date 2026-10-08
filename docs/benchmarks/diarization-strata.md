# Diarization strata measurements

`sagascript_core::diarization_strata::measure_strata` computes bounded,
offline measurements from validated `SpeakerTurn` inputs. It receives the
explicit UEM and an existing `EvaluationReport`; the report's hypothesis to
reference speaker mapping is reused exactly. The helper never re-optimizes a
local mapping and never applies a quality threshold.

## Short reference regions

Reference turns are unioned separately for each speaker before any UEM
clipping. A source region is one resulting contiguous interval. A region is
short when its source length is strictly less than two seconds. The report
includes source region count and duration, short count and duration, and the
each speaker's activity inside the selected short-turn windows as
`short_window_reference_seconds`. This can include a concurrent long speaker.
Source intervals are the reference intervals supplied to the scorer: an
annotation ending at a review-window or unknown boundary does not establish
that a real utterance ended there. Genuine short-turn accuracy requires
independently verified turn boundaries.

## Fixed-mapping score over short-turn windows

All source regions shorter than two seconds are unioned across speakers into
disjoint short-turn windows, then intersected with the supplied explicit UEM.
The score sweep covers only those selected windows. A selected window can
therefore include a concurrent long reference speaker; its error totals are
all-active-speaker totals for the window, not errors attributed solely to the
short speaker identity. Reference and hypothesis turns are unioned per speaker
before clipping. Overlap is preserved and any UEM hole remains excluded. Miss,
false alarm, and confusion use the fixed global mapping and speaker-time
denominator:

* `miss = max(reference speaker count - hypothesis speaker count, 0)`;
* `false_alarm = max(hypothesis speaker count - reference speaker count, 0)`;
* `confusion = min(reference speaker count, hypothesis speaker count) - correct mapped speakers`.

Each quantity is accumulated over the disjoint selected windows. The report
also retains the normalized explicit UEM and the selected `short_turn_windows`
so the clipped scoring duration is separate from source short-region duration.
`der` is reported only when the
reference speaker-time denominator is positive; it is a measurement, not an
acceptance decision.

The native CLI exposes these measurements under `strata` with
`strata_collar_seconds: 0.0`. They retain the original UEM even when the global
DER mapping was selected with a nonzero collar; short intervals and original
boundaries therefore remain visible. These strata scores use that same fixed
mapping and must not be compared as if they used the global DER collar.

## Boundary errors

For every reference speaker and for starts and ends independently, boundaries
are matched one-to-one with the nearest same-type hypothesis boundary from a
hypothesis speaker mapped to that reference speaker. Matching processes
reference boundaries in sorted time order and greedily takes the nearest
currently unused hypothesis boundary; this is deterministic nearest-greedy
matching, not a global optimal assignment. Matching cannot cross an explicit
UEM region. Boundaries exactly equal to a UEM start or end are excluded for
both reference and hypothesis intervals, since an annotation edge at the
scoring boundary cannot establish a real utterance boundary. The signed error
is `hypothesis_time - reference_time`; positive starts are late and positive
ends are late. The report includes the signed errors, matched count, unmatched
reference count, and unmatched hypothesis count.

Source boundaries outside the UEM are discarded. A UEM clip edge or an
unknown-region hole never creates a synthetic boundary. Output speakers,
regions, and signed errors are deterministic and sorted.

The helper enforces the same limits as the scorer: at most 100,000 turns per
stream, 64 distinct speaker IDs per stream, four hours of timeline, and
100,000 explicit UEM regions. It uses sorted per-speaker unions, linear UEM
clipping, event sweeps, and binary search plus ordered candidate sets for
boundary matching.
