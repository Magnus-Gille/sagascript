"""Name (proper noun) recall: capitalised words in the reference that are not
sentence-initial, matched case-insensitively against the hypothesis as a multiset."""
import collections, json, re, sys, unicodedata
sys.path.insert(0, '.')
from norm import norm

def ref_names(text):
    text = unicodedata.normalize('NFC', text)
    names = []
    for sent in re.split(r'(?<=[.!?])\s+|"\s*', text):
        words = re.findall(r"[\w'’-]+", sent)
        for i, w in enumerate(words):
            if i > 0 and w[:1].isupper() and not w.isupper():
                names.extend(norm(w).split())
    return [n for n in names if n]

def hyp_tokens(path):
    if path.endswith('.json'):
        j = json.load(open(path))
        text = j.get('text') or ' '.join(w['word'] for w in j.get('words', []))
    else:
        text = open(path).read()
    return collections.Counter(norm(text).split())

ref = open(sys.argv[1]).read()
names = ref_names(ref)
need = collections.Counter(names)
print(f"reference: {len(names)} name tokens, {len(need)} distinct")
for hyp in sys.argv[2:]:
    have = hyp_tokens(hyp)
    hit = sum(min(c, have[n]) for n, c in need.items())
    missed = sorted(((n, c - min(c, have[n])) for n, c in need.items() if have[n] < c), key=lambda x: -x[1])
    print(f"{hyp.split('/')[-1]}: {hit}/{len(names)} = {100*hit/len(names):.1f}%  missed e.g. {[m for m,_ in missed[:12]]}")
