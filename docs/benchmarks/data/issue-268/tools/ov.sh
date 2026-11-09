#!/bin/bash
cd $HOME/.cache/sagascript-bench/issue-268
./gate.sh >/dev/null
for cfg in "s40 4" "s40 12" "s30 4" "s30 10"; do
  set -- $cfg
  SAGASCRIPT_ENGINE_OVERLAP_S=$2 ./run-$1.sh --input ../longform/fleurs-sv-distinct-15min.wav --output /tmp/ov.json --variant x-ane --warmup ../longform/fleurs-sv-1min.wav 2>&1 | tail -2
  ../bench/.venv/bin/python - "$1" "$2" <<'PY'
import json,sys
sys.path.insert(0,"../bench")
from norm import norm
import jiwer
d=json.load(open("/tmp/ov.json")); ref=open("../longform/fleurs-sv-distinct-15min.txt").read()
o=jiwer.process_words(norm(ref),norm(d["text"]))
print(sys.argv[1],"overlap",sys.argv[2],"errors",o.substitutions+o.deletions+o.insertions,"wer",round(jiwer.wer(norm(ref),norm(d["text"]))*100,2),"tx",round(d["transcribe_s"],2))
PY
done
