#!/bin/bash
# usage: run2.sh label wav termsfile|- weight   (env passes through, e.g. SAGASCRIPT_BOOST_BLANK_MARGIN)
label=$1; wav=$2; terms=$3; w=$4
B=/Users/magnus/.cache/sagascript-bench/bias-298
WT=/Users/magnus/repos/sagascript-issue-298-biasing
export HOME=$B/home
export SAGASCRIPT_ENGINE_HOST=$WT/src-tauri/engine-host/coreml/.build/out/Products/Release/sagascript-engine-host
export SAGASCRIPT_PIANISSIMO_MODEL_DIR="/Users/magnus/Library/Application Support/Sagascript/Models/pianissimo-sv-coreml-r1"
args=(transcribe --model pianissimo-sv --language sv --json)
[ "$w" != "0" ] && args+=(--glossary-boost $w --boost-terms-file $terms)
s=$(date +%s.%N)
$WT/src-tauri/target/debug/sagascript "${args[@]}" $wav > $B/runs2/$label.json 2> $B/runs2/$label.err
rc=$?; e=$(date +%s.%N)
echo "$label rc=$rc seconds=$(echo "$e - $s"|bc)" >> $B/runs2/times.log
