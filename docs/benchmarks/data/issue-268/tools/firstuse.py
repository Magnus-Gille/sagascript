#!$HOME/.cache/sagascript-bench/bench/.venv/bin/python
import sys, os, time, json, subprocess, threading
sys.path.insert(0, "$HOME/.cache/sagascript-bench/bench/runners")
import run_engine_host as H
model, cache = sys.argv[1], sys.argv[2]
os.makedirs(cache, mode=0o700, exist_ok=True)
binp = "/Applications/Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host"
def lv():
    o = subprocess.run(["notifyutil","-g","com.apple.system.thermalpressurelevel"],capture_output=True,text=True).stdout
    return int(o.split()[-1])
mx=[0]; stop=threading.Event(); rss=[0]
c = H.HostClient(binp, cache)
def samp():
    while not stop.is_set():
        mx[0]=max(mx[0],lv())
        r=subprocess.run(["ps","-o","rss=","-p",str(c.process.pid)],capture_output=True,text=True).stdout.strip()
        if r: rss[0]=max(rss[0],int(r))
        stop.wait(2)
t=threading.Thread(target=samp,daemon=True); t.start()
c.wait(c.send("hello", client={"name":"b","version":"1","git_sha":"0"*40}),10)
t0=time.time(); l=c.wait(c.send("load", model_dir=model, model_id="pianissimo-sv-coreml-bench", compute_units="ane"),3600); load_s=time.time()-t0
print(json.dumps({"model":model,"ok":l["ok"],"window_s":l.get("window_s"),"first_load_s":round(load_s,1),"thermal_max":mx[0],"host_rss_mb":round(rss[0]/1024),"loadavg":os.getloadavg()[0]}), flush=True)
stop.set(); c.close()
