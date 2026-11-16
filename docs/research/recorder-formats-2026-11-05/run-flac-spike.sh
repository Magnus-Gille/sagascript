#!/bin/bash
# SAGA-547: runs the flacenc spike on the fixture built by measure-formats.sh.
# Usage: WORK=<dir used for measure-formats.sh> SPIKE=<dir with flac-spike.* copied to Cargo.toml and src/main.rs, built with cargo build --release> bash run-flac-spike.sh
set -u
WORK=${WORK:?}; SPIKE=${SPIKE:?}
BIN=$SPIKE/target/release/flacbench
IN=$WORK/src48_24.wav
OUT=$WORK
size() { stat -f %z "$1"; }

echo "=== mono: whole-stream encode (flacenc, default config)"
$BIN whole $IN $OUT/rs_whole_mono.flac 1
echo "=== mono: streaming (frame by frame, header rewritten at stop)"
$BIN stream $IN $OUT/rs_stream_mono.flac 1 1
echo "--- verify with claxon (pure Rust decoder), bit-exact vs source"
$BIN verify $IN $OUT/rs_stream_mono.flac 1
echo "--- verify with libFLAC (flac -t checks the MD5 stored in STREAMINFO)"
flac -t -s $OUT/rs_stream_mono.flac && echo "flac -t: OK"
echo "--- independent decode via CoreAudio (afconvert) -> raw md5 equals source?"
afconvert $OUT/rs_stream_mono.flac $OUT/rs_stream_mono.ca.wav -f WAVE -d LEI24 && echo "CoreAudio md5: $(sox $OUT/rs_stream_mono.ca.wav -t raw -e signed -b 24 - | md5)"
echo "source md5:    $(sox $IN -t raw -e signed -b 24 - | md5)"
echo "sizes: libFLAC -5 = $(size $OUT/f5.flac)  flacenc stream = $(size $OUT/rs_stream_mono.flac)  flacenc whole = $(size $OUT/rs_whole_mono.flac)"

echo "=== stereo (second track = quieter, delayed copy)"
$BIN stream $IN $OUT/rs_stream_st.flac 2 1
$BIN verify $IN $OUT/rs_stream_st.flac 2
flac -t -s $OUT/rs_stream_st.flac && echo "flac -t: OK"
echo "stereo size: $(size $OUT/rs_stream_st.flac)"

echo "=== crash: process 'killed' at 60%, header never fixed up"
$BIN stream $IN $OUT/rs_crash.flac 1 1 crash
echo "ffmpeg decoded seconds: $(ffmpeg -nostdin -v error -i $OUT/rs_crash.flac -ar 48000 -ac 1 -f s16le - 2>/dev/null | wc -c | awk '{printf "%.1f", $1/96000}')"
if afconvert $OUT/rs_crash.flac $OUT/rs_crash.ca.wav -f WAVE -d LEI16 >/dev/null 2>&1; then
  echo "CoreAudio decoded: $(afinfo $OUT/rs_crash.ca.wav | awk -F': ' '/estimated duration/ {print $2}')"
else echo "CoreAudio decoded: FAIL"; fi
echo "claxon on crash file:"; $BIN verify $IN $OUT/rs_crash.flac 1 2>&1 | tail -3
echo "flac -t on crash file: $(flac -t -s $OUT/rs_crash.flac 2>&1 | head -2 | tr '\n' ' ')"

echo "=== non-multiple length: 336123 samples = 82 blocks of 4096 + a short final frame of 251"
sox $IN -b 24 $OUT/odd.wav trim 0 336123s
$BIN stream $OUT/odd.wav $OUT/rs_odd.flac 1 1
$BIN verify $OUT/odd.wav $OUT/rs_odd.flac 1
flac -t $OUT/rs_odd.flac 2>&1 | egrep 'WARNING|ERROR|ok'
metaflac --list --block-type=STREAMINFO $OUT/rs_odd.flac | egrep 'blocksize|total samples'
afconvert $OUT/rs_odd.flac $OUT/rs_odd.ca.wav -f WAVE -d LEI24 && echo "CoreAudio md5: $(sox $OUT/rs_odd.ca.wav -t raw -e signed -b 24 - | md5)"
echo "source md5:    $(sox $OUT/odd.wav -t raw -e signed -b 24 - | md5)"

echo "=== torn tail: the crash file cut 1234 bytes short, which lands inside a frame"
head -c $(( $(size $OUT/rs_crash.flac) - 1234 )) $OUT/rs_crash.flac > $OUT/rs_torn.flac
echo "ffmpeg decoded seconds: $(ffmpeg -nostdin -v error -i $OUT/rs_torn.flac -ar 48000 -ac 1 -f s16le - 2>/dev/null | wc -c | awk '{printf "%.1f", $1/96000}')"
afconvert $OUT/rs_torn.flac $OUT/rs_torn.ca.wav -f WAVE -d LEI16 && echo "CoreAudio decoded: $(afinfo $OUT/rs_torn.ca.wav | awk -F': ' '/estimated duration/ {print $2}')"
echo "flac -t on torn file: $(flac -t -s $OUT/rs_torn.flac 2>&1 | head -2 | tr '\n' ' ')"

echo "=== edge-case signals (24-bit, 480000 samples each), default config and with the fixed predictors off"
python3 -I "$(dirname "$0")/gen-edge-signals.py" $OUT
for n in silence dc_max dc_min square_fullscale white_fullscale alternating_extremes single_impulse; do
  for mode in default nofixed; do
    if [ $mode = nofixed ]; then export FLAC_NO_FIXED=1; else unset FLAC_NO_FIXED; fi
    $BIN stream $OUT/$n.wav $OUT/$n.$mode.flac 1 1 | tail -1 | sed "s/^/$n $mode: /"
    echo "  claxon: $($BIN verify $OUT/$n.wav $OUT/$n.$mode.flac 1 2>&1 | egrep 'mismatches|FormatError' | tail -1)  size $(size $OUT/$n.$mode.flac) (wav $(size $OUT/$n.wav))"
    rm -f $OUT/$n.$mode.flac
  done
done
unset FLAC_NO_FIXED

echo "=== soak: 60 repeats = about 58 minutes of 48 kHz / 24-bit mono, streaming, peak memory"
/usr/bin/time -l $BIN stream $IN $OUT/rs_soak.flac 1 60 2>&1 | egrep 'stream:|encode time|maximum resident|real'
echo "soak output size: $(size $OUT/rs_soak.flac) bytes"
flac -t -s $OUT/rs_soak.flac && echo "soak: flac -t OK (MD5 of 58 min matches)"
rm -f $OUT/rs_soak.flac
