#!/bin/bash
# Offline two-track evaluation (no recording). Usage:
#   run_eval.sh CLI SCRATCH_DIR SV2_WAV_DIR RTTM_DIR [CROP]
# Needs the bench venv python (numpy, scipy) as $PY and the kb-whisper-tiny and
# diarization models. Writes synthetic audio to SCRATCH_DIR (never commit it) and
# result JSON to SCRATCH_DIR/results/.
set -euo pipefail
CLI=$1; S=$2; WAVS=$3; RTTMS=$4; CROP=${5:-0:420}
PY=${PY:-python3}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$S/results"
# id|speaker that plays "me"
for spec in "IP_hc10606|Jessica_Rodén_(S)" "IP_hd10115|Jessica_Rodén_(S)"; do
  id=${spec%%|*}; me=${spec#*|}
  for leak in clean leak15; do
    args=(); [ "$leak" = leak15 ] && args=(--leak-db -15)
    wav="$S/$id.$leak.wav"; ref="$S/$id.ref.rttm"
    $PY "$HERE/build_synthetic.py" --wav "$WAVS/$id.wav" --rttm "$RTTMS/sv2-$id.rttm" --me "$me" --crop "$CROP" \
        ${args[@]+"${args[@]}"} --out "$wav" --ref-out "$ref"
    $PY "$HERE/eval_two_track.py" --cli "$CLI" --wav "$wav" --ref "$ref" --me "$me" --label "$id $leak guard=on" > "$S/results/$id.$leak.on.json"
    if [ "$leak" != clean ]; then
      $PY "$HERE/eval_two_track.py" --cli "$CLI" --wav "$wav" --ref "$ref" --me "$me" --label "$id $leak guard=off" --guard-off --no-baseline > "$S/results/$id.$leak.off.json"
    fi
    rm -f "$wav"
  done
done
