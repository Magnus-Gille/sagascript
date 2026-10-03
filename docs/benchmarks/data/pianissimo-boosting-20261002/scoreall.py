import sys,os,json,collections
sys.path.insert(0,'.')
from eval import score
from norm import norm
L=os.path.expanduser('~/.cache/sagascript-bench/longform')
refs={'fleurs15':L+'/fleurs-sv-distinct-15min.txt','fleurs60':L+'/fleurs-sv-distinct-60min.txt','riksdag':os.path.expanduser('~/.cache/sagascript-bench/riksdag/reference.txt')}
spk=set(t for l in open('terms-riksdag.txt') for t in norm(l).split())
rows=[]
for ds,rp in refs.items():
    ref=open(rp).read()
    for kind in ['true','fp']:
        fp=[l.strip() for l in open(f'terms-unrelated-{ds}.txt')] if kind=='fp' else []
        for w in [0,1,2,3,5]:
            lab=f'{ds}-w0' if w==0 else f'{ds}-{kind}-w{w}'
            if w==0 and kind=='fp': 
                fpl=[l.strip() for l in open(f'terms-unrelated-{ds}.txt')]
                hyp=open(f'runs/{lab}.txt').read(); r=score(ref,hyp,fpl)
            else:
                hyp=open(f'runs/{lab}.txt').read(); r=score(ref,hyp,fp)
            if ds=='riksdag':
                rc=collections.Counter(norm(ref).split()); hc=collections.Counter(norm(hyp).split())
                tot=sum(rc[t] for t in spk); hit=sum(min(rc[t],hc[t]) for t in spk)
                r['speaker_recall']=100*hit/tot; r['speaker_tokens']=tot
            r.update(ds=ds,kind=kind,w=w); rows.append(r)
            print(json.dumps({k:(round(v,2) if isinstance(v,float) else v) for k,v in r.items()}))
