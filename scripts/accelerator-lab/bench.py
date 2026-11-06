"""Isolated native engine-host benchmark; never reads or writes app settings."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import statistics
import struct
import sys
import subprocess
import tarfile
import threading
import time
import wave

import numpy as np
import psutil


def prepare(lab, fixture):
    audio = lab / "audio"
    audio.mkdir(exist_ok=True)
    rows = list(csv.reader((lab / "fleurs-sv-dev.tsv").open(encoding="utf-8"), delimiter="\t"))
    chosen, ids, sexes = [], set(), {"MALE": 0, "FEMALE": 0}
    for r in rows:
        if r[0] not in ids and 6 <= int(r[5]) / 16000 <= 22 and sexes.get(r[6], 6) < 5:
            chosen.append(r)
            ids.add(r[0])
            sexes[r[6]] += 1
        if len(chosen) == 10:
            break
    wanted = {r[1] for r in chosen}
    with tarfile.open(lab / "fleurs-sv-dev.tar.gz") as tar:
        for member in tar:
            if Path(member.name).name in wanted:
                (audio / Path(member.name).name).write_bytes(tar.extractfile(member).read())
    corpus = [{"file": str(fixture), "reference": "Hongkongön ger Hongkongs territorium dess namn och är den plats som många turister betraktar som huvudfokus.", "id": "hongkong-test"}]
    corpus += [{"file": str(audio / r[1]), "reference": r[2], "id": r[1][:-4], "gender": r[6]} for r in chosen]
    for c in corpus:
        c["sha256"] = hashlib.sha256(Path(c["file"]).read_bytes()).hexdigest()
    (lab / "corpus.json").write_text(json.dumps(corpus, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"clips": len(corpus), "genders": sexes}))


def pcm(path):
    data = Path(path).read_bytes()
    assert data[:4] == b"RIFF" and data[8:12] == b"WAVE"
    pos, fmt, audio = 12, None, None
    while pos + 8 <= len(data):
        name, size = struct.unpack_from("<4sI", data, pos)
        chunk = data[pos + 8:pos + 8 + size]
        if name == b"fmt ":
            fmt = struct.unpack_from("<HHIIHH", chunk)
        elif name == b"data":
            audio = chunk
        pos += 8 + size + (size % 2)
    assert fmt is not None and audio is not None
    kind, channels, rate, _, _, bits = fmt
    assert (channels, rate) == (1, 16000), fmt
    if (kind, bits) == (1, 16):
        return np.frombuffer(audio, dtype="<i2").astype("<f4") / 32768
    if (kind, bits) == (3, 32):
        return np.frombuffer(audio, dtype="<f4").copy()
    raise ValueError(f"Unsupported WAV format: {fmt}")


def wer(ref, hyp):
    a, b = re.findall(r"\w+", ref.lower()), re.findall(r"\w+", hyp.lower())
    prev = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        curr = [i]
        for j, y in enumerate(b, 1):
            curr.append(min(prev[j] + 1, curr[-1] + 1, prev[j - 1] + (x != y)))
        prev = curr
    return {"errors": prev[-1], "words": len(a), "wer": prev[-1] / max(len(a), 1)}


class Host:
    def __init__(self, exe, env, stderr):
        self.err = open(stderr, "w", encoding="utf-8")
        self.p = subprocess.Popen([str(exe), "--protocol", "1"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err, text=True, encoding="utf-8", env={**os.environ, **env}, creationflags=subprocess.CREATE_NO_WINDOW)
        self.q = queue.Queue()
        def reader():
            for line in self.p.stdout:
                self.q.put(line)
            self.q.put(None)
        threading.Thread(target=reader, daemon=True).start()
        self.id = 0

    def send(self, op, **fields):
        self.id += 1
        begin = time.perf_counter()
        self.p.stdin.write(json.dumps({"v": 1, "id": self.id, "op": op, **fields}) + "\n")
        self.p.stdin.flush()
        while True:
            line = self.q.get(timeout=180)
            if line is None:
                raise RuntimeError("host exited; inspect stderr")
            r = json.loads(line)
            if r.get("id") == self.id and "ok" in r:
                if not r["ok"]:
                    raise RuntimeError(r)
                return r, (time.perf_counter() - begin) * 1000

    def close(self):
        if self.p.poll() is None:
            self.p.kill()
        self.p.wait()
        self.err.close()


def run(args):
    lab = Path(args.lab)
    result = {"label": args.label, "host": args.host, "runs": [], "env": dict(args.env), "idle_seconds": args.idle_seconds}
    target = lab / (args.label + ".json")
    def save():
        target.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    corpus = json.loads((lab / "corpus.json").read_text(encoding="utf-8"))[:args.limit]
    vocab = dict((int(i), p) for p, i in (line.rsplit(" ", 1) for line in (Path(args.model) / "vocab.txt").read_text(encoding="utf-8").splitlines()))
    host = Host(Path(args.host), dict(args.env), lab / (args.label + ".stderr.txt"))
    try:
        if args.qos:
            sys.path.insert(0, str(Path(__file__).resolve().parent))
            from windows_qos import process_qos
            result['process_qos'] = process_qos(host.p.pid, args.qos)
        result["hello"], _ = host.send("hello", client={"name": "accelerator-lab", "version": "1", "git_sha": "0" * 40})
        result["load"], result["load_wall_ms"] = host.send("load", model_dir=args.model, model_id="pianissimo-lab", compute_units="cpu")
        for c in corpus:
            samples = pcm(c["file"])
            audio_file = lab / "input.f32"
            samples.tofile(audio_file)
            for iteration in range(args.repeats):
                if args.idle_seconds:
                    time.sleep(args.idle_seconds)
                r, elapsed = host.send("transcribe_window", pcm_path=str(audio_file), offset_samples=0, num_samples=len(samples), sample_rate=16000, format="f32le", priority="interactive")
                tokens = r.get("tokens", [])
                text = "".join(vocab.get(t.get("id", t.get("token_id")), "") for t in tokens).replace("▁", " ").strip()
                row = {"id": c["id"], "iteration": iteration, "audio_s": len(samples)/16000, "wall_ms": elapsed, "timings": r.get("timings"), "text": text, "reference": c["reference"], "quality": wer(c["reference"], text), "rss_mb": psutil.Process(host.p.pid).memory_info().rss / 1e6, "tokens": tokens}
                if args.qos:
                    row['qos_after_request'] = process_qos(host.p.pid)
                result["runs"].append(row)
                save()
                print(json.dumps({k:v for k,v in row.items() if k not in ["tokens", "reference"]}, ensure_ascii=False), flush=True)
        host.send("unload")
        host.send("shutdown")
        host.p.wait(timeout=15)
    except Exception as e:
        result["error"] = repr(e)
        print("FAILED", repr(e), flush=True)
    finally:
        host.close()
        result['status'] = 'failed' if result.get('error') else 'complete'
        save()
    return 1 if result.get('error') else 0


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--lab", required=True)
    p.add_argument("--prepare", type=Path)
    p.add_argument("--host")
    p.add_argument("--model")
    p.add_argument("--label", default="cpu")
    p.add_argument("--qos", choices=['system', 'high', 'eco'])
    p.add_argument("--idle-seconds", type=float, default=0)
    p.add_argument("--limit", type=int, default=11)
    p.add_argument("--repeats", type=int, default=3)
    p.add_argument("--env", action="append", default=[], type=lambda x:x.split("=",1))
    args = p.parse_args()
    if args.prepare:
        prepare(Path(args.lab), args.prepare.resolve())
    else:
        sys.exit(run(args))
