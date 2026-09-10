"""Bounded word-level edit distance; no I/O or external dependencies."""


def edit_counts(reference: list[str], hypothesis: list[str]) -> tuple[int, int, int]:
    """Return substitutions, deletions, insertions at minimum word edit cost.

    Ties prefer diagonal (match/substitution), then deletion, then insertion.
    Inputs must be lists of strings, each at most 2048 tokens, with at most
    1,048,576 reference/hypothesis cell pairs. Memory is O(len(hypothesis)).
    """
    if not isinstance(reference, list) or not isinstance(hypothesis, list):
        raise ValueError("Inputs must be lists")
    for words in (reference, hypothesis):
        if any(not isinstance(word, str) for word in words):
            raise ValueError("Inputs must contain only strings")
    if len(reference) > 2048 or len(hypothesis) > 2048:
        raise ValueError("Input exceeds 2048 tokens")
    if len(reference) * len(hypothesis) > 1048576:
        raise ValueError("Product of lengths exceeds limit")

    # Each rolling-row cell is (cost, substitutions, deletions, insertions).
    previous = [(j, 0, 0, j) for j in range(len(hypothesis) + 1)]
    for i, reference_word in enumerate(reference, 1):
        current = [(i, 0, i, 0)]
        for j, hypothesis_word in enumerate(hypothesis, 1):
            difference = reference_word != hypothesis_word
            diagonal = previous[j - 1][0] + difference
            deletion = previous[j][0] + 1
            insertion = current[j - 1][0] + 1
            cost = min(diagonal, deletion, insertion)
            if diagonal == cost:
                _, substitutions, deletions, insertions = previous[j - 1]
                current.append((cost, substitutions + difference, deletions, insertions))
            elif deletion == cost:
                _, substitutions, deletions, insertions = previous[j]
                current.append((cost, substitutions, deletions + 1, insertions))
            else:
                _, substitutions, deletions, insertions = current[j - 1]
                current.append((cost, substitutions, deletions, insertions + 1))
        previous = current
    _, substitutions, deletions, insertions = previous[-1]
    return substitutions, deletions, insertions
