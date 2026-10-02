#!/usr/bin/env python3
"""DER / speaker-count / purity-coverage harness for the Riksdag eval set (issue #284).

Run with ~/.cache/sagascript-bench/bench/.venv/bin/python (numpy+scipy only).

  eval_der.py --bin <diarize_eval> --scratch DIR --thresholds 0.5,0.75,1.0 [--ids SoU40,...]
      analysis-only path (no Whisper): <bin> analyze (cached in DIR/analysis/<id>.<tag>.json),
      then <bin> cluster per threshold.
  eval_der.py --cli <sagascript> --scratch DIR --ids SoU38 [--threshold 0.75]
      end-to-end: `sagascript transcribe --diarize --meeting-json --diarize-cache`.

Metric: frame-based (10 ms) DER with a 0.25 s collar around every reference turn boundary,
scored only inside reference speeches (gaps/chair excluded). Optimal 1:1 speaker mapping
(Hungarian on overlap). DER = (miss + false alarm + confusion) / reference speech time.
"""
import argparse, json, subprocess, sys
from pathlib import Path
import numpy as np
from scipy.optimize import linear_sum_assignment

DATA = Path(__file__).resolve().parents[2] / "docs/benchmarks/data/diarization-sv"
FR = 0.01

def read_rttm(p):
    t = []
    for l in open(p):
        f = l.split()
        t.append((float(f[3]), float(f[3]) + float(f[4]), f[7]))
    return t

def hyp_from_segments(segs):
    return [(s["start"], s["end"], s["speaker"]) for s in segs]

def hyp_from_meeting(m):
    spk = {s.get("id"): s.get("label", s.get("id")) for s in m.get("speakers", [])}
    out = []
    for s in m["segments"]:
        sid = s.get("speaker_id", s.get("speaker"))
        out.append((s["start"], s["end"], spk.get(sid, sid)))
    return out

def der(ref, hyp, collar=0.25):
    T = int(max(max(r[1] for r in ref), max((h[1] for h in hyp), default=0)) / FR) + 2
    rl = sorted({r[2] for r in ref}); hl = sorted({h[2] for h in hyp})
    R = np.zeros((len(rl), T), bool); H = np.zeros((max(len(hl), 1), T), bool)
    keep = np.zeros(T, bool)
    for s, e, n in ref:
        R[rl.index(n), int(s / FR):int(e / FR)] = True; keep[int(s / FR):int(e / FR)] = True
    for s, e, n in hyp:
        H[hl.index(n), int(s / FR):int(e / FR)] = True
    for s, e, _ in ref:  # collars drop scoring around boundaries
        for b in (s, e):
            keep[max(0, int((b - collar) / FR)):int((b + collar) / FR)] = False
    R = R[:, keep]; H = H[:, keep]
    ov = R.astype(np.float32) @ H.T.astype(np.float32)
    ri, hi = linear_sum_assignment(-ov)
    nref = R.sum(0); nhyp = H.sum(0)
    ncorrect = np.zeros(R.shape[1], int)
    for a, b in zip(ri, hi):
        ncorrect += (R[a] & H[b])
    miss = np.maximum(nref - nhyp, 0).sum(); fa = np.maximum(nhyp - nref, 0).sum()
    conf = (np.minimum(nref, nhyp) - ncorrect).sum(); total = nref.sum()
    # per ref speaker coverage (best hyp cluster share), per hyp cluster purity
    cov = {rl[i]: float(ov[i].max() / max(R[i].sum(), 1)) for i in range(len(rl))}
    pur = {hl[j]: float(ov[:, j].max() / max(H[j].sum(), 1)) for j in range(len(hl)) if H[j].sum() > 0}
    return dict(der=float((miss + fa + conf) / total), miss=float(miss / total), fa=float(fa / total),
                conf=float(conf / total), ref_speakers=len(rl),
                hyp_speakers=sum(1 for j in range(len(hl)) if H[j].sum() * FR >= 5.0),  # >=5 s of scored speech
                hyp_speakers_all=len(pur), coverage=cov, purity=pur)

def run(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin"); ap.add_argument("--cli"); ap.add_argument("--scratch", required=True)
    ap.add_argument("--tag", default="main"); ap.add_argument("--ids")
    ap.add_argument("--thresholds", default="0.75"); ap.add_argument("--json-out")
    a = ap.parse_args()
    scratch = Path(a.scratch); manifest = json.load(open(DATA / "manifest.json"))
    ids = a.ids.split(",") if a.ids else [m["id"] for m in manifest]
    res = {}
    for m in manifest:
        if m["id"] not in ids: continue
        i = m["id"]; ref = read_rttm(DATA / f"{i}.rttm"); res[i] = {"truth": m["reference_speakers"]}
        for th in [float(x) for x in a.thresholds.split(",")]:
            if a.bin:
                an = scratch / "analysis" / f"{i}.{a.tag}.json"; an.parent.mkdir(exist_ok=True)
                if not an.exists(): run([a.bin, "analyze", str(scratch / f"{i}.wav"), str(an)])
                out = scratch / "analysis" / f"{i}.{a.tag}.{th}.seg.json"
                run([a.bin, "cluster", str(an), str(th), str(out)], stderr=subprocess.DEVNULL)
                hyp = hyp_from_segments(json.load(open(out)))
            else:
                cache = scratch / "runs" / a.tag / f"{i}.cache.json"; cache.parent.mkdir(parents=True, exist_ok=True)
                p = run([a.cli, "transcribe", str(scratch / f"{i}.wav"), "--language", "sv", "--model", "kb-whisper-tiny",
                         "--diarize", "--meeting-json", "--diarize-threshold", str(th), "--diarize-cache", str(cache)],
                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                hyp = hyp_from_meeting(json.loads(p.stdout))
            r = der(ref, hyp); res[i][str(th)] = r
            print(f"{i:6s} th={th:<5} DER={r['der']*100:5.1f}% (miss {r['miss']*100:.1f} fa {r['fa']*100:.1f} conf {r['conf']*100:.1f}) speakers {r['hyp_speakers']}/{r['ref_speakers']} (all {r['hyp_speakers_all']})")
    if a.json_out: json.dump(res, open(a.json_out, "w"), indent=1)

if __name__ == "__main__":
    main()
