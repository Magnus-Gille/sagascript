#!/usr/bin/env python3
"""Telephone-band copies of Swedish eval clips (issue #284): stands in for call audio until real
Swedish call recordings exist. 300-3400 Hz band-pass, 8 kHz, then back to 16 kHz mono for the
pipeline. Audio stays in the scratch dir; manifest-degraded.json records the recipe and hashes."""
import hashlib, json, subprocess
from pathlib import Path
SCRATCH = Path.home() / ".cache/sagascript-bench/diar-284"
OUT = Path(__file__).resolve().parents[2] / "docs/benchmarks/data/diarization-sv"
# (source set, source id, source wav relative to scratch)
SRC = [("sv2", "IP_hd10256", "sv2/IP_hd10256.wav"), ("sv2", "IP_hc10606", "sv2/IP_hc10606.wav"),
       ("sv2", "IP_hd10562", "sv2/IP_hd10562.wav"), ("sv2", "FS_20260611", "sv2/FS_20260611.wav"),
       ("riksdag", "SoU38", "SoU38.wav"), ("riksdag", "UU24", "UU24.wav")]
AF = "highpass=f=300,lowpass=f=3400,aresample=8000,aresample=16000"
def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""): h.update(b)
    return h.hexdigest()
def main():
    d = SCRATCH / "sv2/deg"; d.mkdir(parents=True, exist_ok=True); man = []
    for sset, sid, rel in SRC:
        out = d / f"{sid}_tel.wav"
        if not out.exists():
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(SCRATCH / rel), "-af", AF, "-ac", "1", "-ar", "16000", str(out)], check=True)
        srcman = json.load(open(OUT / ("manifest.json" if sset == "riksdag" else "manifest-sv2.json")))
        m = next(x for x in srcman if x["id"] == sid)
        man.append({"id": f"{sid}_tel", "source_set": sset, "source_id": sid, "ffmpeg_af": AF,
                    "wav_16k_mono_sha256": sha(out), "reference_speakers": m["reference_speakers"]})
    json.dump(man, open(OUT / "manifest-degraded.json", "w"), indent=1)
if __name__ == "__main__": main()
