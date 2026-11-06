"""Make a documented long-file stress fixture from the existing public corpus."""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import wave
import numpy as np
sys.path.insert(0, str(Path(__file__).resolve().parent))
from bench import pcm

p=argparse.ArgumentParser()
p.add_argument('--lab', required=True, type=Path)
a=p.parse_args()
corpus=json.loads((a.lab/'corpus.json').read_text(encoding='utf-8'))
samples=np.concatenate([pcm(c['file']) for c in corpus])
target=a.lab/'swedish-composite-long.wav'
with wave.open(str(target),'wb') as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(16000)
    w.writeframes((np.clip(samples,-1,32767/32768)*32768).astype('<i2').tobytes())
meta={'file':str(target),'audio_s':len(samples)/16000,
      'reference':' '.join(c['reference'] for c in corpus),
      'sources':[c['id'] for c in corpus],
      'sha256':hashlib.sha256(target.read_bytes()).hexdigest(),
      'note':'Synthetic concatenation of 11 public FLEURS clips; not a natural meeting.'}
(a.lab/'long-fixture.json').write_text(json.dumps(meta,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps({'file':str(target),'audio_s':meta['audio_s']}))
