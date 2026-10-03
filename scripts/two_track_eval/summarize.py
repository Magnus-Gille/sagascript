#!/usr/bin/env python3
"""Markdown table from run_eval.sh results: summarize.py RESULTS_DIR"""
import glob, json, sys
rows = []
for f in sorted(glob.glob(sys.argv[1] + "/*.json")):
    d = json.load(open(f)); label = d["label"]
    for k, name in (("two_track", "two-track"), ("downmix", "downmix")):
        if k in d:
            r = d[k]; w = d.get("me_words") if k == "two_track" else None
            rows.append(f"| {label} | {name} | {r['der']*100:.1f} | {r['conf']*100:.1f} | {r['miss']*100:.1f} | {r['fa']*100:.1f} | "
                        f"{r['me_recall']*100:.0f} | {r['false_me']*100:.0f} | "
                        + (f"{w['word_recall']*100:.0f} / {w['wer']*100:.0f} | {w['vad_coverage']*100:.0f} / {w['active_coverage_after_guard']*100:.0f} | {w['ref_words']} / {w['raw_words']} / {w['me_words']} |" if w else " | | |"))
print("| Case | Pipeline | DER % | Conf % | Miss % | FA % | Me speech recall % | False Me % | Me word recall / WER % | VAD / after-guard coverage % | words ref / raw / kept |")
print("|---|---|---|---|---|---|---|---|---|---|---|")
print("\n".join(rows))
