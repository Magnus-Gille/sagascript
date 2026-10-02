#!/bin/bash
# usage: run.sh label wav termsfile|- weight
label=$1; wav=$2; terms=$3; w=$4
export HOME=$HOME/.cache/sagascript-bench/bias-298/home
export SAGASCRIPT_ENGINE_HOST=$HOME/repos/sagascript-issue-298-biasing/src-tauri/engine-host/coreml/.build/out/Products/Release/sagascript-engine-host
export SAGASCRIPT_PIANISSIMO_MODEL_DIR="$HOME/Library/Application Support/Sagascript/Models/pianissimo-sv-coreml-r1"
args=(transcribe --model pianissimo-sv --language sv)
[ "$w" != "0" ] && args+=(--glossary-boost $w --boost-terms-file $terms)
s=$(date +%s.%N)
$HOME/repos/sagascript-issue-298-biasing/src-tauri/target/debug/sagascript "${args[@]}" $wav > $HOME/.cache/sagascript-bench/bias-298/runs/$label.txt 2> $HOME/.cache/sagascript-bench/bias-298/runs/$label.err
rc=$?; e=$(date +%s.%N)
echo "$label rc=$rc seconds=$(echo "$e - $s"|bc)" >> $HOME/.cache/sagascript-bench/bias-298/runs/times.log
