#!$HOME/.cache/sagascript-bench/bench/.venv/bin/python
"""Warm-host short-utterance latency: one host, load once, time transcribe_window per clip (5 reps)."""
import sys, os, time, json, tempfile, statistics, subprocess
sys.path.insert(0, "$HOME/.cache/sagascript-bench/bench/runners")
import run_engine_host as H
B="$HOME/.cache/sagascript-bench"
model, cache, label = sys.argv[1:4]
man = json.load(open(f"{B}/longform/clips-manifest.json"))["files"]
clips = [(k, v) for k, v in sorted(man.items()) if 2 <= v["duration_s"] <= 8.5]
binp = "/Applications/Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host"
c = H.HostClient(binp, cache)
c.wait(c.send("hello", client={"name":"b","version":"1","git_sha":"0"*40}),10)
t0=time.time(); l=c.wait(c.send("load", model_dir=model, model_id="pianissimo-sv-coreml-bench", compute_units="ane"),1800); load_s=time.time()-t0
W=float(l["window_s"]); lat=[]; per={}
with tempfile.TemporaryDirectory() as d:
    for k, v in clips:
        s = H.read_wav(f"{B}/longform/{k}.wav") if False else None
        import soundfile as sf
        a, sr = sf.read(f"{B}/longform/{k}.wav", dtype="float32"); assert sr==16000
        p = H.write_pcm(d, k+".f32le", a.tolist())
        ts=[]
        for r in range(6):
            t=time.perf_counter(); rid=c.send("transcribe_window", pcm_path=p, offset_samples=0, num_samples=len(a), sample_rate=16000, format="f32le", priority="interactive")
            resp=c.wait(rid, 120); ts.append(time.perf_counter()-t); assert resp["ok"], resp
        ts=ts[1:]  # drop first (warm-up) per clip
        per[k]={"dur_s":v["duration_s"],"median_ms":round(1000*statistics.median(ts),1),"enc_ms":resp.get("timings",{}).get("encode_ms")}
        lat+=ts
lat.sort()
out={"variant":label,"window_s":W,"clips":len(clips),"load_s":round(load_s,1),"median_ms":round(1000*statistics.median(lat),1),"p90_ms":round(1000*lat[int(0.9*(len(lat)-1))],1),"loadavg":os.getloadavg()[0],"per_clip":per}
print(json.dumps(out))
c.close()
