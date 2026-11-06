#!$HOME/.cache/sagascript-bench/bench/.venv/bin/python
"""Peak host RSS (ps rss of the engine-host pid, sampled 0.5 s) during a 60 min file run via run_engine_host.main."""
import sys, os, json, subprocess, threading
sys.path.insert(0, "$HOME/.cache/sagascript-bench/bench/runners")
import run_engine_host as H
label = sys.argv[1]; sys.argv = ["x", "--input", "$HOME/.cache/sagascript-bench/longform/fleurs-sv-distinct-60min.wav", "--output", f"/tmp/rss60-{label}.json", "--variant", "x-ane", "--warmup", "$HOME/.cache/sagascript-bench/longform/fleurs-sv-1min.wav"]
orig = H.HostClient.__init__; pids=[]; peak=[0]; stop=threading.Event()
def init(self,*a,**k): orig(self,*a,**k); pids.append(self.process.pid)
H.HostClient.__init__ = init
def samp():
    while not stop.is_set():
        for p in pids:
            r=subprocess.run(["ps","-o","rss=","-p",str(p)],capture_output=True,text=True).stdout.strip()
            if r: peak[0]=max(peak[0],int(r))
        stop.wait(0.5)
threading.Thread(target=samp,daemon=True).start()
H.main(); stop.set()
print(json.dumps({"variant":label,"host_peak_rss_mb":round(peak[0]/1024)}))
