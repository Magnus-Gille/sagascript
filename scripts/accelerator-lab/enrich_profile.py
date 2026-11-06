"""Attach actual executed provider events to a successful Rust-host benchmark."""
import argparse
from collections import Counter
import json
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument('--lab',required=True,type=Path)
p.add_argument('--label',required=True)
a=p.parse_args()
target=a.lab/(a.label+'.json')
data=json.loads(target.read_text(encoding='utf-8'))
profiles=sorted(a.lab.glob(a.label+'-profile*.json'),key=lambda p:p.stat().st_mtime)
if not profiles:
    raise SystemExit('No completed profile found')
profile=profiles[-1]
events=json.loads(profile.read_text(encoding='utf-8'))
nodes=[e for e in events if e.get('args',{}).get('provider')]
data['profile']=str(profile)
data['placement_events']=dict(Counter(e['args']['provider'] for e in nodes))
data['provider_node_duration_us']={p:sum(e.get('dur',0) for e in nodes if e['args']['provider']==p) for p in data['placement_events']}
data['unique_provider_nodes']={p:sorted({e['name'] for e in nodes if e['args']['provider']==p}) for p in data['placement_events']}
target.write_text(json.dumps(data,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({'profile':str(profile),'events':data['placement_events'],'unique_nodes':{p:len(v) for p,v in data['unique_provider_nodes'].items()},'duration_us':data['provider_node_duration_us']}))
