#!/usr/bin/env python3
"""Build the extra Swedish Riksdag diarization eval set (issue #284): interpellation debates
(two or three speakers), question time (many speakers, short turns) and a party-leader debate.

Same method as build_refs.py: speaker and speech timing come from the riksdagen.se webb-tv page
JSON (Riksdag open data); one RTTM turn per speech, speeches under 5 s dropped, scored region =
union of speeches. Licence: "Riksdagens oppna data ar fria att anvandas och spridas vidare sa lange
du anger kalla" (https://www.riksdagen.se/sv/dokument-och-lagar/riksdagens-oppna-data/anvandarstod/anvandningsvillkor/).
Source: Sveriges riksdag. Audio goes to the scratch dir; RTTMs + manifest-sv2.json to docs/benchmarks/data/diarization-sv/.
"""
import hashlib, json, re, subprocess, sys, urllib.request
from pathlib import Path

SCRATCH = Path.home() / ".cache/sagascript-bench/diar-284/sv2"
OUT = Path(__file__).resolve().parents[2] / "docs/benchmarks/data/diarization-sv"
# (our id, riksdagen video id, kind, crop seconds or None)
CLIPS = [
    ("IP_hd10256", "hd10256", "interpellation", None),
    ("IP_hc10606", "hc10606", "interpellation", None),
    ("IP_hb10760", "hb10760", "interpellation", None),
    ("IP_hb10618", "hb10618", "interpellation", None),
    ("IP_hd10562", "hd10562", "interpellation", None),
    ("IP_hd10115", "hd10115", "interpellation", None),
    ("FS_20260611", "hdc120260611fs", "fragestund", 2400),
    ("FS_20260604", "hdc120260604fs", "fragestund", 2400),
    ("PL_20260610", "hdc120260610pd", "partiledardebatt", 2400),
]
LICENCE = "Riksdagens oppna data: free to use and redistribute with source attribution (Kalla: Sveriges riksdag)"

def page(vid):
    u = f"https://www.riksdagen.se/sv/webb-tv/video/x/x_{vid}/"
    h = subprocess.run(["curl", "-sL", "-m", "60", u], capture_output=True, text=True).stdout
    d = json.loads(re.search(r'<script id="__NEXT_DATA__"[^>]*>(.*?)</script>', h, re.S).group(1))
    return u, d["props"]["pageProps"]["contentApiData"]

def sha(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()

def main():
    SCRATCH.mkdir(parents=True, exist_ok=True)
    manifest = []
    for cid, vid, kind, crop in CLIPS:
        u, c = page(vid)
        mp3 = SCRATCH / f"{cid}.mp3"
        if not mp3.exists():
            urllib.request.urlretrieve(c["video"]["audioUrl"], mp3)
        speeches = [s for s in c["speakers"] if s["speechSeconds"] >= 5]
        if crop:
            speeches = [s for s in speeches if s["startPosition"] + s["speechSeconds"] <= crop]
        cut = max(s["startPosition"] + s["speechSeconds"] for s in speeches) + 20 if crop else None
        wav = SCRATCH / f"{cid}.wav"
        if not wav.exists():
            cmd = ["ffmpeg", "-v", "error", "-y", "-i", str(mp3)] + (["-t", str(cut)] if cut else []) + ["-ac", "1", "-ar", "16000", str(wav)]
            subprocess.run(cmd, check=True)
        dur = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(wav)], capture_output=True, text=True).stdout)
        turns = [(s["startPosition"], s["speechSeconds"], s["speaker"]) for s in speeches]
        with open(OUT / f"sv2-{cid}.rttm", "w") as f:
            for st, d, spk in turns:
                f.write(f"SPEAKER {cid} 1 {st:.3f} {d:.3f} <NA> <NA> {spk.replace(' ', '_')} <NA> <NA>\n")
        manifest.append({
            "id": cid, "kind": kind, "language": "sv", "video_id": vid, "page_url": u,
            "date": c["broadcastInformation"]["date"][:10], "audio_url": c["video"]["audioUrl"],
            "licence": LICENCE, "source_mp3_sha256": sha(mp3), "wav_16k_mono_sha256": sha(wav),
            "crop_seconds": cut, "duration_seconds": round(dur, 1), "speeches": len(turns),
            "reference_speakers": len({t[2] for t in turns}),
            "reference_speech_seconds": round(sum(t[1] for t in turns), 1),
            "speakers": sorted({t[2] for t in turns}),
        })
        print(cid, manifest[-1]["duration_seconds"], manifest[-1]["reference_speakers"], len(turns), file=sys.stderr)
    json.dump(manifest, open(OUT / "manifest-sv2.json", "w"), indent=1, ensure_ascii=False)

if __name__ == "__main__":
    main()
