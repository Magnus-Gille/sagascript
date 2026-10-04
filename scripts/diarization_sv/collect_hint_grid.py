#!/usr/bin/env python3
"""Collect eval_der.py --hint runs (issue #305) into one committed JSON, then print the doc tables.

  collect_hint_grid.py collect OUT.json --code-commit SHA DIR      # DIR holds <set>.<variant>.json files
  collect_hint_grid.py tables GRID.json                            # markdown for docs/benchmarks/diarization-sv.md

Variant names: none; force_T / force_T1 (forced exact T / T+1); T_d<limit>, T1_d<limit> (exact T / T+1),
minT_d<limit> (min T), maxT1_d<limit> (max T+1), each with the merge-distance limit `d`.
T is the manifest's reference_speakers (chair not counted). Each cell: [confusion %, DER %, found, truth].
"""
import json, sys, argparse, re
from pathlib import Path

SETS = ["riksdag", "sv2", "degraded", "other"]
LABEL = {"riksdag": "Riksdag debates", "sv2": "Other Riksdag", "degraded": "Swedish, band-limited", "other": "French/Norwegian calls"}

def collect(out, code_commit, d):
    res, variants = {}, set()
    for f in sorted(Path(d).glob("*.json")):
        s, v = f.stem.split(".", 1)
        if s not in SETS: continue
        variants.add(v)
        data = json.load(open(f))
        res.setdefault(v, {})[s] = {i: [round(r["0.34"]["conf"] * 100, 2), round(r["0.34"]["der"] * 100, 2),
                                        r["0.34"]["hyp_speakers"], r["truth"]] for i, r in data.items()}
    meta = {"issue": 305, "threshold": 0.34, "min_speaker_seconds": 8, "absorb_max_distance": 0.75,
            "code_commit": code_commit, "analyses": "cached 'main' analyses of the committed manifests (analysis stage unchanged)",
            "tool": "diarize_eval cluster ... [hint] [hint_merge_max_distance] via eval_der.py --hint",
            "cell": "[confusion %, DER %, speakers found, truth]"}
    json.dump({"meta": meta, "results": res}, open(out, "w"), indent=1, sort_keys=True)

def agg(g, v, s):
    rows = list(g["results"][v][s].values())
    n = len(rows)
    return (sum(r[2] == r[3] for r in rows), n, sum(r[0] for r in rows) / n, sum(r[1] for r in rows) / n)

def tables(path, limits=("0.6", "0.75", "0.9", "1.0")):
    g = json.load(open(path))
    print("| Variant | " + " | ".join(f"{LABEL[s]} ({len(g['results']['none'][s])})" for s in SETS) + " |")
    print("| --- | " + " | ".join("---" for _ in SETS) + " |")
    rows = [("threshold only", "none"), ("forced exact T", "force_T"), ("forced exact T+1", "force_T1")]
    for kind, name in (("T", "exact T"), ("T1", "exact T+1"), ("minT", "min T"), ("maxT1", "max T+1")):
        for lim in limits:
            v = f"{kind}_d{lim}"
            if v in g["results"]: rows.append((f"{name}, limit {lim}", v))
    for label, v in rows:
        cells = []
        for s in SETS:
            ex, n, conf, der = agg(g, v, s)
            cells.append(f"{ex}/{n}, {conf:.1f} % / {der:.1f} %")
        print(f"| {label} | " + " | ".join(cells) + " |")
    print("\nPer recording, chosen variant vs threshold only vs forced T (found/truth, confusion):")
    print("| Recording | Threshold only | Exact T, limit 0.9 | Forced exact T |\n| --- | --- | --- | --- |")
    for s in SETS:
        for i in g["results"]["none"][s]:
            c = lambda v: (lambda r: f"{r[2]}/{r[3]}, {r[0]:.1f} %")(g["results"][v][s][i])
            a, b, f = c("none"), c("T_d0.9"), c("force_T")
            if len({a, b, f}) > 1: print(f"| {i} | {a} | {b} | {f} |")

if __name__ == "__main__":
    ap = argparse.ArgumentParser(); sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("collect"); c.add_argument("out"); c.add_argument("--code-commit", required=True); c.add_argument("dir")
    t = sub.add_parser("tables"); t.add_argument("grid")
    a = ap.parse_args()
    collect(a.out, a.code_commit, a.dir) if a.cmd == "collect" else tables(a.grid)
