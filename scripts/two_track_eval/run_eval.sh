#!/bin/bash
# Offline two-track evaluation (no recording). Usage:
#   run_eval.sh CLI SCRATCH_DIR SV2_WAV_DIR RTTM_DIR "MODEL ..." [CROP]
# Guard modes: auto (default), off, and on (tiny model only: auto applies it when echo is
# detected, so on == auto there).
# Variants per file: clean; leak15 (right channel leaks into the microphone at
# -15 dB); dt15 (leak plus remote speech playing while the user talks). Needs
# $PY (numpy, scipy), the named Whisper models and the diarization models.
# Synthetic audio goes to SCRATCH_DIR (never commit it); JSON to SCRATCH_DIR/results/.
set -euo pipefail
CLI=$1; S=$2; WAVS=$3; RTTMS=$4; MODELS=${5:-kb-whisper-tiny}; CROP=${6:-0:420}
PY=${PY:-python3}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$S/results"
for spec in "IP_hc10606|Jessica_Rodén_(S)" "IP_hd10115|Jessica_Rodén_(S)"; do
  id=${spec%%|*}; me=${spec#*|}
  for variant in clean leak15 dt15; do
    args=()
    [ "$variant" != clean ] && args+=(--leak-db -15)
    [ "$variant" = dt15 ] && args+=(--double-talk)
    wav="$S/$id.$variant.wav"; ref="$S/$id.ref.rttm"; meonly="$S/$id.meonly.wav"
    $PY "$HERE/build_synthetic.py" --wav "$WAVS/$id.wav" --rttm "$RTTMS/sv2-$id.rttm" --me "$me" --crop "$CROP" \
        ${args[@]+"${args[@]}"} --out "$wav" --ref-out "$ref" --me-only-out "$meonly"
    for model in $MODELS; do
      reftxt="$S/$id.$model.meref.txt"
      [ -s "$reftxt" ] || "$CLI" transcribe "$meonly" --language sv --model "$model" > "$reftxt" 2>/dev/null
      tag="$model.$id.$variant"
      # Default (auto) with the downmix baseline; then forced off and on.
      $PY "$HERE/eval_two_track.py" --cli "$CLI" --wav "$wav" --ref "$ref" --me "$me" --model "$model" --me-ref-text "$reftxt" \
          --label "$tag guard=auto" --guard auto > "$S/results/$tag.auto.json"
      if [ "$variant" != clean ]; then
        $PY "$HERE/eval_two_track.py" --cli "$CLI" --wav "$wav" --ref "$ref" --me "$me" --model "$model" --me-ref-text "$reftxt" \
            --label "$tag guard=off" --guard off --no-baseline > "$S/results/$tag.off.json"
        if [ "$model" = kb-whisper-tiny ]; then
          $PY "$HERE/eval_two_track.py" --cli "$CLI" --wav "$wav" --ref "$ref" --me "$me" --model "$model" \
              --me-ref-text "$reftxt" --label "$tag guard=on" --guard on --no-baseline > "$S/results/$tag.on.json"
        fi
      fi
    done
    rm -f "$wav"
  done
done
