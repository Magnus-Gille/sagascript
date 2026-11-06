"""Summarize saved results; one quality observation per unique clip, not per repeat."""
import argparse
import json
from pathlib import Path
import statistics

p=argparse.ArgumentParser()
p.add_argument('--lab',required=True,type=Path)
a=p.parse_args()
summary=[]
baseline=json.loads((a.lab/'cpu-shipped-final.json').read_text(encoding='utf-8'))
base={r['id']:r for r in baseline['runs'] if r['iteration']==0}
for path in sorted(a.lab.glob('*.json')):
    if 'profile' in path.name or path.name.endswith('-process.json'):
        continue
    try:
        data=json.loads(path.read_text(encoding='utf-8'))
    except (ValueError,UnicodeError):
        continue
    if not isinstance(data,dict) or not isinstance(data.get('runs'),list):
        continue
    runs=[r for r in data['runs'] if isinstance(r,dict) and 'wall_ms' in r]
    row={'label':data.get('label',path.stem),'status':data.get('status'),
         'error':data.get('error') or data.get('errors'), 'runs':len(runs),
         'load_ms':data.get('encoder_load_ms',data.get('load_wall_ms',data.get('load_ms')))}
    if runs:
        first={r['id']:r for r in runs if r['iteration']==0}
        warm=[r for r in runs if r['iteration']>0] or runs
        errors=sum(r['quality']['errors'] for r in first.values())
        words=sum(r['quality']['words'] for r in first.values())
        row.update(clips=len(first),audio_s=sum(r['audio_s'] for r in first.values()),
                   median_warm_ms=statistics.median(r['wall_ms'] for r in warm),
                   warm_min_ms=min(r['wall_ms'] for r in warm),warm_max_ms=max(r['wall_ms'] for r in warm),
                   warm_rtf=sum(r['wall_ms'] for r in warm)/1000/sum(r['audio_s'] for r in warm),
                   errors=errors,reference_words=words,wer=errors/max(words,1),
                   text_mismatches_vs_shipped=[k for k,r in first.items() if k in base and r['text']!=base[k]['text']],
                   max_rss_mb=max(r['rss_mb'] for r in runs))
        if data.get('placement_events'):
            row['placement_events']=data['placement_events']
            row['unique_provider_nodes']={k:len(v) for k,v in data['unique_provider_nodes'].items()}
            row['provider_node_duration_us']=data['provider_node_duration_us']
    procpath=a.lab/(path.stem+'-process.json')
    if procpath.exists():
        proc=json.loads(procpath.read_text())
        row['process']={k:proc.get(k) for k in ['exit_code','elapsed_s','peak_sampled_rss_mb','status']}
    summary.append(row)
(a.lab/'summary.json').write_text(json.dumps(summary,indent=2),encoding='utf-8')
print(json.dumps(summary,indent=2))
