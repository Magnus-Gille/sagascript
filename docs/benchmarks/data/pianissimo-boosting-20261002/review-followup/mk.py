import csv,os,sys,random,re,json,collections
import numpy as np, soundfile as sf
sys.path.insert(0,os.path.expanduser('~/.cache/sagascript-bench/bench')); sys.path.insert(0,'.')
from norm import norm
from eval import ref_names, cased_names
B=os.path.expanduser('~/.cache/sagascript-bench/bias-298')
L=os.path.expanduser('~/.cache/sagascript-bench/longform')
refs={'fleurs60':open(L+'/fleurs-sv-distinct-60min.txt').read(),'fleurs15':open(L+'/fleurs-sv-distinct-15min.txt').read(),
      'riksdag':open(os.path.expanduser('~/.cache/sagascript-bench/riksdag/reference.txt')).read()}
# whole FLEURS test text (all sentences) so distractors are absent from every clip we may use
root=os.path.expanduser("~/.cache/sagascript-bench/fleurs")
rows=list(csv.reader(open(f"{root}/data/sv_se/test.tsv"),delimiter="\t",quoting=csv.QUOTE_NONE))
allf=' '.join(r[2] for r in rows)
vocab=set()
for t in list(refs.values())+[allf]: vocab|=set(norm(t).split())
raw=sorted(set(l.strip() for l in open('distractors-raw.txt') if l.strip()))
dis=[d for d in raw if not (set(norm(d).split())&vocab)]
print('distractors',len(raw),'kept',len(dis),'dropped',[d for d in raw if d not in dis])
open('dicts/distractors.txt','w').write('\n'.join(dis)+'\n')
# no-names corpus
seen=set();pieces=[];refl=[];dur=0;gap=np.zeros(8000,np.float32)
audio=f"{root}/sv_test_audio/test"
for r in rows:
    p=os.path.join(audio,r[1])
    if not os.path.exists(p) or r[2] in seen: continue
    seen.add(r[2])
    if ref_names(r[2]) or re.search(r'\d',r[2]): continue
    a,sr=sf.read(p,dtype='float32'); 
    if a.ndim>1:a=a.mean(axis=1)
    pieces+=[a,gap];refl.append(r[2]);dur+=len(a)/16000+0.5
    if dur>=900: break
sf.write('nonames/nonames.wav',np.concatenate(pieces),16000,subtype='PCM_16')
open('nonames/nonames.txt','w').write(' '.join(refl)+'\n')
print('nonames clips',len(refl),'seconds',round(dur),'words',len(' '.join(refl).split()),'names',len(ref_names(' '.join(refl))))
# short clips 2-8 s
short=[];seen=set()
for r in rows:
    p=os.path.join(audio,r[1])
    if not os.path.exists(p) or r[2] in seen: continue
    seen.add(r[2]); i=sf.info(p); d=i.frames/i.samplerate
    if 2<=d<=8: short.append((p,round(d,2)))
open('dicts/short-clips.txt','w').write('\n'.join(p for p,_ in short)+'\n')
print('short',len(short),'median dur',sorted(d for _,d in short)[len(short)//2])
# name pools
fn=[l.strip() for l in open('terms-fleurs60.txt') if l.strip()]
open('dicts/names-fleurs60.txt','w').write('\n'.join(fn)+'\n')
rik=[l.strip() for l in open('terms-riksdag.txt') if l.strip()]
# (a) FLEURS60 dictionaries: P present names + distractors, 5 seeds
for N,P in [(100,5),(400,20)]:
    for seed in range(1,6):
        rnd=random.Random(1000*N+seed)
        present=rnd.sample(fn,P); d=rnd.sample(dis,min(N-P,len(dis)))
        terms=present+d; rnd.shuffle(terms)
        open(f'dicts/f60-N{N}-s{seed}.txt','w').write('\n'.join(terms)+'\n')
        open(f'dicts/f60-N{N}-s{seed}.present','w').write('\n'.join(present)+'\n')
# riksdag: 20 speaker terms + 380 distractors, 5 seeds
for seed in range(1,6):
    rnd=random.Random(7000+seed); d=rnd.sample(dis,400-len(rik)); terms=rik+d; rnd.shuffle(terms)
    open(f'dicts/rik-N400-s{seed}.txt','w').write('\n'.join(terms)+'\n')
# (c) 470ish distractors + 30 FLEURS names (names are absent from the no-names audio by construction)
rnd=random.Random(99); nm=rnd.sample(fn,30); terms=dis[:470]+nm; rnd.shuffle(terms)
open('dicts/large-500.txt','w').write('\n'.join(terms)+'\n'); print('large', len(terms))
open('dicts/large-distractors-only.txt','w').write('\n'.join(dis[:500])+'\n')
