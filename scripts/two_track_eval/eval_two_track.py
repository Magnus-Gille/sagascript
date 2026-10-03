#!/usr/bin/env python3
"""Score two-track meeting transcription against a reference RTTM.

For one synthetic two-track file (see build_synthetic.py) run
`sagascript transcribe --diarize --meeting-json` twice: the two-track pipeline
(automatic via the file marker) and the downmix baseline (--no-two-track), then
report on the reference speech (frame-based, 10 ms, 0.25 s collar around every
reference turn boundary, optimal 1:1 speaker mapping, like
scripts/diarization_sv/eval_der.py on the main branch):

  DER, confusion          standard, optimal speaker mapping
  me recall               share of the local user's reference speech labelled as the local user
  false me                share of the others' reference speech wrongly labelled as the local user
  speakers                hypothesis speakers with >= 5 s of scored speech / reference speakers

For the baseline there is no "Me" label: the cluster that best matches the local
user's reference speech is taken as "me" (a generous choice, the baseline cannot
know which speaker is the user).

Needs numpy + scipy (the bench venv). Output is JSON on stdout plus one summary
line per run on stderr.
"""
import argparse, json, re, subprocess, sys
import numpy as np
from scipy.optimize import linear_sum_assignment

FR = 0.01
ME = "Me"

def read_rttm(p):
    t = []
    for l in open(p):
        f = l.split()
        if f and f[0] == "SPEAKER":
            t.append((float(f[3]), float(f[3]) + float(f[4]), f[7]))
    return t

def hyp_from_meeting(m):
    spk = {s["id"]: s["label"] for s in m["speakers"]}
    return [(s["start"], s["end"], spk.get(s["speaker"], s["speaker"])) for s in m["segments"]]

def score(ref, hyp, me_ref, me_hyp=None, collar=0.25):
    T = int(max(max(r[1] for r in ref), max((h[1] for h in hyp), default=0)) / FR) + 2
    rl = sorted({r[2] for r in ref}); hl = sorted({h[2] for h in hyp}) or ["-"]
    R = np.zeros((len(rl), T), bool); H = np.zeros((len(hl), T), bool); keep = np.zeros(T, bool)
    for s, e, n in ref:
        R[rl.index(n), int(s / FR):int(e / FR)] = True; keep[int(s / FR):int(e / FR)] = True
    for s, e, n in hyp:
        H[hl.index(n), int(s / FR):int(e / FR)] = True
    for s, e, _ in ref:
        for b in (s, e):
            keep[max(0, int((b - collar) / FR)):int((b + collar) / FR)] = False
    R, H = R[:, keep], H[:, keep]
    ov = R.astype(np.float32) @ H.T.astype(np.float32)
    ri, hi = linear_sum_assignment(-ov)
    nref, nhyp = R.sum(0), H.sum(0)
    ncorrect = np.zeros(R.shape[1], int)
    for a, b in zip(ri, hi):
        ncorrect += R[a] & H[b]
    total = nref.sum()
    miss = np.maximum(nref - nhyp, 0).sum(); fa = np.maximum(nhyp - nref, 0).sum()
    conf = (np.minimum(nref, nhyp) - ncorrect).sum()
    # "me" attribution
    mi = rl.index(me_ref)
    if me_hyp is None:  # baseline: the best-matching cluster stands in for "me"
        me_hyp = hl[int(np.argmax(ov[mi]))]
    hj = hl.index(me_hyp) if me_hyp in hl else None
    me_frames = R[mi].sum(); other_frames = (R.any(0) & ~R[mi]).sum()
    if hj is None:
        recall, false_me = 0.0, 0.0
    else:
        recall = (R[mi] & H[hj]).sum() / max(me_frames, 1)
        false_me = ((R.any(0) & ~R[mi]) & H[hj]).sum() / max(other_frames, 1)
    return dict(der=float((miss + fa + conf) / total), conf=float(conf / total), miss=float(miss / total),
                fa=float(fa / total), me_recall=float(recall), false_me=float(false_me),
                hyp_speakers=int(sum(1 for j in range(len(hl)) if H[j].sum() * FR >= 5.0)), ref_speakers=len(rl))

def run_cli(cli, wav, extra, env_extra=None, model="kb-whisper-tiny", want_stderr=False):
    import os
    env = dict(os.environ); env.update(env_extra or {})
    p = subprocess.run([cli, "transcribe", wav, "--language", "sv", "--model", model, "--diarize",
                        "--meeting-json"] + extra, check=True, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE if want_stderr else subprocess.DEVNULL, env=env)
    doc = json.loads(p.stdout)
    return (doc, p.stderr.decode("utf-8", "replace")) if want_stderr else doc

def words(text):
    return re.findall(r"\w+", text.lower())

def lcs(a, b):
    prev = [0] * (len(b) + 1)
    for x in a:
        cur = [0]
        for j, y in enumerate(b):
            cur.append(prev[j] + 1 if x == y else max(prev[j + 1], cur[j]))
        prev = cur
    return prev[-1]

def wer(ref, hyp):
    prev = list(range(len(hyp) + 1))
    for i, x in enumerate(ref, 1):
        cur = [i]
        for j, y in enumerate(hyp, 1):
            cur.append(min(prev[j] + 1, cur[j - 1] + 1, prev[j - 1] + (x != y)))
        prev = cur
    return prev[-1] / max(len(ref), 1)

def cover(ref_turns, intervals):
    """Share of the reference turns' seconds inside `intervals`."""
    tot = sum(e - s for s, e in ref_turns); got = 0.0
    for s, e in ref_turns:
        for a, b in intervals:
            got += max(0.0, min(e, b) - max(s, a))
    return got / max(tot, 1e-9)

def parse_debug(stderr, tag):
    out = []
    for l in stderr.splitlines():
        f = l.split("\t")
        if f[0] == tag and len(f) >= 3:
            out.append((float(f[1]), float(f[2]), f[3] if len(f) > 3 else ""))
    return out

def word_report(meeting, stderr, ref_text, me_turns):
    me_text = " ".join(s["text"] for s in meeting["segments"] if s["speaker"] == ME)
    ref_w, hyp_w = words(ref_text), words(me_text)
    vad = [(s, e) for s, e, _ in parse_debug(stderr, "MICVAD")]
    act = [(s, e) for s, e, _ in parse_debug(stderr, "MICACT")]
    raw = parse_debug(stderr, "MICRAW")
    return dict(ref_words=len(ref_w), me_words=len(hyp_w), raw_words=sum(len(words(t)) for _, _, t in raw),
                word_recall=lcs(ref_w, hyp_w) / max(len(ref_w), 1), wer=wer(ref_w, hyp_w),
                vad_coverage=cover(me_turns, vad), active_coverage_after_guard=cover(me_turns, act))

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cli", required=True); ap.add_argument("--wav", required=True)
    ap.add_argument("--ref", required=True, help="RTTM of the (cropped) reference, see build_synthetic --ref-out")
    ap.add_argument("--me", required=True)
    ap.add_argument("--model", default="kb-whisper-tiny")
    ap.add_argument("--me-ref-text", help="reference transcript of the local user's turns (plain transcription of the me-only WAV)")
    ap.add_argument("--label", default=None); ap.add_argument("--no-baseline", action="store_true")
    ap.add_argument("--guard-off", action="store_true", help="two-track run with the crosstalk guard disabled")
    a = ap.parse_args()
    ref = read_rttm(a.ref); out = {"label": a.label or a.wav}
    env = {"SAGASCRIPT_TWO_TRACK_CROSSTALK_GUARD": "off"} if a.guard_off else None
    env = dict(env or {}); env["SAGA_DIAR_DEBUG"] = "1"
    m, err = run_cli(a.cli, a.wav, [], env, a.model, want_stderr=True)
    assert m.get("local_speaker") == ME, "two-track pipeline did not engage (no local_speaker)"
    out["two_track"] = score(ref, hyp_from_meeting(m), a.me, ME)
    if a.me_ref_text:
        out["me_words"] = word_report(m, err, open(a.me_ref_text).read(), [(s, e) for s, e, n in ref if n == a.me])
    if not a.no_baseline:
        b = run_cli(a.cli, a.wav, ["--no-two-track"], None, a.model)
        assert "local_speaker" not in b
        out["downmix"] = score(ref, hyp_from_meeting(b), a.me, None)
    for k in ("two_track", "downmix"):
        if k in out:
            r = out[k]
            print(f"{out['label']:34s} {k:9s} DER={r['der']*100:5.1f}% conf={r['conf']*100:5.1f}% miss={r['miss']*100:4.1f}% fa={r['fa']*100:4.1f}% "
                  f"me_recall={r['me_recall']*100:5.1f}% false_me={r['false_me']*100:4.1f}% speakers {r['hyp_speakers']}/{r['ref_speakers']}", file=sys.stderr)
    if "me_words" in out:
        w = out["me_words"]
        print(f"{'':34s} me words: ref {w['ref_words']} raw {w['raw_words']} kept {w['me_words']} recall={w['word_recall']*100:.0f}% WER={w['wer']*100:.0f}% "
              f"VAD covers {w['vad_coverage']*100:.0f}% of me speech, after guard {w['active_coverage_after_guard']*100:.0f}%", file=sys.stderr)
    json.dump(out, sys.stdout, indent=1)

if __name__ == "__main__":
    main()
