#!/usr/bin/env python3
"""lmbench-median.py — aggregate N standing passes into ONE result file
consumable by make-report.py: per case, pick the MEDIAN measurement
line across passes (same extraction family as make-report; multi-float
lines like lat_ctx keep their whole line, median chosen on the parsed
value).
Usage: lmbench-median.py <out.txt> <pass1.txt> <pass2.txt> [...]
"""
import re
import sys
import statistics

BANNER = re.compile(r"POSIX Compatible|v0\.1\.0")
NUM = re.compile(r"[0-9]+\.[0-9]+")


def parse(path):
    out = {}
    case = None
    for raw in open(path, encoding="utf-8", errors="replace"):
        line = raw.rstrip("\r\n")
        if line.startswith("===CASE "):
            case = line[8:].strip()
            out[case] = []
            continue
        if case is None or not line or BANNER.search(line):
            continue
        if "arch: WARNING" in line or "KILLTRACE" in line or "DEQUEUE" in line:
            continue
        out[case].append(line)
    return out


def line_value(line):
    """Sort key for a measurement line: last float in it (0 if none)."""
    m = NUM.findall(line)
    return float(m[-1]) if m else 0.0


def main():
    out_path, passes = sys.argv[1], sys.argv[2:]
    data = [parse(p) for p in passes]
    cases = []
    for d in data:
        for c in d:
            if c not in cases:
                cases.append(c)
    with open(out_path, "w", encoding="utf-8") as f:
        for c in cases:
            f.write(f"===CASE {c}\n")
            # collect (value, line, pass_idx); a case may have several
            # measurement lines per pass (lat_ctx sizes, bw sizes) —
            # group by line position within the pass.
            groups = max(d.get(c, []) for d in data).__len__()
            rows = [d.get(c, []) for d in data]
            for i in range(max(len(r) for r in rows)):
                cands = [(line_value(r[i]), r[i]) for r in rows if i < len(r)]
                if not cands:
                    continue
                cands.sort(key=lambda t: t[0])
                mid = cands[len(cands) // 2][1] if len(cands) % 2 else \
                    cands[len(cands) // 2 - 1][1]
                f.write(mid + "\n")
    print(f"median file -> {out_path}")


if __name__ == "__main__":
    main()
