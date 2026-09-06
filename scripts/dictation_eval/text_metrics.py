"""Content-free aggregate text metrics for the offline dictation evaluator."""

from edit_counts import edit_counts
from normalization import count_phrase_occurrences, normalize_text


MAX_TERMS = 256
MAX_FALSE_GLOSSARY_REPLACEMENTS = 100_000


def _validate_bool(value: bool, name: str) -> None:
    if type(value) is not bool:
        raise ValueError(f"{name} must be a boolean")


def _normalize_terms(terms: list[str], name: str) -> list[str]:
    if not isinstance(terms, list):
        raise ValueError(f"{name} must be a list")
    if len(terms) > MAX_TERMS:
        raise ValueError(f"{name} exceeds the term limit")

    normalized_terms: list[str] = []
    seen: set[tuple[str, ...]] = set()
    for term in terms:
        if not isinstance(term, str):
            raise ValueError(f"{name} must contain only strings")
        tokens = normalize_text(term)
        if not tokens:
            raise ValueError(f"{name} contains an empty term")
        key = tuple(tokens)
        if key in seen:
            raise ValueError(f"{name} contains duplicate terms")
        seen.add(key)
        normalized_terms.append(" ".join(tokens))
    return normalized_terms


def score_text(
    reference: str,
    hypothesis: str,
    specialist_terms: list[str],
    control_terms: list[str],
    *,
    is_silence: bool = False,
    is_ordinary_control: bool = False,
    false_glossary_replacements: int | None = None,
) -> dict[str, int | float | bool | None]:
    """Score one reference/hypothesis pair without returning content.

    Specialist terms must occur in the reference so a missing fixture term is
    rejected rather than silently improving recall. Control terms may be absent
    because their insertion is itself an evaluated error. False glossary
    replacements are an explicit annotation; this function never infers them
    from transcript text.
    """

    _validate_bool(is_silence, "is_silence")
    _validate_bool(is_ordinary_control, "is_ordinary_control")
    if false_glossary_replacements is not None:
        if (
            type(false_glossary_replacements) is not int
            or not 0 <= false_glossary_replacements <= MAX_FALSE_GLOSSARY_REPLACEMENTS
        ):
            raise ValueError("false glossary replacement annotation is invalid")
        if not is_ordinary_control:
            raise ValueError("false glossary replacement annotation requires an ordinary control")

    reference_words = normalize_text(reference)
    hypothesis_words = normalize_text(hypothesis)
    if is_silence and reference_words:
        raise ValueError("silence references must be empty")

    specialist = _normalize_terms(specialist_terms, "specialist terms")
    control = _normalize_terms(control_terms, "control terms")
    specialist_counts: list[tuple[int, int]] = []
    for term in specialist:
        reference_count = count_phrase_occurrences(reference, term)
        if reference_count == 0:
            raise ValueError("specialist term is absent from the reference")
        specialist_counts.append(
            (reference_count, count_phrase_occurrences(hypothesis, term))
        )

    substitutions, deletions, insertions = edit_counts(reference_words, hypothesis_words)
    reference_count = len(reference_words)
    hypothesis_count = len(hypothesis_words)
    edit_total = substitutions + deletions + insertions

    specialist_expected = sum(expected for expected, _ in specialist_counts)
    specialist_recalled = sum(
        min(expected, recalled) for expected, recalled in specialist_counts
    )
    specialist_recall = (
        specialist_recalled / specialist_expected if specialist_expected else None
    )
    control_errors = sum(
        abs(
            count_phrase_occurrences(reference, term)
            - count_phrase_occurrences(hypothesis, term)
        )
        for term in control
    )
    ordinary_control_words = reference_count if is_ordinary_control else 0
    false_replacements_per_1000 = (
        false_glossary_replacements * 1000 / ordinary_control_words
        if false_glossary_replacements is not None and ordinary_control_words
        else None
    )

    return {
        "reference_words": reference_count,
        "hypothesis_words": hypothesis_count,
        "substitutions": substitutions,
        "deletions": deletions,
        "insertions": insertions,
        "wer": edit_total / reference_count if reference_count else None,
        "specialist_expected": specialist_expected,
        "specialist_recalled": specialist_recalled,
        "specialist_recall": specialist_recall,
        "control_errors": control_errors,
        "silence_hallucination": is_silence and bool(hypothesis_words),
        "ordinary_control_words": ordinary_control_words,
        "false_glossary_replacements": false_glossary_replacements,
        "false_replacements_per_1000_control_words": false_replacements_per_1000,
    }
