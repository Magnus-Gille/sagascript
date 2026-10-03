#!/usr/bin/env python3
"""Build a synthetic two-track WAV from a mono Riksdag recording plus its RTTM.

Left (microphone, "me") carries only the turns of one reference speaker, silence
elsewhere; right (system audio, "the others") carries everything else. With
--leak-db the microphone also picks up a delayed, low-passed, attenuated copy of
the right channel (a listener without headphones). The result is a plain 16 kHz
16-bit stereo WAV; use `transcribe --two-track` (no marker is written here) or
let build_synthetic add the marker with --marker (done by default).

Needs numpy only. Never commit the audio this produces.

  build_synthetic.py --wav IN.wav --rttm REF.rttm --me "Jessica_Rodén_(S)" \
      --crop 0:600 [--leak-db -20] --out OUT.wav [--ref-out OUT.rttm]
"""
import argparse, struct, wave
import numpy as np

SR = 16000

def read_rttm(path):
    turns = []
    for line in open(path):
        f = line.split()
        if f and f[0] == "SPEAKER":
            turns.append((float(f[3]), float(f[3]) + float(f[4]), f[7]))
    return turns

def read_wav(path):
    with wave.open(path) as w:
        assert w.getframerate() == SR and w.getsampwidth() == 2 and w.getnchannels() == 1, "need 16 kHz mono 16-bit"
        return np.frombuffer(w.readframes(w.getnframes()), dtype="<i2").astype(np.float32) / 32768.0

def marker_list_chunk():
    """The same LIST/INFO/ICMT marker the Rust recorder writes."""
    def sub(i, text):
        body = text.encode() + b"\0"
        return i + struct.pack("<I", len(body)) + body + (b"\0" if len(body) % 2 else b"")
    info = b"INFO" + sub(b"ISFT", "Sagascript synthetic") + sub(b"ICMT", "sagascript-two-track/1 left=microphone right=system recorder=synthetic")
    return b"LIST" + struct.pack("<I", len(info)) + info

def write_two_track(path, left, right, marker=True):
    n = max(len(left), len(right))
    st = np.zeros((n, 2), np.float32); st[:len(left), 0] = left; st[:len(right), 1] = right
    pcm = (np.clip(st, -1, 1) * 32767).astype("<i2").tobytes()
    extra = marker_list_chunk() if marker else b""
    fmt = struct.pack("<HHIIHH", 1, 2, SR, SR * 4, 4, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", 16) + fmt + extra + b"data" + struct.pack("<I", len(pcm)) + pcm
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", len(body)) + body)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--wav", required=True); ap.add_argument("--rttm", required=True)
    ap.add_argument("--me", required=True, help="reference speaker label that plays the local user")
    ap.add_argument("--crop", default=None, help="START:END seconds")
    ap.add_argument("--leak-db", type=float, default=None, help="level of the right channel leaking into the microphone, in dB")
    ap.add_argument("--leak-delay-ms", type=float, default=40.0)
    ap.add_argument("--double-talk", action="store_true",
                    help="also play unreferenced remote speech (the others' audio shifted by 60 s) on the system channel while the local user speaks")
    ap.add_argument("--me-only-out", help="mono WAV of the local user's turns only, gaps shortened, for a reference transcript")
    ap.add_argument("--no-marker", action="store_true")
    ap.add_argument("--out", required=True); ap.add_argument("--ref-out")
    a = ap.parse_args()
    audio = read_wav(a.wav); turns = read_rttm(a.rttm)
    t0, t1 = (0.0, len(audio) / SR)
    if a.crop:
        t0, t1 = (float(x) for x in a.crop.split(":")); t1 = min(t1, len(audio) / SR)
    audio = audio[int(t0 * SR):int(t1 * SR)]
    turns = [(max(s, t0) - t0, min(e, t1) - t0, n) for s, e, n in turns if e > t0 and s < t1]
    labels = {n for _, _, n in turns}
    assert a.me in labels, f"{a.me!r} not in reference: {sorted(labels)}"
    mask = np.zeros(len(audio), np.float32)
    for s, e, n in turns:
        if n == a.me:
            mask[int(s * SR):int(e * SR)] = 1.0
    k = np.ones(int(0.02 * SR)) / int(0.02 * SR)  # 20 ms fades, no clicks
    mask = np.clip(np.convolve(mask, k, mode="same"), 0, 1)
    left = audio * mask
    right = audio * (1.0 - mask)
    if a.double_talk:
        right = right + np.roll(right, 60 * SR) * mask
    if a.leak_db is not None:
        d = int(a.leak_delay_ms / 1000 * SR)
        leak = np.convolve(np.concatenate([np.zeros(d, np.float32), right])[:len(right)], np.ones(4) / 4, mode="same")
        left = left + (10 ** (a.leak_db / 20)) * leak
    write_two_track(a.out, left, right, marker=not a.no_marker)
    if a.me_only_out:
        parts = []
        for s, e, n in sorted(turns):
            if n == a.me:
                parts += [audio[int(s * SR):int(e * SR)], np.zeros(int(0.4 * SR), np.float32)]
        pcm = (np.clip(np.concatenate(parts), -1, 1) * 32767).astype("<i2").tobytes()
        with wave.open(a.me_only_out, "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR); w.writeframes(pcm)
    if a.ref_out:
        with open(a.ref_out, "w") as f:
            for s, e, n in turns:
                f.write(f"SPEAKER synth 1 {s:.3f} {e - s:.3f} <NA> <NA> {n} <NA> <NA>\n")
    print(f"{a.out}: {len(audio) / SR:.0f}s, me={a.me}, leak={a.leak_db}")

if __name__ == "__main__":
    main()
