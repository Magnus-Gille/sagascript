#!/usr/bin/env python3
"""Transcribe a file with Sagascript, time it and score it against a reference.

Usage: run.py AUDIO REFERENCE.txt [--model pianissimo-sv] [--language sv] [--json]

Shows each step live (checking files, starting engine, transcribing with a progress
bar, computing WER) and ends with a summary table. With --json nothing but one JSON
object is printed on stdout (seconds = wall time of the whole transcribe command).
Stdlib only, no install needed. NO_COLOR=1 turns colours off.
"""
import argparse, json, os, shutil, subprocess, sys, threading, time, wave

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from wer import norm, wer

ap = argparse.ArgumentParser()
ap.add_argument("audio"); ap.add_argument("reference")
ap.add_argument("--model", default="pianissimo-sv"); ap.add_argument("--language", default="sv")
ap.add_argument("--bin", default="sagascript")
ap.add_argument("--json", action="store_true", help="machine output only: one JSON object on stdout")
a = ap.parse_args()

TTY = sys.stdout.isatty() and not a.json
COLOR = TTY and "NO_COLOR" not in os.environ


def c(code, s):
    return f"\033[{code}m{s}\033[0m" if COLOR else s


bold, dim, green, cyan, red, yellow = (lambda s, k=k: c(k, s) for k in ("1", "2", "32", "36", "31", "33"))


def say(s=""):
    if not a.json:
        print(s, flush=True)


def fail(msg):
    if a.json:
        sys.exit(msg)
    print(f"  {red('✗')} {msg}", flush=True)
    sys.exit(1)


def fmt_dur(s):
    s = int(round(s))
    return f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}" if s >= 3600 else f"{s // 60}:{s % 60:02d}"


def step(n, total, text):
    say(f"{cyan(f'[{n}/{total}]')} {bold(text)}")


def ok(text):
    say(f"      {green('✓')} {text}")


def bar(pct, width=34):
    fill = int(width * pct / 100)
    return "█" * fill + "░" * (width - fill)


def audio_length(path):
    try:
        with wave.open(path) as w:
            return w.getnframes() / w.getframerate()
    except Exception:
        pass
    if shutil.which("ffprobe"):
        r = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", path],
                           capture_output=True, text=True)
        try:
            return float(r.stdout.strip())
        except ValueError:
            pass
    return None


say()
say(f"  {bold('Sagascript')} {dim('· local Swedish transcription + word error rate')}")
say(f"  {dim('─' * 58)}")

# 1. Check files ------------------------------------------------------------
step(1, 4, "Checking files")
for p in (a.audio, a.reference):
    if not os.path.isfile(p):
        fail(f"not found: {p}")
binary = shutil.which(a.bin) or a.bin
ver = subprocess.run([binary, "--version"], capture_output=True, text=True) if shutil.which(a.bin) else None
if ver is None or ver.returncode:
    fail(f"cannot run '{a.bin}' (is Sagascript installed and on PATH?)")
version = ver.stdout.strip() or ver.stderr.strip()
audio_s = audio_length(a.audio)
reference = open(a.reference, encoding="utf-8").read()
ref_words = len(norm(reference))
if not ref_words:
    fail("reference text is empty")
ok(f"audio      {os.path.basename(a.audio)}  {dim('(' + (fmt_dur(audio_s) if audio_s else '?') + ')')}")
ok(f"reference  {os.path.basename(a.reference)}  {dim(f'({ref_words:,} words)')}")
ok(f"engine     {version}  {dim('· model ' + a.model + ' · language ' + a.language)}")

# 2+3. Start engine, transcribe ------------------------------------------------
step(2, 4, "Starting engine and loading model")
cmd = [binary, "transcribe", "--model", a.model, "--language", a.language, "--json", "--progress-json", a.audio]
t0 = time.monotonic()
proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
chunks = []
reader = threading.Thread(target=lambda: chunks.append(proc.stdout.read()), daemon=True)
reader.start()
errors, pct, last_print, started_tx = [], -1, -1, False
for line in proc.stderr:
    line = line.strip()
    ev = None
    if line.startswith("{"):
        try:
            ev = json.loads(line)
        except ValueError:
            pass
    if not ev or ev.get("event") != "transcription_progress":
        if line:
            errors.append(line)
        continue
    if not started_tx:
        started_tx = True
        ok(f"engine ready after {time.monotonic() - t0:.1f} s")
        step(3, 4, "Transcribing")
    p = ev.get("percent")
    if p is None:
        continue
    pct = max(pct, int(p))
    el = time.monotonic() - t0
    if TTY:
        sys.stdout.write(f"\r      {cyan(bar(pct))} {bold(f'{pct:3d}%')}  {dim(f'{el:5.1f} s')}  ")
        sys.stdout.flush()
    elif not a.json and pct // 10 != last_print:
        last_print = pct // 10
        say(f"      {pct:3d}%  {el:5.1f} s")
rc = proc.wait()
reader.join()
seconds = time.monotonic() - t0
if TTY and started_tx:
    sys.stdout.write("\n")
if rc:
    fail("sagascript failed: " + (errors[-1] if errors else f"exit code {rc}"))
try:
    res = json.loads(chunks[0])
except (IndexError, ValueError):
    fail("could not read sagascript's JSON output")
if isinstance(res, list):
    res = res[0].get("result", res[0])
text = res.get("text") or ""
if not text.strip():
    fail("no text in sagascript output")
ok(f"done in {seconds:.1f} s")
audio_s = audio_s or res.get("duration_seconds")
hyp = os.path.splitext(a.audio)[0] + ".hypothesis.txt"
open(hyp, "w", encoding="utf-8").write(text)

# 4. WER ---------------------------------------------------------------------
step(4, 4, "Computing word error rate")
box = []
th = threading.Thread(target=lambda: box.append(wer(reference, text)))
th.start()
frames, i, tw = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏", 0, time.monotonic()
while th.is_alive():
    if TTY:
        sys.stdout.write(f"\r      {cyan(frames[i % 10])} comparing {ref_words:,} words  {dim(f'{time.monotonic() - tw:.0f} s')}  ")
        sys.stdout.flush()
        i += 1
    th.join(0.1)
if TTY:
    sys.stdout.write("\r\033[K")
result = box[0]
ok(f"{result['errors']:,} word errors")

out = dict(result, seconds=round(seconds, 1), audio_seconds=round(audio_s, 1) if audio_s else None,
           realtime_factor=round(audio_s / seconds) if audio_s else None,
           model=a.model, language=a.language, engine=version, hypothesis=hyp)
if a.json:
    print(json.dumps(out))
    sys.exit(0)

rows = [
    ("Word error rate", f"{out['wer_percent']:.2f} %", True),
    ("Errors", f"{out['errors']:,} of {out['ref_words']:,} words", False),
    ("Words out / reference", f"{out['hyp_words']:,} / {out['ref_words']:,}", False),
    ("Time taken", f"{seconds:.1f} s", True),
    ("Audio length", f"{fmt_dur(audio_s)}  ({audio_s:,.0f} s)" if audio_s else "unknown", False),
    ("Speed", f"{out['realtime_factor']}× realtime" if audio_s else "unknown", True),
    ("Model", a.model, False),
    ("Language", a.language, False),
    ("Engine", version, False),
]
w1 = max(len(r[0]) for r in rows)
w2 = max(len(r[1]) for r in rows)
say()
say(f"  ╭─{'─' * w1}─┬─{'─' * w2}─╮")
say(f"  │ {bold('Result'.ljust(w1))} │ {' ' * w2} │")
say(f"  ├─{'─' * w1}─┼─{'─' * w2}─┤")
for k, v, hl in rows:
    cell = v.ljust(w2)
    say(f"  │ {k.ljust(w1)} │ {bold(green(cell)) if hl else cell} │")
say(f"  ╰─{'─' * w1}─┴─{'─' * w2}─╯")
say(f"  {dim('transcript: ' + hyp)}")
say()
