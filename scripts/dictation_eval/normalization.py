"""Unicode-aware, whole-token normalization for offline dictation scoring."""

import unicodedata


MAX_INPUT_CHARACTERS = 100_000


def _validate_text(text: str) -> None:
    if not isinstance(text, str):
        raise ValueError("Text must be a string")
    if len(text) > MAX_INPUT_CHARACTERS:
        raise ValueError("Text exceeds 100000 characters")


def _is_word_character(character: str) -> bool:
    category = unicodedata.category(character)
    return category[0] in {"L", "N", "M"}


def normalize_text(text: str) -> list[str]:
    """Return normalized whole-word tokens from *text*.

    Input is capped before normalization. NFC is applied before casefolding and
    once more afterwards because casefolding can introduce decomposed marks.
    Unicode letters, numbers, and combining marks stay in tokens. An ASCII or
    U+2019 apostrophe stays only when it is internal to a token, and U+2019 is
    canonicalized to ASCII apostrophe. Every other character separates tokens.
    """

    _validate_text(text)
    normalized = unicodedata.normalize("NFC", text).casefold()
    normalized = unicodedata.normalize("NFC", normalized)

    tokens: list[str] = []
    current: list[str] = []

    def flush() -> None:
        if current:
            tokens.append("".join(current))
            current.clear()

    for index, character in enumerate(normalized):
        if _is_word_character(character):
            current.append(character)
            continue

        if (
            character in {"'", "\u2019"}
            and current
            and index + 1 < len(normalized)
            and _is_word_character(normalized[index + 1])
        ):
            current.append("'")
            continue

        flush()

    flush()
    return tokens


def normalize_words(text: str) -> list[str]:
    """Compatibility spelling for callers that describe the result as words."""

    return normalize_text(text)


def count_phrase_occurrences(text: str, phrase: str) -> int:
    """Count non-overlapping, exact whole-token occurrences of *phrase*.

    Both arguments are normalized with :func:`normalize_text`; therefore a
    phrase never matches a substring inside another token. An empty normalized
    phrase is invalid rather than matching at every boundary.
    """

    tokens = normalize_text(text)
    phrase_tokens = normalize_text(phrase)
    if not phrase_tokens:
        raise ValueError("Phrase must contain at least one token")

    width = len(phrase_tokens)
    occurrences = 0
    index = 0
    while index <= len(tokens) - width:
        if tokens[index : index + width] == phrase_tokens:
            occurrences += 1
            index += width
        else:
            index += 1
    return occurrences
