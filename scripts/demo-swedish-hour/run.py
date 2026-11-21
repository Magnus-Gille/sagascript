#!/usr/bin/env python3
"""Transcribe a file with Sagascript, time it and score it against a reference.

Usage: run.py AUDIO REFERENCE.txt [--model pianissimo-sv] [--language sv]
Prints JSON: seconds (wall time of the whole command), audio_seconds, wer_percent...
"""
import argparse, json, os, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wer import wer

ap = argparse.ArgumentParser()
ap.add_argument("audio"); ap.add_argument("reference")
ap.add_argument("--model", default="pianissimo-sv"); ap.add_argument("--language", default="sv")
ap.add_argument("--bin", default="sagascript")
a = ap.parse_args()
t0 = time.monotonic()
p = subprocess.run([a.bin, "transcribe", "--model", a.model, "--language", a.language, "--json", a.audio],
                   capture_output=True, text=True)
secs = time.monotonic() - t0
if p.returncode:
    sys.exit(p.stderr)
res = json.loads(p.stdout)
if isinstance(res, list):
    res = res[0]
text = res.get("text") or res.get("result", {}).get("text", "")
hyp = os.path.splitext(a.audio)[0] + ".hypothesis.txt"
open(hyp, "w", encoding="utf-8").write(text)
out = wer(open(a.reference, encoding="utf-8").read(), text)
out.update(seconds=round(secs, 1), audio_seconds=res.get("duration"), model=a.model, hypothesis=hyp)
print(json.dumps(out))
