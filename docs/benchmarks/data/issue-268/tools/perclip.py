#!$HOME/.cache/sagascript-bench/bench/.venv/bin/python
"""Per-clip WER over FLEURS sv_se test. engines: host:<model_dir> | mlx8 . Accuracy only (no timing claims)."""
import sys, os, json, csv, time, tempfile
sys.path.insert(0, "$HOME/.cache/sagascript-bench/bench"); sys.path.insert(0, "$HOME/.cache/sagascript-bench/bench/runners")
from norm import norm
import jiwer
B = "$HOME/.cache/sagascript-bench"
engine, out = sys.argv[1], sys.argv[2]
limit = int(sys.argv[3]) if len(sys.argv) > 3 else None
rows, seen = [], set()
for r in csv.reader(open(f"{B}/fleurs/data/sv_se/test.tsv", encoding="utf-8"), delimiter="\t"):
    if r[1] in seen or not os.path.exists(f"{B}/fleurs/sv_test_audio/test/{r[1]}"): continue
    seen.add(r[1]); rows.append((r[1], r[2]))   # raw transcription column
rows = rows[:limit]
print(len(rows), "clips", flush=True)
hyps = {}
if engine.startswith("host:"):
    import run_engine_host as H
    binary = os.environ.get("HOST_BIN", "/Applications/Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host")
    cache = os.environ.get("HOST_CACHE", B + "/issue-268/hostcache")
    os.makedirs(cache, mode=0o700, exist_ok=True)
    c = H.HostClient(binary, cache)
    c.wait(c.send("hello", client={"name": "b", "version": "1", "git_sha": "0"*40}), 10)
    l = c.wait(c.send("load", model_dir=os.path.abspath(engine[5:]), model_id="pianissimo-sv-coreml-bench", compute_units="ane"), 1800)
    assert l["ok"], l
    W = float(l["window_s"]); print("window", W, flush=True)
    with tempfile.TemporaryDirectory() as d:
        for i, (f, ref) in enumerate(rows):
            import soundfile as sf
            s, sr_ = sf.read(f"{B}/fleurs/sv_test_audio/test/{f}", dtype="float32")
            assert sr_ == 16000
            s = s.tolist()
            p = H.write_pcm(d, f"{i}.f32le", s)
            toks_all, plan = [], H.plan_fixed(len(s), W, 6 if W > 15 else 2)
            st = {"tokens": 0}
            res, merged, _ = H.transcribe(c, p, len(s), W, 6 if W > 15 else 2, 1, st)
            hyps[f] = res.text; os.remove(p)
    c.close()
else:
    sys.path.insert(0, f"{B}/models/pianissimo-sv-mlx-8bit")
    import mlx.core as mx, pianissimo_mlx
    from mlx_common import read_wav
    m = pianissimo_mlx.load(f"{B}/models/pianissimo-sv-mlx-8bit"); mx.eval(m.parameters())
    for f, ref in rows:
        hyps[f] = pianissimo_mlx.transcribe(m, read_wav(f"{B}/fleurs/sv_test_audio/test/{f}")).text
per = []; E = N = 0
for f, ref in rows:
    o = jiwer.process_words(norm(ref), norm(hyps[f])); n = len(norm(ref).split())
    e = o.substitutions + o.deletions + o.insertions
    per.append({"clip": f, "words": n, "errors": e, "wer": e / n if n else None, "ref": norm(ref), "hyp": norm(hyps[f])}); E += e; N += n
import statistics
summary = {"engine": engine, "clips": len(per), "words": N, "errors": E, "pooled_wer": E / N, "mean_clip_wer": statistics.mean(p["wer"] for p in per), "median_clip_wer": statistics.median(p["wer"] for p in per)}
print(json.dumps(summary)); json.dump({"summary": summary, "clips": per}, open(out, "w"), ensure_ascii=False)
