import sys,os,json,glob,collections,re
sys.path.insert(0,os.path.expanduser('~/.cache/sagascript-bench/bench')); sys.path.insert(0,'.')
import jiwer
from norm import norm
from eval import ref_names
B=os.path.expanduser('~/.cache/sagascript-bench/bias-298'); L=os.path.expanduser('~/.cache/sagascript-bench/longform')
REF={'f60':open(L+'/fleurs-sv-distinct-60min.txt').read(),'rik':open(os.path.expanduser('~/.cache/sagascript-bench/riksdag/reference.txt')).read(),'nn':open(B+'/nonames/nonames.txt').read()}
def hyp(label): return json.load(open(f'{B}/runs2/{label}.json'))['text']
def toks(t): return norm(t).split()
def terms_tokens(path): 
    s=set()
    for l in open(path):
        s|=set(toks(l))
    return s
def metrics(audio,label,dictname=None,present=None,base=None):
    ref=REF[audio]; h=hyp(label); rt=toks(ref); ht=toks(h)
    refc=collections.Counter(rt); hc=collections.Counter(ht)
    out={}
    out['wer']=100*jiwer.wer(' '.join(rt),' '.join(ht))
    names=ref_names(ref); need=collections.Counter(names)
    out['names_hit']=sum(min(c,hc[n]) for n,c in need.items()); out['names_total']=len(names)
    if present is not None:
        pt=set(); [pt.update(toks(p)) for p in present]
        tot=sum(refc[t] for t in pt); hit=sum(min(refc[t],hc[t]) for t in pt)
        out['target_hit']=hit; out['target_total']=tot
    if dictname:
        dt=terms_tokens(f'{B}/dicts/{dictname}.txt')
        if present is not None:
            pt2=set(); [pt2.update(toks(p)) for p in present]; dt=dt-pt2
        out['false_ins']=sum(max(0,hc[t]-refc[t]) for t in dt)
        out['dict_terms']=sum(1 for _ in open(f'{B}/dicts/{dictname}.txt'))
    out['ref_words']=len(rt)
    if base:
        b=toks(hyp(base)); m=jiwer.process_words(' '.join(b),' '.join(ht)); out['changed_words']=m.substitutions+m.deletions+m.insertions
    return out
