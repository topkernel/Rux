#!/usr/bin/env python3
"""pa41-tabulate.py — L0 profile battery tabulator.

Reads the three pa41 pbench serial logs (dbg / relstrong / relweak,
same source tree, back-to-back boots in one host-load window) plus the
pb1 anchor column, and prints the med comparison + profile deltas.
"""
import re, sys, os

WT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

def parse(path):
    rows = {}
    if not os.path.exists(path):
        return rows
    for line in open(path, errors="replace"):
        f = line.split()
        if len(f) >= 2 and f[0] == "PBENCH" and "med=" in line:
            kv = dict(t.split("=", 1) for t in f if "=" in t)
            if "med" in kv:
                try:
                    rows[f[1]] = float(kv["med"])
                except ValueError:
                    pass
    return rows

anchor = parse(os.path.join(WT, "run", "pb1-results.txt"))
dbg    = parse(os.path.join(WT, "work", "pbench-pa41-dbg-serial.log"))
strong = parse(os.path.join(WT, "work", "pbench-pa41-strong-serial.log"))
weak   = parse(os.path.join(WT, "work", "pbench-pa41-weak-serial.log"))

def pct(a, b):
    if not a or not b:
        return "      —"
    return "%+6.1f%%" % (100.0 * (a - b) / b)

names = sorted(set(dbg) | set(strong) | set(weak))
print("%-22s %10s %10s %10s | %8s %8s %8s | %s" %
      ("bench", "pb1", "dbg", "strong", "dbg/pb1", "str/pb1", "str/dbg", "weak"))
print("-" * 100)
for n in names:
    w = weak.get(n, 0.0)
    print("%-22s %10.3f %10.3f %10.3f | %8s %8s %8s | %10.3f" %
          (n, anchor.get(n, 0.0), dbg.get(n, 0.0), strong.get(n, 0.0),
           pct(dbg.get(n), anchor.get(n)), pct(strong.get(n), anchor.get(n)),
           pct(strong.get(n), dbg.get(n)), w))
if weak:
    print("-" * 100)
    for n in names:
        print("weak vs strong %-18s %s   weak vs dbg %s" %
              (n, pct(weak.get(n), strong.get(n)), pct(weak.get(n), dbg.get(n))))
