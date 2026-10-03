#!/usr/bin/env python3
"""DER / speaker-count / purity-coverage harness for the Riksdag eval set (issue #284).

Run with ~/.cache/sagascript-bench/bench/.venv/bin/python (numpy+scipy only).

  eval_der.py --bin <diarize_eval> --scratch DIR [--thresholds 0.5,0.75] [--set riksdag|other|sv2|degraded] [--ids SoU40,...]
      analysis-only path (no Whisper): <bin> analyze (cached in DIR/analysis/<id>.<tag>.json),
      then <bin> cluster per threshold.
  eval_der.py --cli <sagascript> --scratch DIR --ids SoU38 [--thresholds 0.34]
      end-to-end: `sagascript transcribe --diarize --meeting-json --diarize-cache`.

Metric: frame-based (10 ms) DER with a 0.25 s collar around every reference turn boundary,
scored only inside reference speeches (gaps/chair excluded). Optimal 1:1 speaker mapping
(Hungarian on overlap). DER = (miss + false alarm + confusion) / reference speech time. Confusion (wrong speaker
identity, the part clustering controls) is reported first; missed speech comes from segmentation.
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
    inref = np.zeros(T, bool)
    for s, e, _ in ref:
        inref[int(s / FR):int(e / FR)] = True
    Hfull = H
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
    # Hypothesis clusters with no reference partner: how much of their speech lies outside every
    # reference speech (chair announcements, pauses)? ~1.0 means a genuine non-reference voice such as the chair.
    matched = set(int(b) for a, b in zip(ri, hi) if ov[a, b] > 0)  # a zero-overlap assignment is not a match
    extra = []
    for j in range(len(hl)):
        tot = float(Hfull[j].sum() * FR)
        if j not in matched and tot >= 5.0:
            extra.append(dict(label=hl[j], seconds=round(tot, 1), outside_ref=round(float((Hfull[j] & ~inref).sum() * FR) / tot, 2)))
    return dict(der=float((miss + fa + conf) / total), extra_clusters=extra, miss=float(miss / total), fa=float(fa / total),
                conf=float(conf / total), ref_speakers=len(rl),
                hyp_speakers=sum(1 for j in range(len(hl)) if H[j].sum() * FR >= 5.0),  # >=5 s of scored speech
                hyp_speakers_all=len(pur), coverage=cov, purity=pur)

def run(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)

SETS = {  # name -> (manifest, rttm file for an id, wav sub-directory of --scratch)
    "riksdag": ("manifest.json", lambda i: f"{i}.rttm", ""),
    "other": ("manifest-other.json", lambda i: f"other-{i}.rttm", "other"),
    "sv2": ("manifest-sv2.json", lambda i: f"sv2-{i}.rttm", "sv2"),
    "degraded": ("manifest-degraded.json", lambda i: None, "sv2/deg"),  # telephone-band copies of Swedish clips
}

def hint_arg(spec, truth):
    """Translate a --hint spec into the diarize_eval hint argument for a recording with `truth` speakers."""
    if spec is None:
        return []
    if spec.startswith("truth"):
        return [str(max(1, truth + int(spec[5:] or 0)))]
    kind, lo, hi = spec.split(":")
    f = lambda v: "" if v == "" else str(max(1, truth + int(v)))
    return [f"{f(lo)}-{f(hi)}"]

def default_threshold():
    """The shipped default, read from core so the harness cannot drift from it."""
    import re
    src = (Path(__file__).resolve().parents[2] / "src-tauri/crates/sagascript-core/src/diarization/mod.rs").read_text()
    return re.search(r"pub const DEFAULT_THRESHOLD: f32 = ([0-9.]+);", src).group(1)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin"); ap.add_argument("--cli"); ap.add_argument("--scratch", required=True)
    ap.add_argument("--tag", default="main", help="analysis cache tag; use a new tag per build of the analysis stage")
    ap.add_argument("--set", default="riksdag", choices=list(SETS)); ap.add_argument("--ids")
    ap.add_argument("--min-speaker", default=None, help="MIN_SPEAKER_SECONDS override (--bin only)")
    ap.add_argument("--absorb-max-distance", default=None, help="absorb distance limit override (--bin only)")
    ap.add_argument("--hint", default=None,
                    help="speaker-count hint (--bin only; issue #305), relative to the manifest's reference_speakers T: "
                         "'truth' = exactly T, 'truth+1' = exactly T+1 (counts a Riksdag chair), "
                         "'range:A:B' = min T+A and max T+B (e.g. range:0:1; either side may be empty)")
    ap.add_argument("--thresholds", default=None, help="comma list; default = the shipped DEFAULT_THRESHOLD")
    ap.add_argument("--json-out")
    a = ap.parse_args()
    scratch = Path(a.scratch); mfile, rttm_of, sub = SETS[a.set]
    manifest = json.load(open(DATA / mfile))
    wavdir = scratch / sub if sub else scratch
    ids = a.ids.split(",") if a.ids else [m["id"] for m in manifest]
    thresholds = [float(x) for x in (a.thresholds or default_threshold()).split(",")]
    res = {}
    for m in manifest:
        if m["id"] not in ids: continue
        i = m["id"]
        refname = rttm_of(i)
        if refname is None:  # degraded copies reuse the reference of their source clip
            base = m["source_id"]; bset = m["source_set"]
            refname = SETS[bset][1](base)
        ref = read_rttm(DATA / refname); res[i] = {"truth": m["reference_speakers"]}
        for th in thresholds:
            if a.bin:
                an = scratch / "analysis" / f"{i}.{a.tag}.json"
                an.parent.mkdir(exist_ok=True)
                if not an.exists(): run([a.bin, "analyze", str(wavdir / f"{i}.wav"), str(an)])
                hint = hint_arg(a.hint, m["reference_speakers"])
                htag = f".hint-{hint[0]}" if hint else ""
                out = scratch / "analysis" / f"{i}.{a.tag}.{th}.{a.min_speaker}.{a.absorb_max_distance}{htag}.seg.json"
                extra = ([a.min_speaker or "8"] + ([a.absorb_max_distance] if a.absorb_max_distance else [])) if (a.min_speaker or a.absorb_max_distance) else []
                if hint:  # the hint is the 7th positional argument; fill the two before it with the defaults
                    extra = (extra + ["8", "0.75"][len(extra):]) + hint
                run([a.bin, "cluster", str(an), str(th), str(out)] + extra, stderr=subprocess.DEVNULL)
                hyp = hyp_from_segments(json.load(open(out)))
            else:
                cache = scratch / "runs" / a.tag / f"{i}.cache.json"; cache.parent.mkdir(parents=True, exist_ok=True)
                p = run([a.cli, "transcribe", str(wavdir / f"{i}.wav"), "--language", "sv", "--model", "kb-whisper-tiny",
                         "--diarize", "--meeting-json", "--diarize-threshold", str(th), "--diarize-cache", str(cache)],
                        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
                hyp = hyp_from_meeting(json.loads(p.stdout))
            r = der(ref, hyp); res[i][str(th)] = r
            ex = " extra " + ",".join(f"{e['seconds']}s/{int(e['outside_ref']*100)}%out" for e in r["extra_clusters"]) if r["extra_clusters"] else ""
            print(f"{i:14s} th={th:<5} CONF={r['conf']*100:5.1f}% DER={r['der']*100:5.1f}% (miss {r['miss']*100:.1f} fa {r['fa']*100:.1f}) speakers {r['hyp_speakers']}/{r['ref_speakers']} (all {r['hyp_speakers_all']}){ex}")
    if a.json_out: json.dump(res, open(a.json_out, "w"), indent=1)

if __name__ == "__main__":
    main()
