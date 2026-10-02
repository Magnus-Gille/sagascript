#!/bin/bash
# usage: run_batch.sh label termsfile|- weight listfile   (env passes through)
label=$1; terms=$2; w=$3; list=$4
B=/Users/magnus/.cache/sagascript-bench/bias-298
WT=/Users/magnus/repos/sagascript-issue-298-biasing
export HOME=$B/home
export SAGASCRIPT_ENGINE_HOST=$WT/src-tauri/engine-host/coreml/.build/out/Products/Release/sagascript-engine-host
export SAGASCRIPT_PIANISSIMO_MODEL_DIR="/Users/magnus/Library/Application Support/Sagascript/Models/pianissimo-sv-coreml-r1"
args=(transcribe --model pianissimo-sv --language sv --json)
[ "$w" != "0" ] && args+=(--glossary-boost $w --boost-terms-file $terms)
$WT/src-tauri/target/debug/sagascript "${args[@]}" $(cat $list) > $B/runs2/$label.json 2> $B/runs2/$label.err
echo "$label rc=$?" >> $B/runs2/times.log
