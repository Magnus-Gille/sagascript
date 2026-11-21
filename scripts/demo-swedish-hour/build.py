#!/usr/bin/env python3
"""Build a ~1 hour Swedish test file with an exact reference transcript.

Source: Google FLEURS sv_se test split (CC BY 4.0), read speech. Takes the first
reading of each distinct sentence, joins the clips with 0.5 s of silence into one
16 kHz mono PCM16 WAV and writes the matching reference text (stdlib only).

Usage: build.py [OUT_DIR] [--minutes 60]
"""
import argparse, csv, hashlib, io, json, os, sys, tarfile, urllib.request, wave

BASE = "https://huggingface.co/datasets/google/fleurs/resolve/main/data/sv_se/"
RATE = 16000
GAP = b"\x00\x00" * (RATE // 2)


def fetch(url, dest):
    if not os.path.exists(dest):
        print("downloading", url, file=sys.stderr)
        urllib.request.urlretrieve(url, dest + ".part")
        os.replace(dest + ".part", dest)
    return dest


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out", nargs="?", default="demo-audio")
    ap.add_argument("--minutes", type=float, default=60)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    tsv = fetch(BASE + "test.tsv", os.path.join(a.out, "fleurs-sv-test.tsv"))
    tgz = fetch(BASE + "audio/test.tar.gz", os.path.join(a.out, "fleurs-sv-test.tar.gz"))
    rows, seen = [], set()
    with open(tsv, encoding="utf-8") as f:
        for r in csv.reader(f, delimiter="\t"):
            fn, raw = r[1], r[2].strip()
            if raw and raw not in seen:
                seen.add(raw)
                rows.append((fn, raw))
    want = {fn: raw for fn, raw in rows}
    pcm = {}
    with tarfile.open(tgz) as t:
        for m in t:
            name = os.path.basename(m.name)
            if name in want:
                with wave.open(io.BytesIO(t.extractfile(m).read())) as w:
                    assert (w.getframerate(), w.getnchannels(), w.getsampwidth()) == (RATE, 1, 2), name
                    pcm[name] = w.readframes(w.getnframes())
    out, refs, total = [], [], 0.0
    limit = a.minutes * 60
    for fn, raw in rows:
        if fn not in pcm:
            continue
        dur = len(pcm[fn]) / 2 / RATE
        if total + dur > limit:
            break
        out.append(pcm[fn] + GAP)
        refs.append(raw)
        total += dur + 0.5
    wav = os.path.join(a.out, "swedish-fleurs-60min.wav")
    with wave.open(wav, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(RATE)
        w.writeframes(b"".join(out))
    ref = os.path.join(a.out, "swedish-fleurs-60min.reference.txt")
    with open(ref, "w", encoding="utf-8") as f:
        f.write("\n".join(refs) + "\n")
    sha = hashlib.sha256(open(wav, "rb").read()).hexdigest()
    man = dict(source="google/fleurs sv_se test (CC BY 4.0)", clips=len(refs),
               seconds=round(total, 1), words=sum(len(r.split()) for r in refs), sha256=sha)
    json.dump(man, open(os.path.join(a.out, "swedish-fleurs-60min.manifest.json"), "w"), indent=2)
    print(json.dumps(man))


main()
