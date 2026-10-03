#!/usr/bin/env python3
"""Build the out-of-domain control set for the diarization eval (issue #284).

Clips from medkit/simsamu (MIT licence, https://huggingface.co/datasets/medkit/simsamu):
simulated French emergency-dispatch phone calls with reference RTTMs, plus the Norwegian
`nb_samtale_nb12` conversation reconstructed by test-audio/diarization/fetch.sh.
None of these were used to choose the clustering parameters.
Audio goes to the scratch dir; RTTMs and manifest-other.json go into docs/benchmarks/data/diarization-sv/.
"""
import hashlib, json, shutil, subprocess, urllib.request
from pathlib import Path

SCRATCH = Path.home() / ".cache/sagascript-bench/diar-284"
OUT = Path(__file__).resolve().parents[2] / "docs/benchmarks/data/diarization-sv"
BASE = "https://huggingface.co/datasets/medkit/simsamu/resolve/main"
CLIPS = ["dj_2022_feu", "dj_2022_grand_mere_battue", "dj_2022_intox_med", "dj_2023_coups",
         "dj_2022_avc_16_ans", "dj_2022_douleur_abdo", "dj_2022_mere_fievre"]

def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()

def main():
    d = SCRATCH / "other"; d.mkdir(parents=True, exist_ok=True)
    man = []
    for c in CLIPS:
        m4a, wav, rttm = d / f"{c}.m4a", d / f"{c}.wav", d / f"{c}.rttm"
        for ext, p in (("m4a", m4a), ("rttm", rttm)):
            if not p.exists():
                urllib.request.urlretrieve(f"{BASE}/{c}/{c}.{ext}", p)
        if not wav.exists():
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", str(m4a), "-ac", "1", "-ar", "16000", str(wav)], check=True)
        shutil.copy(rttm, OUT / f"other-{c}.rttm")
        t = [l.split() for l in open(rttm) if l.startswith("SPEAKER")]
        man.append({"id": c, "domain": "fr-phone-dispatch", "source": f"{BASE}/{c}/{c}.m4a", "licence": "MIT (medkit/simsamu)",
                    "source_sha256": sha(m4a), "wav_16k_mono_sha256": sha(wav),
                    "duration_seconds": round(max(float(x[3]) + float(x[4]) for x in t), 1),
                    "reference_speakers": len({x[7] for x in t})})
    # Norwegian conversation: built by test-audio/diarization/fetch.sh from Sprakbanken/nb_samtale (CC0)
    nb = d / "nb_samtale_nb12.wav"
    ref = Path(__file__).resolve().parents[2] / "test-audio/diarization/nb_samtale_nb12_ref.rttm"
    if nb.exists():
        shutil.copy(ref, OUT / "other-nb_samtale_nb12.rttm")
        t = [l.split() for l in open(ref) if l.startswith("SPEAKER")]
        man.append({"id": "nb_samtale_nb12", "domain": "no-conversation", "source": "https://huggingface.co/datasets/Sprakbanken/nb_samtale (recording nb-12, per-turn WAVs concatenated by test-audio/diarization/fetch.sh)",
                    "licence": "CC0-1.0 (Sprakbanken/nb_samtale)", "wav_16k_mono_sha256": sha(nb),
                    "duration_seconds": round(max(float(x[3]) + float(x[4]) for x in t), 1),
                    "reference_speakers": len({x[7] for x in t})})
    json.dump(man, open(OUT / "manifest-other.json", "w"), indent=1)

if __name__ == "__main__":
    main()
