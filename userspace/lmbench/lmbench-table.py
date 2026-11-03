#!/usr/bin/env python3
"""lmbench-table.py — standing refresh: median of N Rux passes + the
frozen reference column (run/lmbench-refkvm.txt), using make-report's
family-aware extraction. Rows where some passes starved use the median
of the passes that landed (a row only shows TIMEOUT if EVERY input
timed out). Usage:
  lmbench-table.py <refkvm.txt> <pass1.txt> <pass2.txt> [...] > table.md
"""
import importlib.util
import os
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location(
    "mr", os.path.join(HERE, "..", "userspace", "lmbench", "make-report.py"))
mr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mr)

REF = sys.argv[1]
PASSES = sys.argv[2:]

ref = mr.parse(REF)
rux = [mr.parse(p) for p in PASSES]

cases = []
for d in rux + [ref]:
    for c in d:
        if c not in cases:
            cases.append(c)


def rux_value(case):
    """Median of the per-pass values; None if every pass timed out."""
    vals = []
    for d in rux:
        line = mr.best(case, d.get(case, []))
        v = mr.value(case, line)
        if v is not None:
            vals.append(v)
    if not vals:
        return None, any(
            any("TIMEOUT" in l for l in d.get(case, [])) for d in rux)
    return statistics.median(vals), False


print("| Case | Rux (x86_64+KVM, med-of-%d) | Reference kernel | Ratio |" % len(PASSES))
print("|---|---|---|---|")
for c in cases:
    rv, timed_out = rux_value(c)
    rl = mr.best(c, (rux[0].get(c, []) or ref.get(c, [])))
    ll = mr.best(c, ref.get(c, []))
    lv = mr.value(c, ll)
    rcell = ("**TIMEOUT**" if timed_out else
             ("%s %s" % (mr.fmt(rv), mr.unit(c, rl))) if rv is not None else "n/a")
    lcell = ("%s %s" % (mr.fmt(lv), mr.unit(c, ll))) if lv is not None else "n/a"
    ratio = "-"
    if rv is not None and lv is not None and rv > 0 and lv > 0:
        if mr.is_throughput(c):
            ratio = "%.2fx" % (lv / rv) if lv / rv >= 1 else "**%.2fx (faster)**" % (rv / lv)
            if lv / rv >= 1:
                ratio += " slower" if lv / rv > 1.05 else " (at reference)"
        else:
            ratio = "%.2fx" % (rv / lv) if rv / lv >= 1 else "**%.2fx (faster)**" % (lv / rv)
    print("| `%s` | %s | %s | %s |" % (c, rcell, lcell, ratio))
