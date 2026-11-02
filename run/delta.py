#!/usr/bin/env python3
"""delta.py — compare two LTP scan results files (idx|test|class|rc|tail).

Groups differences into gains (now PASS) / losses (was PASS) / class moves,
and prints counts by class for the summary row.

Usage: python3 run/delta.py results/results-r20.txt results/results-r21.txt
"""
import sys
from collections import Counter


def load(path):
    out = {}
    for line in open(path):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("|", 4)
        if len(parts) < 3:
            continue
        out[parts[1]] = parts[2]
    return out


def main():
    a_path, b_path = sys.argv[1], sys.argv[2]
    A, B = load(a_path), load(b_path)
    print(f"[delta] {a_path}: {Counter(A.values())}")
    print(f"[delta] {b_path}: {Counter(B.values())}")
    gains = sorted(t for t in B if B[t] == "PASS" and A.get(t) != "PASS")
    losses = sorted(t for t in B if B[t] != "PASS" and A.get(t) == "PASS")
    moves = {}
    for t in B:
        if t in A and A[t] != B[t] and not (B[t] == "PASS" or A[t] == "PASS"):
            moves.setdefault(f"{A[t]}->{B[t]}", []).append(t)
    print(f"\n[delta] PASS gains ({len(gains)}):")
    for t in gains:
        print(f"  + {t} (was {A.get(t, 'MISSING')})")
    print(f"\n[delta] PASS losses ({len(losses)}):")
    for t in losses:
        print(f"  - {t} (now {B[t]})")
    print(f"\n[delta] non-PASS class moves:")
    for k in sorted(moves):
        print(f"  {k} ({len(moves[k])}): {', '.join(sorted(moves[k])[:40])}"
              f"{'...' if len(moves[k]) > 40 else ''}")
    pa = sum(1 for v in A.values() if v == "PASS")
    pb = sum(1 for v in B.values() if v == "PASS")
    print(f"\n[delta] PASS {pa} -> {pb} ({pb-pa:+d})")


if __name__ == "__main__":
    main()
