#!/usr/bin/env python3
"""Collect eval_der.py --json-out files into one compact grid for select_default.py.

  collect_grid.py OUT.json VARIANT=SET:FILE [VARIANT=SET:FILE ...]
"""
import json, sys
out = {}
for arg in sys.argv[2:]:
    variant, rest = arg.split("=", 1); sset, f = rest.split(":", 1)
    res = json.load(open(f))
    out.setdefault(variant, {})[sset] = {
        i: {t: [round(r["conf"] * 100, 2), round(r["der"] * 100, 2), r["hyp_speakers"], v["truth"]]
            for t, r in v.items() if t != "truth"} for i, v in res.items()}
json.dump(out, open(sys.argv[1], "w"), indent=1, sort_keys=True)
