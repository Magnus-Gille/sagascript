#!/bin/bash
# SAGA-547 recorder format measurements. Offline and read-only with respect to the repository.
# Needs (macOS): afconvert, afinfo, flac, ffmpeg (with libopus), sox, xxd, python3, bc.
# Usage, from inside the repository:  WORK=$(mktemp -d) bash docs/research/recorder-formats-2026-11-05/measure-formats.sh
# The input is a repository fixture. Nothing is recorded from any microphone or output device.
set -u
D=${WORK:?set WORK to an empty scratch directory}
# The default input is git-ignored: run test-audio/diarization/fetch.sh first, or set SRC=<any mono speech wav>.
SRC=${SRC:-${REPO:-$(git rev-parse --show-toplevel)}/test-audio/diarization/dj_2022_feu.wav}
[ -f "$SRC" ] || { echo "missing input $SRC (see the note above)"; exit 1; }
cd "$D" || exit 1
TIMEFORMAT='%R'
size() { stat -f %z "$1"; }

# --- Source signal -------------------------------------------------------------------------
# 58.6 s of Swedish speech (16 kHz mono fixture) converted to 48 kHz / 24-bit and mixed with a
# -54 dBFS pink-noise floor, so lossless ratios are not flattered by a digitally silent background.
# Caveat: the speech is band-limited at 8 kHz; a real 48 kHz microphone has wideband noise, so real
# FLAC ratios will be somewhat worse than measured here.
sox "$SRC" -r 48000 -b 24 speech48.wav
DUR=$(soxi -D speech48.wav)
sox -n -r 48000 -b 24 -c 1 floor.wav synth "$DUR" pinknoise vol 0.002
sox -m -v 1 speech48.wav -v 1 floor.wav src48_24.wav
echo "source: $(soxi -D src48_24.wav) s, $(soxi -r src48_24.wav) Hz, $(soxi -b src48_24.wav)-bit, $(soxi -c src48_24.wav) ch"
echo "RAW PCM md5 (source): $(sox src48_24.wav -t raw -e signed -b 24 - | md5)"

# --- 1. Encode time (single run each; process start included, so indicative only) -------------
t() { local label=$1; shift; local s; s=$( { time "$@" >/dev/null 2>&1; } 2>&1 ); echo "$label encode_seconds=$s"; }
echo "== encode"
t flac_c0_libFLAC   flac -s -0 -f -o f0.flac src48_24.wav
t flac_c5_libFLAC   flac -s -5 -f -o f5.flac src48_24.wav
t flac_c8_libFLAC   flac -s -8 -f -o f8.flac src48_24.wav
t flac_ffmpeg       ffmpeg -nostdin -y -v error -i src48_24.wav -c:a flac ff.flac
t flac_afconvert    afconvert src48_24.wav af.flac -f flac -d flac
t alac_afconvert    afconvert src48_24.wav alac.m4a -f m4af -d alac
t aac64_afconvert   afconvert src48_24.wav aac64.m4a -f m4af -d aac -b 64000
t aac128_afconvert  afconvert src48_24.wav aac128.m4a -f m4af -d aac -b 128000
t opus32_ffmpeg     ffmpeg -nostdin -y -v error -i src48_24.wav -c:a libopus -b:a 32k opus32.ogg
t opus64_ffmpeg     ffmpeg -nostdin -y -v error -i src48_24.wav -c:a libopus -b:a 64k opus64.ogg

# --- 2. Size ----------------------------------------------------------------------------------
echo "== sizes (bytes) and ratio vs WAV 24-bit"
W=$(size src48_24.wav)
for f in src48_24.wav f0.flac f5.flac f8.flac ff.flac af.flac alac.m4a aac64.m4a aac128.m4a opus32.ogg opus64.ogg; do
  S=$(size $f); printf '%-14s %10d  %5.1f%%\n' $f $S $(echo "scale=1; 100*$S/$W" | bc)
done

# --- 3. Lossless round trip across encoders and decoders ---------------------------------------
echo "== lossless round trip (raw 24-bit PCM md5 must equal the source md5 above)"
rt() { local label=$1; shift; local m; m=$("$@" 2>/dev/null | md5); echo "$label md5=$m"; }
rt flac_libFLAC_decode      bash -c 'flac -d -s -c f5.flac | sox -t wav - -t raw -e signed -b 24 -'
rt flac_afconvert_decode    bash -c 'afconvert af.flac rt_af.wav -f WAVE -d LEI24 && sox rt_af.wav -t raw -e signed -b 24 -'
rt alac_afconvert_decode    bash -c 'afconvert alac.m4a rt_alac.wav -f WAVE -d LEI24 && sox rt_alac.wav -t raw -e signed -b 24 -'
rt flac_ffmpeg_to_libFLAC   bash -c 'flac -d -s -c ff.flac | sox -t wav - -t raw -e signed -b 24 -'
rt afconvert_flac_to_ffmpeg bash -c 'ffmpeg -nostdin -v error -i af.flac -f s24le -'

# --- 4. Crash tolerance: file cut at 60 % ------------------------------------------------------
# CoreAudio (via afconvert) is used as a proxy for the macOS system decoders. It is NOT a test of
# WKWebView or WebView2 playback.
echo "== file cut at 60 percent: decoded seconds (full duration $DUR s)"
cut60() { head -c $(( $(size $1) * 60 / 100 )) "$1" > "cut_$1"; }
for f in f5.flac af.flac alac.m4a aac64.m4a opus32.ogg src48_24.wav; do cut60 $f; done
dec_ffmpeg() { local b; b=$(ffmpeg -nostdin -v error -i "$1" -ar 48000 -ac 1 -f s16le - 2>/dev/null | wc -c); echo "scale=1; $b/96000" | bc; }
for f in f5.flac af.flac alac.m4a aac64.m4a opus32.ogg src48_24.wav; do
  if afconvert "cut_$f" "cut_$f.coreaudio.wav" -f WAVE -d LEI16 >/dev/null 2>&1; then
    ca=$(afinfo "cut_$f.coreaudio.wav" 2>/dev/null | awk -F': ' '/estimated duration/ {print $2}')
  else ca="FAIL"; fi
  printf '%-14s ffmpeg=%ss  coreaudio=%s\n' "$f" "$(dec_ffmpeg cut_$f)" "$ca"
done

echo "== WAV header left unfixed after a crash, cut at 60 percent"
ffmpeg -nostdin -y -v error -i src48_24.wav -f wav -c:a pcm_s24le - 2>/dev/null > stream.wav   # unseekable output: size fields = 0xFFFFFFFF
head -c $(( $(size stream.wav) * 60 / 100 )) stream.wav > cut_ffff.wav
python3 -I - <<'EOF'
import struct
b = bytearray(open('cut_ffff.wav', 'rb').read())
i = b.find(b'data')
struct.pack_into('<I', b, 4, 0)
struct.pack_into('<I', b, i + 4, 0)
open('cut_zero.wav', 'wb').write(b)
EOF
for f in cut_ffff.wav cut_zero.wav; do
  if afconvert $f $f.ca.wav -f WAVE -d LEI16 >/dev/null 2>&1; then ca=$(afinfo $f.ca.wav | awk -F': ' '/estimated duration/ {print $2}'); else ca=FAIL; fi
  printf '%-14s ffmpeg=%ss  coreaudio=%s\n' "$f" "$(dec_ffmpeg $f)" "$ca"
done
echo "(cut_ffff.wav: RIFF and data sizes 0xFFFFFFFF; cut_zero.wav: both sizes 0)"

# --- 5. Quality tiers: size per hour of mono speech --------------------------------------------
echo "== size per hour (MB), same 58.6 s signal"
sox src48_24.wav -r 16000 -b 16 t_16k16.wav
sox src48_24.wav -r 24000 -b 16 t_24k16.wav
sox src48_24.wav -r 48000 -b 16 t_48k16.wav
flac -s -5 -f -o t_16k16.flac t_16k16.wav
flac -s -5 -f -o t_24k16.flac t_24k16.wav
flac -s -5 -f -o t_48k16.flac t_48k16.wav
afconvert src48_24.wav t_aac32.m4a -f m4af -d aac -b 32000
mb() { echo "scale=0; $(size "$1") * 3600 / $DUR / 1000000" | bc; }
printf '%-34s %6s\n' "FLAC 48 kHz / 24-bit"            "$(mb f5.flac)"
printf '%-34s %6s\n' "FLAC 48 kHz / 16-bit"            "$(mb t_48k16.flac)"
printf '%-34s %6s\n' "FLAC 24 kHz / 16-bit"            "$(mb t_24k16.flac)"
printf '%-34s %6s\n' "FLAC 16 kHz / 16-bit"            "$(mb t_16k16.flac)"
printf '%-34s %6s\n' "AAC-LC 64 kbit/s (M4A)"          "$(mb aac64.m4a)"
printf '%-34s %6s\n' "AAC-LC 32 kbit/s (M4A)"          "$(mb t_aac32.m4a)"
printf '%-34s %6s\n' "WAV 48 kHz / 24-bit"             "$(mb src48_24.wav)"
printf '%-34s %6s\n' "WAV 16 kHz / 16-bit (record -o)" "$(mb t_16k16.wav)"
echo "classic WAV (4 GiB) limit, 48 kHz / 24-bit: $(echo "scale=1; 4294967295/(48000*3)/3600" | bc) h mono, $(echo "scale=1; 4294967295/(48000*3*2)/3600" | bc) h with two tracks in one file"
