#!/usr/bin/env python3
"""Word error rate between a reference and a hypothesis text file (stdlib only).

Normalisation: NFC, lowercase, punctuation -> space, whitespace collapsed; numbers
are not normalised (same as docs/benchmarks/pianissimo-coreml-2026-09.md).
Prints one JSON object: wer_percent, ref_words, errors.
"""
import json, re, sys, unicodedata


def norm(t):
    t = unicodedata.normalize("NFC", t).lower()
    return re.sub(r"[^\w\s]|_", " ", t).split()


def distance(a, b):
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        cur = [i]
        for j, y in enumerate(b, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1]


def wer(ref_text, hyp_text):
    ref, hyp = norm(ref_text), norm(hyp_text)
    d = distance(ref, hyp)
    return dict(wer_percent=round(100 * d / len(ref), 2), ref_words=len(ref), hyp_words=len(hyp), errors=d)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: wer.py REFERENCE.txt HYPOTHESIS.txt")
    r = open(sys.argv[1], encoding="utf-8").read()
    h = open(sys.argv[2], encoding="utf-8").read()
    print(json.dumps(wer(r, h)))
