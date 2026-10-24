#!/usr/bin/env python3
"""pbmed.py — med-of-runs aggregation for PBENCH result files
(perf-report.md 8.5 table 8a rule: cell = median of the per-run medians;
p99 cell = median of the per-run p99s; also prints per-run spread so the
protocol's stability check (max/min med) is one glance).

Usage: pbmed.py <results.txt> [<results.txt> ...]
Output: one row per bench present in the FIRST file:
  <name>  med=<med-of-runs> p99=<med-of-runs> spread=<max/min meds> runs=n[,n,...]
"""
import sys

def med(xs):
    s = sorted(xs)
    n = len(s)
    return s[n // 2] if n % 2 else (s[n // 2 - 1] + s[n // 2]) / 2.0

def parse(path):
    out = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        f = line.split()
        if len(f) >= 2 and f[0] == "PBENCH" and f[1] not in ("START", "CAL", "PREP", "DONE"):
            kv = {t.split("=", 1)[0]: t.split("=", 1)[1] for t in f if "=" in t}
            if "med" in kv and "p99" in kv:
                out[f[1]] = (float(kv["med"]), float(kv["p99"]))
    return out

def main():
    files = sys.argv[1:]
    if not files:
        sys.exit(__doc__)
    tables = [parse(p) for p in files]
    base = tables[0]
    for name in base:
        meds, p99s = [], []
        for t in tables:
            if name in t:
                meds.append(t[name][0])
                p99s.append(t[name][1])
        if len(meds) < 2:
            print(f"{name:24s} med={meds[0]:12.3f} p99={p99s[0]:12.3f} single-run")
            continue
        spread = max(meds) / min(meds) if min(meds) > 0 else float("inf")
        print(f"{name:24s} med={med(meds):12.3f} p99={med(p99s):12.3f} "
              f"spread={spread:6.3f} runs={','.join(f'{m:g}' for m in meds)}")

if __name__ == "__main__":
    main()
