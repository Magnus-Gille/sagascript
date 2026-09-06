"""Pooled paired WER statistics for bounded offline evaluation."""

import math
import random
from numbers import Integral


MAX_UTTERANCES = 500
MAX_REFERENCE_WORDS = 2048
MAX_ERRORS = 10_000
MIN_RESAMPLES = 100
MAX_RESAMPLES = 10_000
MAX_SEED = 2**32 - 1


def _validate_integer(value: object, name: str, lower: int, upper: int) -> int:
    if isinstance(value, bool) or not isinstance(value, Integral):
        raise ValueError(f"{name} must be an integer")
    converted = int(value)
    if not lower <= converted <= upper:
        raise ValueError(f"{name} is outside the permitted range")
    return converted


def _validate_samples(
    values: object,
    name: str,
    lower: int,
    upper: int,
) -> list[int]:
    if not isinstance(values, list):
        raise ValueError(f"{name} must be a list")
    if not 0 < len(values) <= MAX_UTTERANCES:
        raise ValueError("sample lists must contain between one and 500 utterances")
    return [
        _validate_integer(value, name, lower, upper)
        for value in values
    ]


def _nearest_rank(values: list[float], percentile: float) -> float:
    if not values:
        raise ValueError("bootstrap values must be nonempty")
    rank = math.ceil(percentile * len(values)) - 1
    return sorted(values)[rank]


def paired_wer_interval(
    reference_words: list[int],
    baseline_errors: list[int],
    candidate_errors: list[int],
    *,
    seed: int = 187,
    resamples: int = 2000,
) -> dict[str, float | int | list[float] | None | str]:
    """Compute pooled WER and a paired utterance bootstrap interval.

    References are speech-utterance word counts. Silence controls and any
    provenance or adoption decision remain outside this pure function.
    """

    references = _validate_samples(
        reference_words, "reference_words", 1, MAX_REFERENCE_WORDS
    )
    baseline = _validate_samples(baseline_errors, "baseline_errors", 0, MAX_ERRORS)
    candidate = _validate_samples(candidate_errors, "candidate_errors", 0, MAX_ERRORS)
    if len(references) != len(baseline) or len(references) != len(candidate):
        raise ValueError("all sample lists must have the same utterances")
    seed_value = _validate_integer(seed, "seed", 0, MAX_SEED)
    resample_count = _validate_integer(
        resamples, "resamples", MIN_RESAMPLES, MAX_RESAMPLES
    )

    reference_total = sum(references)
    baseline_error_total = sum(baseline)
    candidate_error_total = sum(candidate)
    baseline_wer = baseline_error_total / reference_total
    candidate_wer = candidate_error_total / reference_total

    def relative_reduction(candidate_value: float, baseline_value: float) -> float | None:
        if baseline_value == 0.0:
            return None
        return 1.0 - candidate_value / baseline_value

    rng = random.Random(seed_value)
    absolute_changes: list[float] = []
    relative_reductions: list[float] = []
    has_zero_baseline_resample = False
    utterances = len(references)
    for _ in range(resample_count):
        selected = [rng.randrange(utterances) for _ in range(utterances)]
        sampled_references = sum(references[index] for index in selected)
        sampled_baseline_errors = sum(baseline[index] for index in selected)
        sampled_candidate_errors = sum(candidate[index] for index in selected)
        sampled_baseline_wer = sampled_baseline_errors / sampled_references
        sampled_candidate_wer = sampled_candidate_errors / sampled_references
        absolute_changes.append(sampled_candidate_wer - sampled_baseline_wer)
        if sampled_baseline_errors == 0:
            has_zero_baseline_resample = True
        else:
            relative_reductions.append(
                1.0 - sampled_candidate_wer / sampled_baseline_wer
            )

    return {
        "baseline_wer": baseline_wer,
        "candidate_wer": candidate_wer,
        "absolute_wer_change": candidate_wer - baseline_wer,
        "relative_wer_reduction": relative_reduction(candidate_wer, baseline_wer),
        "absolute_wer_change_interval": [
            _nearest_rank(absolute_changes, 0.025),
            _nearest_rank(absolute_changes, 0.975),
        ],
        "relative_wer_reduction_interval": (
            None
            if has_zero_baseline_resample
            else [
                _nearest_rank(relative_reductions, 0.025),
                _nearest_rank(relative_reductions, 0.975),
            ]
        ),
        "utterances": utterances,
        "resamples": resample_count,
        "seed": seed_value,
        "sampling_unit": "utterance",
    }
