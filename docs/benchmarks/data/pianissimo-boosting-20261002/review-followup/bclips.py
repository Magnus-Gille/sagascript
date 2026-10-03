import sys,os,json,re,random,difflib,unicodedata
import numpy as np, soundfile as sf
sys.path.insert(0,os.path.expanduser('~/.cache/sagascript-bench/bench')); sys.path.insert(0,'.')
from norm import norm
B=os.path.expanduser('~/.cache/sagascript-bench/bias-298'); L=os.path.expanduser('~/.cache/sagascript-bench/longform')
ref=unicodedata.normalize('NFC',open(L+'/fleurs-sv-distinct-60min.txt').read())
# reference tokens with a name flag (same rule as names.py)
rt=[]
for sent in re.split(r'(?<=[.!?])\s+|"\s*',ref):
    words=re.findall(r"[\w'’-]+",sent)
    for i,w in enumerate(words):
        flag=i>0 and w[:1].isupper() and not w.isupper()
        for t in norm(w).split(): rt.append((t,flag))
j=json.load(open(B+'/runs2/f60-w0.json'))
ht=[]
for s in j['segments']:
    for t in norm(s['text']).split(): ht.append((t,s['start'],s['end']))
sm=difflib.SequenceMatcher(None,[t for t,_ in rt],[t for t,_,_ in ht],autojunk=False)
occ=[]  # (name, hit, start, end)
for op,i1,i2,j1,j2 in sm.get_opcodes():
    if op=='equal' or (op=='replace' and i2-i1==j2-j1):
        for k in range(i2-i1):
            t,flag=rt[i1+k]
            if flag: occ.append((t,op=='equal',ht[j1+k][1],ht[j1+k][2]))
names={}
for t,hit,s,e in occ: names.setdefault(t,[]).append((hit,s,e))
rnd=random.Random(5)
hits=[(n,v[0]) for n,v in names.items() if v[0][0]]; miss=[(n,v[0]) for n,v in names.items() if not v[0][0]]
rnd.shuffle(hits); rnd.shuffle(miss)
sel=[('hit',n,v) for n,v in hits[:30]]+[('miss',n,v) for n,v in miss[:30]]
print('occurrences mapped',len(occ),'distinct names',len(names),'hit-names',len(hits),'miss-names',len(miss),'selected',len(sel))
a,sr=sf.read(L+'/fleurs-sv-distinct-60min.wav',dtype='float32'); assert sr==16000
manifest=[]
for kind,n,(hit,s,e) in sel:
    for variant,off in [('cutA',14.7),('cutB',12.7),('mid',6.0)]:
        c0=s-off
        if c0<0: continue
        c1=min(len(a)/16000,s+8)
        p=f'{B}/bclips/{n}-{variant}.wav'
        sf.write(p,a[int(c0*16000):int(c1*16000)],16000,subtype='PCM_16')
        manifest.append(dict(name=n,kind=kind,variant=variant,path=p,dur=round(c1-c0,1)))
json.dump(manifest,open(B+'/bclips/manifest.json','w'))
open(B+'/dicts/bclips-names.txt','w').write('\n'.join(n for _,n,_ in sel)+'\n')
print('clips',len(manifest))
