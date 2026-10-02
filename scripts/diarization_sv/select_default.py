#!/usr/bin/env python3
"""Deterministic default-threshold selection and table generation for issue #284.

Input: a grid file written by `collect_grid.py` (per variant / set / recording / threshold:
[confusion, DER, speakers found, speakers in reference]). The selection set is the Swedish
clean recordings only (`riksdag` + `sv2`); `degraded` and `other` are reported, never selected on.

Objective: mean per-recording confusion. NEAR_TIE (percentage points, defined here) marks
thresholds whose training-set mean is within that of the minimum; the fold's choice is the
median of the tied thresholds. The full-set plateau gives the final default and its margins.

  select_default.py GRID.json [--variant NAME] > tables.md
  select_default.py GRID.json --table 0.36     # per-recording table (needs the `oldcode` variant)
  select_default.py GRID.json --at 0.36        # variant comparison at one threshold
"""
import argparse, json, statistics

NEAR_TIE = 0.25  # percentage points of mean confusion
SELECT_SETS = ("riksdag", "sv2")

def mean(xs):
    xs = list(xs)
    return sum(xs) / len(xs)

def pick(train, ths):
    """train: {rec: {th: conf}} -> (chosen, tied list, best mean)."""
    sc = {t: mean(v[t] for v in train.values()) for t in ths}
    best = min(sc.values())
    tied = [t for t in ths if sc[t] <= best + NEAR_TIE]
    return tied[len(tied) // 2], tied, best

def main():
    ap = argparse.ArgumentParser(); ap.add_argument("grid"); ap.add_argument("--variant", default="shipped")
    a = ap.parse_args()
    g = json.load(open(a.grid))[a.variant]
    ths = sorted(float(t) for t in next(iter(next(iter(g.values())).values())))
    recs = {}
    for s in SELECT_SETS:
        for i, v in g[s].items():
            recs[i] = {float(t): x[0] for t, x in v.items()}
    print(f"### Leave-one-recording-out over {len(recs)} Swedish recordings (variant `{a.variant}`, near-tie {NEAR_TIE} pp)\n")
    print("| Held-out | Tied range (training) | Chosen | Held-out confusion |\n| --- | --- | ---: | ---: |")
    held = []
    for r in recs:
        c, tied, _ = pick({k: v for k, v in recs.items() if k != r}, ths)
        held.append(recs[r][c]); print(f"| {r} | {tied[0]:.2f}-{tied[-1]:.2f} | {c:.2f} | {recs[r][c]:.1f} % |")
    print(f"\nMean held-out confusion {mean(held):.2f} %.\n")
    c, tied, best = pick(recs, ths)
    print("| Threshold | mean confusion (selection set) |\n| ---: | ---: |")
    for t in ths:
        print(f"| {t:.2f} | {mean(v[t] for v in recs.values()):.2f} % |")
    print(f"\nFull-set minimum {best:.2f} %; near-tie plateau {tied[0]:.2f}-{tied[-1]:.2f}; midpoint {c:.2f}; margins {c - tied[0]:.2f} below, {tied[-1] - c:.2f} above.")

def table(grid, new, old="0.75"):
    """Per-recording table: old code at 0.75 (variant `oldcode`), shipped code at 0.48 and at `new`."""
    g = json.load(open(grid)); cell = lambda r: f"{r[2]}/{r[3]}, {r[0]:.1f} / {r[1]:.1f} %"
    print(f"| Recording | main code, 0.75 | new code, 0.48 | new code, {new} (default) |\n| --- | --- | --- | --- |")
    for sset in ("riksdag", "sv2", "degraded", "other"):
        for i in g["shipped"][sset]:
            sh = g["shipped"][sset][i]
            print(f"| {i} | {cell(g['oldcode'][sset][i][old])} | {cell(sh['0.48'])} | {cell(sh[new])} |")

def variants(grid, th):
    """Mean confusion / DER and speaker-count agreement (found in truth..truth+1) per variant at `th`."""
    g = json.load(open(grid)); key = f"{th:.2f}".rstrip("0") if False else str(round(th, 2))
    print(f"### Variants at threshold {th:.2f}\n")
    print("| Variant | Swedish (14) conf / DER, count ok | Telephone-band (6) | French/Norwegian (8, non-gating) |\n| --- | --- | --- | --- |")
    for var, sets in g.items():
        if key not in next(iter(next(iter(sets.values())).values())):
            continue
        def cell(names):
            v = [x[key] for n in names for x in sets[n].values()]
            ok = sum(r[3] <= r[2] <= r[3] + 1 for r in v)
            return f"{mean(r[0] for r in v):.1f} / {mean(r[1] for r in v):.1f} %, {ok}/{len(v)}"
        print(f"| {var} | {cell(SELECT_SETS)} | {cell(('degraded',))} | {cell(('other',))} |")

if __name__ == "__main__":
    import sys
    if "--table" in sys.argv:
        i = sys.argv.index("--table"); table(sys.argv[1], sys.argv[i + 1])
    elif "--at" in sys.argv:
        i = sys.argv.index("--at"); variants(sys.argv[1], float(sys.argv[i + 1]))
    else:
        main()
