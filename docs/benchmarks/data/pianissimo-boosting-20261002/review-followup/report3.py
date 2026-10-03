import json,os,sys,statistics
sys.path.insert(0,'.')
from score3 import *
res={}
def r(label,audio,d=None,present=None,base=None):
    m=metrics(audio,label,d,present,base); res[label]=m; return m
# (c)
print('== (c)')
for a,d in [('nn','large-500'),('f60','large-distractors-only'),('rik','large-distractors-only')]:
    b=r(f'{a}-w0',a)
    b=r(f'{a}-w0',a,d); print(a,'w0',{k:round(v,2) for k,v in b.items()})
    for sfx in ['','-sf0']:
        for w in [1,2,3,5]:
            lab=f'c-{a}-w{w}{sfx}'
            if not os.path.exists(f'runs2/{lab}.json'): continue
            m=r(lab,a,d,None,f'{a}-w0'); print(lab,{k:round(v,2) for k,v in m.items()})
# (a)
print('== (a)')
def agg(prefix,audio,Ns,seeds,w,sfx,dictfmt,presfn):
    tot=dict(th=0,tt=0,nh=0,nt=0,fi=0,ch=0,wer=0,n=0)
    for s in seeds:
        d=dictfmt.format(s=s); lab=f'{prefix}-s{s}-w{w}{sfx}'
        pres=presfn(s)
        m=metrics(audio,lab,d,pres,f'{audio}-w0'); b=metrics(audio,f'{audio}-w0',d,pres)
        tot['th']+=m['target_hit'];tot['tt']+=m['target_total'];tot['bh']=tot.get('bh',0)+b['target_hit']
        tot['nh']+=m['names_hit'];tot['nt']+=m['names_total'];tot['fi']+=m['false_ins'];tot['ch']+=m['changed_words'];tot['wer']+=m['wer']-b['wer'];tot['n']+=1
        tot['bnh']=tot.get('bnh',0)+b['names_hit']; tot['bfi']=tot.get('bfi',0)+b['false_ins']
    return tot
f60names=lambda N:(lambda s:[l.strip() for l in open(f'dicts/f60-N{N}-s{s}.present')])
rik=[l.strip() for l in open('terms-riksdag.txt') if l.strip()]
for N in [100,400]:
    for sfx in ['','-sf0']:
        for w in ([3,5] if sfx=='' else [5]):
            t=agg(f'a-f60-N{N}','f60',[N],range(1,6),w,sfx,f'f60-N{N}-s{{s}}',f60names(N))
            print(f'f60 N={N} w={w}{sfx}: targets base {t["bh"]}/{t["tt"]} -> {t["th"]}/{t["tt"]}; all names base {t["bnh"]} -> {t["nh"]} of {t["nt"]}; false ins (base {t["bfi"]}) {t["fi"]}; WER delta mean {t["wer"]/t["n"]:+.2f}; words changed total {t["ch"]} (5 seeds)')
for sfx in ['','-sf0']:
    for w in ([3,5] if sfx=='' else [5]):
        t=agg('a-rik-N400','rik',[400],range(1,6),w,sfx,'rik-N400-s{s}',lambda s:rik)
        print(f'rik N=400 w={w}{sfx}: targets base {t["bh"]}/{t["tt"]} -> {t["th"]}/{t["tt"]}; all names base {t["bnh"]} -> {t["nh"]} of {t["nt"]}; false ins (base {t["bfi"]}) {t["fi"]}; WER delta mean {t["wer"]/t["n"]:+.2f}; words changed total {t["ch"]}')
