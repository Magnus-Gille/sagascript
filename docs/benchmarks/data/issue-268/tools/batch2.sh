#!/bin/sh
cd $HOME/.cache/sagascript-bench/issue-268
./gate.sh > results/gate2.log 2>&1
(cd ../bench && ./.venv/bin/python bench.py --spec ../issue-268/spec2.json --files fleurs-sv-distinct-5min,fleurs-sv-distinct-15min,fleurs-sv-distinct-60min --reps 3 --out ../issue-268/results/timing-long-skew.jsonl > ../issue-268/results/timing-long-skew.log 2>&1)
(cd ../bench && ./.venv/bin/python bench.py --spec ../issue-268/spec2.json --files fleurs-sv-distinct-30min --reps 1 --only r2s-30s,r2s-40s --out ../issue-268/results/wer-30min-skew.jsonl > ../issue-268/results/wer-30min-skew.log 2>&1)
for v in s30 s40; do
  ./gate.sh >/dev/null; m=$(grep MODEL_DIR run-$v.sh | cut -d'"' -f2)
  ./latency.py "$m" cache-$v $v 2>&1 | tail -1 >> results/latency-skew.jsonl
  SAGASCRIPT_ENGINE_HOST_MODEL_DIR="$m" SAGASCRIPT_ENGINE_HOST_BIN=/Applications/Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host SAGASCRIPT_ENGINE_HOST_CACHE_DIR=$PWD/cache-$v ./rss60.py $v 2>&1 | tail -1 >> results/rss-skew.jsonl
done
./gate.sh >/dev/null; ./latency.py "$HOME/Library/Application Support/Sagascript/Models/pianissimo-sv-coreml-r1" cache-r1 r1-again 2>&1 | tail -1 >> results/latency-skew.jsonl
for v in s30 s40; do HOST_CACHE=$PWD/cache-$v ./perclip.py host:$PWD/models/pianissimo-sv-coreml-r2s-${v#s}s results/perclip-$v.json > results/perclip-$v.log 2>&1; done
echo done > results/batch2.done
