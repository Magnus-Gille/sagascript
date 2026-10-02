import json,sys,os,collections
sys.path.insert(0,os.path.expanduser('~/.cache/sagascript-bench/bench'))
from norm import norm
man=json.load(open('bclips/manifest.json'))
def load(l):
    j=json.load(open(f'runs2/{l}.json'))
    return {os.path.basename(i['source']):i['result']['text'] for i in j}
res={}
for lab in ['b-w0','b-w3','b-w5','b-w5-sf0']:
    h=load(lab); c=collections.Counter()
    for m in man:
        key=(m['kind'],m['variant']); toks=norm(h[os.path.basename(m['path'])]).split()
        c[key+('tot',)]+=1; c[key+('hit',)]+= m['name'] in toks
    res[lab]=c
for kind in ['hit','miss']:
    for var in ['cutA','cutB','mid']:
        print(kind,var,' | '.join(f"{lab}: {res[lab][(kind,var,'hit')]}/{res[lab][(kind,var,'tot')]}" for lab in res))
