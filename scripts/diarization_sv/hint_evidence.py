#!/usr/bin/env python3
"""Which default clusters does a forced exact-T hint merge? (issue #305 evidence)

  hint_evidence.py SCRATCH SET REC [REC ...]   # compares <rec>.main.0.34.None.None.seg.json with the forced-T run

For every default cluster with at least 10 s of speech: seconds, seconds inside each reference speaker's
speech, share outside all reference speech (chair-like when ~1.0), and the forced cluster it ended up in.
"""
import json, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from eval_der import DATA, SETS, read_rttm

def seg_label(a, b, i):
    return i  # raw segments are identical in both runs: compare by index

def main():
    scratch, sset, recs = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    manifest = {m["id"]: m for m in json.load(open(DATA / SETS[sset][0]))}
    out = {}
    for rec in recs:
        m = manifest[rec]; truth = m["reference_speakers"]
        ref = read_rttm(DATA / SETS[sset][1](rec))
        base = json.load(open(scratch / "analysis" / f"{rec}.main.0.34.None.None.seg.json"))
        forced = json.load(open(scratch / "analysis" / f"{rec}.main.0.34.None.None.hint-{truth}!.seg.json"))
        clusters = {}
        for s, f in zip(base, forced):
            c = clusters.setdefault(s["speaker"], dict(sec=0.0, inref={}, outside=0.0, forced=set()))
            dur = s["end"] - s["start"]; c["sec"] += dur; c["forced"].add(f["speaker"])
            covered = 0.0
            for rs, re_, who in ref:
                o = max(0.0, min(s["end"], re_) - max(s["start"], rs))
                if o > 0: c["inref"][who] = c["inref"].get(who, 0.0) + o; covered += o
            c["outside"] += max(0.0, dur - covered)
        out[rec] = {k: dict(seconds=round(v["sec"], 1), outside_ref=round(v["outside"] / v["sec"], 2),
                            ref_speakers={w: round(x, 1) for w, x in sorted(v["inref"].items(), key=lambda t: -t[1])[:2]},
                            forced_cluster=sorted(v["forced"])) for k, v in clusters.items() if v["sec"] >= 10}
    json.dump(out, sys.stdout, indent=1, ensure_ascii=False)

if __name__ == "__main__":
    main()
