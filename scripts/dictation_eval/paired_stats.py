"""Seeded paired bootstrap statistics for bounded latency evidence."""

import math
import random
from numbers import Real


MAX_SAMPLE_VALUES = 100_000
MAX_UTTERANCES = 500
MIN_REPETITIONS = 5
MAX_REPETITIONS = 20
MIN_RESAMPLES = 100
MAX_RESAMPLES = 10_000
MAX_SEED = 2**32 - 1


def _finite_real(value: object, name: str, *, nonnegative: bool) -> float:
    if isinstance(value, bool) or not isinstance(value, Real):
        raise ValueError(f"{name} must be a real number")
    try:
        converted = float(value)
    except (OverflowError, ValueError) as error:
        raise ValueError(f"{name} must be finite") from error
    if not math.isfinite(converted) or (nonnegative and converted < 0.0):
        requirement = "finite and non-negative" if nonnegative else "finite"
        raise ValueError(f"{name} must be {requirement}")
    return converted


def _rank_finite(values: list[float], percentile: float) -> float:
    """Return nearest rank for finite values, allowing negative values."""

    if not values or len(values) > MAX_SAMPLE_VALUES:
        raise ValueError("values must be nonempty and within the sample limit")
    rank = math.ceil(percentile * len(values)) - 1
    return sorted(values)[rank]


def nearest_rank(values: list[float], percentile: float) -> float:
    """Return the nearest-rank percentile of finite non-negative values."""

    if not isinstance(values, list):
        raise ValueError("values must be a list")
    if not values or len(values) > MAX_SAMPLE_VALUES:
        raise ValueError("values must be nonempty and within the sample limit")
    percentile_value = _finite_real(percentile, "percentile", nonnegative=True)
    if percentile_value <= 0.0 or percentile_value > 1.0:
        raise ValueError("percentile must be greater than zero and at most one")
    checked = [
        _finite_real(value, "values", nonnegative=True)
        for value in values
    ]
    return _rank_finite(checked, percentile_value)


def _validate_groups(
    groups: object,
    name: str,
    *,
    require_positive: bool,
) -> tuple[list[list[float]], int]:
    if not isinstance(groups, list):
        raise ValueError(f"{name} must be a list of utterance groups")
    utterances = len(groups)
    if not 0 < utterances <= MAX_UTTERANCES:
        raise ValueError("utterance count must be between one and 500")

    checked_groups: list[list[float]] = []
    repetitions: int | None = None
    for group in groups:
        if not isinstance(group, list):
            raise ValueError(f"{name} groups must be lists")
        group_repetitions = len(group)
        if not MIN_REPETITIONS <= group_repetitions <= MAX_REPETITIONS:
            raise ValueError("each utterance must have between 5 and 20 repetitions")
        if repetitions is None:
            repetitions = group_repetitions
        elif repetitions != group_repetitions:
            raise ValueError("all utterances must have the same repetition count")
        checked_groups.append(
            [
                _finite_real(value, name, nonnegative=not require_positive)
                for value in group
            ]
        )
    if repetitions is None:
        raise ValueError("utterance groups cannot be empty")
    if require_positive and any(value <= 0.0 for group in checked_groups for value in group):
        raise ValueError("baseline values must be greater than zero")
    return checked_groups, repetitions


def paired_cluster_interval(
    baseline: list[list[float]],
    candidate: list[list[float]],
    *,
    seed: int = 187,
    resamples: int = 2000,
) -> dict[str, float | int | list[float] | str]:
    """Compute a seeded paired utterance-cluster bootstrap interval.

    This function only consumes caller-supplied latency values. It does not
    establish whether those values are warm, visible, or otherwise eligible.
    """

    if type(seed) is not int or not 0 <= seed <= MAX_SEED:
        raise ValueError("seed must be an integer between zero and 2**32-1")
    if type(resamples) is not int or not MIN_RESAMPLES <= resamples <= MAX_RESAMPLES:
        raise ValueError("resamples must be an integer between 100 and 10000")

    baseline_groups, repetitions = _validate_groups(
        baseline, "baseline", require_positive=True
    )
    candidate_groups, candidate_repetitions = _validate_groups(
        candidate, "candidate", require_positive=False
    )
    if len(baseline_groups) != len(candidate_groups):
        raise ValueError("baseline and candidate must have the same utterances")
    if repetitions != candidate_repetitions:
        raise ValueError("baseline and candidate must have the same repetitions")

    baseline_values = [value for group in baseline_groups for value in group]
    candidate_values = [value for group in candidate_groups for value in group]
    utterances = len(baseline_groups)
    if len(baseline_values) > MAX_SAMPLE_VALUES:
        raise ValueError("paired samples exceed the sample limit")

    baseline_p50 = nearest_rank(baseline_values, 0.5)
    baseline_p95 = nearest_rank(baseline_values, 0.95)
    candidate_p50 = nearest_rank(candidate_values, 0.5)
    candidate_p95 = nearest_rank(candidate_values, 0.95)

    def relative_gain(candidate_p95: float, baseline_p95: float) -> float:
        gain = 1.0 - candidate_p95 / baseline_p95
        if not math.isfinite(gain):
            raise ValueError("relative gain must be finite")
        return gain

    rng = random.Random(seed)
    gains: list[float] = []
    for _ in range(resamples):
        selected = [rng.randrange(utterances) for _ in range(utterances)]
        baseline_sample = [
            value for index in selected for value in baseline_groups[index]
        ]
        candidate_sample = [
            value for index in selected for value in candidate_groups[index]
        ]
        sampled_baseline_p95 = nearest_rank(baseline_sample, 0.95)
        sampled_candidate_p95 = nearest_rank(candidate_sample, 0.95)
        gains.append(relative_gain(sampled_candidate_p95, sampled_baseline_p95))

    return {
        "baseline_p50": baseline_p50,
        "baseline_p95": baseline_p95,
        "candidate_p50": candidate_p50,
        "candidate_p95": candidate_p95,
        "relative_p95_gain": relative_gain(candidate_p95, baseline_p95),
        "relative_p95_gain_interval": [
            _rank_finite(gains, 0.025),
            _rank_finite(gains, 0.975),
        ],
        "utterances": utterances,
        "repetitions_per_utterance": repetitions,
        "resamples": resamples,
        "seed": seed,
        "sampling_unit": "utterance_cluster",
    }
