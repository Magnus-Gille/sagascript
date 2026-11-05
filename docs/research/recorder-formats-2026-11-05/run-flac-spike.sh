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

echo "=== soak: 60 repeats = about 58 minutes of 48 kHz / 24-bit mono, streaming, peak memory"
/usr/bin/time -l $BIN stream $IN $OUT/rs_soak.flac 1 60 2>&1 | egrep 'stream:|encode time|maximum resident|real'
echo "soak output size: $(size $OUT/rs_soak.flac) bytes"
flac -t -s $OUT/rs_soak.flac && echo "soak: flac -t OK (MD5 of 58 min matches)"
rm -f $OUT/rs_soak.flac
