"""Bound native probes and preserve exit codes/peak sampled process-family RSS."""
import argparse
import json
from pathlib import Path
import subprocess
import time
import psutil

p = argparse.ArgumentParser()
p.add_argument('--lab', required=True, type=Path)
p.add_argument('--label', required=True)
p.add_argument('--timeout', type=float, default=420)
p.add_argument('--max-rss-mb', type=float, default=0)
p.add_argument('--min-available-mb', type=float, default=0)
p.add_argument('command', nargs=argparse.REMAINDER)
a = p.parse_args()
command = a.command[1:] if a.command[:1] == ['--'] else a.command
result = {'command': command, 'timeout_s': a.timeout, 'status': 'running', 'peak_sampled_rss_mb': 0}
target = a.lab / (a.label + '-process.json')
def save():
    target.write_text(json.dumps(result, indent=2), encoding='utf-8')
started = time.perf_counter()
with (a.lab / (a.label + '.stdout.txt')).open('w', encoding='utf-8') as out, (a.lab / (a.label + '.stderr.txt')).open('w', encoding='utf-8') as err:
    proc = subprocess.Popen(command, stdout=out, stderr=err, creationflags=subprocess.CREATE_NO_WINDOW)
    save()
    root = psutil.Process(proc.pid)
    while proc.poll() is None:
        elapsed = time.perf_counter() - started
        try:
            rss = sum(x.memory_info().rss for x in [root, *root.children(recursive=True)]) / 1e6
            result['peak_sampled_rss_mb'] = max(rss, result['peak_sampled_rss_mb'])
        except psutil.Error:
            pass
        result['elapsed_s'] = elapsed
        save()
        reason = None
        if elapsed > a.timeout:
            reason = 'timeout'
        elif a.max_rss_mb and result['peak_sampled_rss_mb'] > a.max_rss_mb:
            reason = 'process_memory_limit'
        elif a.min_available_mb and psutil.virtual_memory().available / 1e6 < a.min_available_mb:
            reason = 'system_memory_reserve'
        if reason:
            for child in root.children(recursive=True):
                child.kill()
            proc.kill()
            result['status'] = reason
            break
        time.sleep(0.5)
    result['exit_code'] = proc.wait()
result['elapsed_s'] = time.perf_counter() - started
if result['status'] == 'running':
    result['status'] = 'exited'
save()
print(json.dumps(result), flush=True)
raise SystemExit(0 if result['status'] == 'exited' and result['exit_code'] == 0 else 1)
