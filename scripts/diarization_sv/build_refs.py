#!/usr/bin/env python3
"""Build the Swedish Riksdag diarization eval set (issue #284).

Reads the riksdagen.se webb-tv page JSON of each debate (speech start/length and
speaker come from the Riksdag open data), writes reference RTTM files and a
manifest (URLs, SHA-256, durations, speaker counts) into docs/benchmarks/data/diarization-sv/.
Audio is downloaded to the scratch dir and never committed.

Reference policy: one RTTM turn per speech, labelled by the speaker name. Gaps between
speeches (chair/Talman announcements, votes, pauses) are NOT labelled and are excluded
from scoring (scored region = union of speech intervals, `uem` in the manifest).
Speeches shorter than 5 s (chair announcements) are dropped. Crops end after a complete speech.
"""
import hashlib, json, re, subprocess, sys, unicodedata, urllib.request
from pathlib import Path

SCRATCH = Path.home() / ".cache/sagascript-bench/diar-284"
OUT = Path(__file__).resolve().parents[2] / "docs/benchmarks/data/diarization-sv"
# (id, committee report page slug title, crop seconds or None)
DEBATES = [
    ("SoU40", "Skyldighet att betala för tandvård - nya regler för vissa utlänningar", None),
    ("JuU41", "Skärpta regler för unga lagöverträdare", 3000),
    ("SoU39", "Förebyggande insatser inom socialtjänsten till skydd för barn och unga vid bristande medverkan", 3600),
    ("SoU38", "För barns rättigheter och trygghet - en ny lag om omhändertagande för vård av barn och unga", 3600),
    ("UU24", "Sveriges utrikes underrättelsetjänst", 2700),
]

def slug(t):
    t = t.lower().replace("å", "a").replace("ä", "a").replace("ö", "o").replace("é", "e")
    return re.sub(r"[^a-z0-9]+", "-", t).strip("-")

def page(did, title):
    u = f"https://www.riksdagen.se/sv/webb-tv/video/debatt-om-forslag/{slug(title)}_hd01{did.lower()}/"
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
    SCRATCH.mkdir(parents=True, exist_ok=True); OUT.mkdir(parents=True, exist_ok=True)
    manifest = []
    for did, title, crop in DEBATES:
        u, c = page(did, title)
        mp3 = SCRATCH / f"{did}.mp3"
        if not mp3.exists():
            urllib.request.urlretrieve(c["video"]["audioUrl"], mp3)
        speeches = [s for s in c["speakers"] if s["speechSeconds"] >= 5]
        if crop:
            speeches = [s for s in speeches if s["startPosition"] + s["speechSeconds"] <= crop]
        cut = None
        if crop:
            cut = max(s["startPosition"] + s["speechSeconds"] for s in speeches) + 20
        wav = SCRATCH / f"{did}.wav"
        if not wav.exists():
            cmd = ["ffmpeg", "-v", "error", "-y", "-i", str(mp3)] + (["-t", str(cut)] if cut else []) + ["-ac", "1", "-ar", "16000", str(wav)]
            subprocess.run(cmd, check=True)
        dur = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(wav)], capture_output=True, text=True).stdout)
        turns = [(s["startPosition"], s["speechSeconds"], s["speaker"]) for s in speeches]
        with open(OUT / f"{did}.rttm", "w") as f:
            for st, d, spk in turns:
                f.write(f"SPEAKER {did} 1 {st:.3f} {d:.3f} <NA> <NA> {spk.replace(' ', '_')} <NA> <NA>\n")
        manifest.append({
            "id": did, "title": title, "date": c["broadcastInformation"]["date"][:10], "page_url": u,
            "audio_url": c["video"]["audioUrl"], "source_mp3_sha256": sha(mp3),
            "wav_16k_mono_sha256": sha(wav), "crop_seconds": cut, "duration_seconds": round(dur, 1),
            "speeches": len(turns), "reference_speakers": len({t[2] for t in turns}),
            "reference_speech_seconds": round(sum(t[1] for t in turns), 1),
            "speakers": sorted({t[2] for t in turns}),
        })
        print(did, manifest[-1]["duration_seconds"], manifest[-1]["reference_speakers"], file=sys.stderr)
    json.dump(manifest, open(OUT / "manifest.json", "w"), indent=1, ensure_ascii=False)

if __name__ == "__main__":
    main()
