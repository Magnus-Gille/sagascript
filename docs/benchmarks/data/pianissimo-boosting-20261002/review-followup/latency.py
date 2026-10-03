import json,statistics,os
def load(lab):
    j=json.load(open(f'runs2/{lab}.json')); out={}
    for i in j:
        p=i['result']['performance']; w=p['windows']
        out[os.path.basename(i['source'])]=dict(total=p['total_seconds']*1000,rt=sum(x['round_trip_ms'] for x in w),dec=sum(x['decode_ms'] for x in w),boost=sum(x.get('boost_us',0) for x in w),text=i['result']['text'],dur=i['result']['duration_seconds'])
    return out
def pct(v,q): v=sorted(v); k=(len(v)-1)*q; f=int(k); c=min(f+1,len(v)-1); return v[f]+(v[c]-v[f])*(k-f)
data={c:[load(f'e-{c}-r{r}') for r in range(1,6)] for c in ['off','N100','N500']}
files=list(data['off'][0].keys()); skip=files[0]
print('clips',len(files),'dur median',statistics.median(data['off'][0][f]['dur'] for f in files))
def med(cfg,metric): return {f:statistics.median(d[f][metric] for d in data[cfg]) for f in files if f!=skip}
for metric in ['total','rt','dec']:
    base=med('off',metric)
    print(metric,'off p50/p95',round(pct(list(base.values()),.5),1),round(pct(list(base.values()),.95),1))
    for c in ['N100','N500']:
        m=med(c,metric); diffs=[m[f]-base[f] for f in base]
        print(' ',c,'abs p50/p95',round(pct(list(m.values()),.5),1),round(pct(list(m.values()),.95),1),'| added p50',round(pct(diffs,.5),1),'p95',round(pct(diffs,.95),1),'max',round(max(diffs),1))
b1={f:data['off'][0][f]['total'] for f in files if f!=skip}; b2={f:data['off'][1][f]['total'] for f in files if f!=skip}
d=[b2[f]-b1[f] for f in b1]; print('noise off-vs-off total diff p50/p95/abs95',round(pct(d,.5),1),round(pct(d,.95),1),round(pct([abs(x) for x in d],.95),1))
for c in ['N100','N500']:
    bs=[data[c][r][skip]['boost'] for r in range(5)]; print(c,'cold trie build us',bs,'later windows with a build:',sum(1 for r in range(5) for f in files if f!=skip and data[c][r][f]['boost']>0))
for c in ['N100','N500']:
    print('changed clips',c,sum(1 for f in files if data[c][0][f]['text']!=data['off'][0][f]['text']),'/',len(files))
