import sys, re, json, collections, unicodedata, os
sys.path.insert(0, os.path.expanduser('~/.cache/sagascript-bench/bench'))
import jiwer
from norm import norm
src = open(os.path.expanduser('~/.cache/sagascript-bench/bench/names.py')).read().split("ref = open")[0]
ns = {}; exec(src, ns); ref_names = ns['ref_names']

def cased_names(text):
    text = unicodedata.normalize('NFC', text); out = []
    for sent in re.split(r'(?<=[.!?])\s+|"\s*', text):
        words = re.findall(r"[\w'’-]+", sent)
        for i, w in enumerate(words):
            if i > 0 and w[:1].isupper() and not w.isupper(): out.append(w)
    seen = set(); res = []
    for w in out:
        if w.lower() not in seen: seen.add(w.lower()); res.append(w)
    return res

def score(ref_text, hyp_text, fp_terms=()):
    names = ref_names(ref_text); need = collections.Counter(names)
    have = collections.Counter(norm(hyp_text).split())
    hit = sum(min(c, have[n]) for n, c in need.items())
    wer = jiwer.wer(norm(ref_text), norm(hyp_text))
    refc = collections.Counter(norm(ref_text).split())
    fp = 0
    for t in fp_terms:
        for tok in norm(t).split():
            fp += max(0, have[tok] - refc[tok])
    return dict(name_recall=100*hit/len(names), names=len(names), wer=100*wer, false_ins=fp)

if __name__ == '__main__':
    cmd = sys.argv[1]
    if cmd == 'terms':
        ref = open(sys.argv[2]).read()
        print('\n'.join(cased_names(ref)))
    elif cmd == 'score':
        ref = open(sys.argv[2]).read(); hyp = open(sys.argv[3]).read()
        fp = open(sys.argv[4]).read().split('\n') if len(sys.argv) > 4 else []
        print(json.dumps(score(ref, hyp, fp)))
