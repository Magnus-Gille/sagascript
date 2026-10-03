import subprocess,os,sys
B=os.path.expanduser('~/.cache/sagascript-bench/bias-298'); L=os.path.expanduser('~/.cache/sagascript-bench/longform')
wav={'f60':L+'/fleurs-sv-distinct-60min.wav','rik':os.path.expanduser('~/.cache/sagascript-bench/riksdag/debate.wav'),'nn':B+'/nonames/nonames.wav'}
jobs=[]  # (label, audio, dict, weight, sf)  sf=None -> host default (now 0)
def add(label,audio,d,w,sf=None): jobs.append((label,audio,d,w,sf))
add('f60-w0','f60','-',0); add('rik-w0','rik','-',0); add('nn-w0','nn','-',0)
for a in ['nn','f60','rik']:
    d='large-500' if a=='nn' else 'large-distractors-only'
    for w in [1,2,3,5]: add(f'c-{a}-w{w}',a,d,w)
    for w in [3,5]: add(f'c-{a}-w{w}-sf25',a,d,w,'0.25')
for N in [100,400]:
    for s in range(1,6):
        for w in [3,5]: add(f'a-f60-N{N}-s{s}-w{w}','f60',f'f60-N{N}-s{s}',w)
        add(f'a-f60-N{N}-s{s}-w5-sf25','f60',f'f60-N{N}-s{s}',5,'0.25')
        if N==400: add(f'a-f60-N{N}-s{s}-w3-sf25','f60',f'f60-N{N}-s{s}',3,'0.25')
for s in range(1,6):
    for w in [3,5]: add(f'a-rik-N400-s{s}-w{w}','rik',f'rik-N400-s{s}',w)
    for w in [3,5]: add(f'a-rik-N400-s{s}-w{w}-sf25','rik',f'rik-N400-s{s}',w,'0.25')
for label,a,d,w,sf in jobs:
    if os.path.exists(f'{B}/runs2/{label}.json') and os.path.getsize(f'{B}/runs2/{label}.json')>1000: continue
    env=dict(os.environ)
    env.pop('SAGASCRIPT_BOOST_START_FACTOR',None)
    if sf: env['SAGASCRIPT_BOOST_START_FACTOR']=sf
    t='-' if d=='-' else f'{B}/dicts/{d}.txt'
    subprocess.run(['/usr/bin/lockf','/tmp/sagascript-build.lock',f'{B}/run2.sh',label,wav[a],t,str(w)],env=env)
open(B+'/EVALDONE','w').write('done')
