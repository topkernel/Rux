#!/usr/bin/env python3
"""make-report.py — turn the two lmbench result files into the
comparison table in docs/development/lmbench-report.md.

Inputs (both produced by userspace/lmbench/run-{rux,linux}.sh):
  run/lmbench-rux.txt     Rux guest (x86_64 + KVM)
  run/lmbench-linux.txt   reference kernel, same binaries, same host CPU

Output: the markdown table body on stdout.
"""
import re
import sys

RUX = sys.argv[1] if len(sys.argv) > 1 else "run/lmbench-rux.txt"
LIN = sys.argv[2] if len(sys.argv) > 2 else "run/lmbench-linux.txt"

BANNER = re.compile(r"POSIX Compatible|v0\.1\.0")
NUM = re.compile(r"[0-9]+\.[0-9]+")


def parse(path):
    """case spec -> list of measurement lines (banner/version lines dropped)."""
    out = {}
    case = None
    for raw in open(path, encoding="utf-8", errors="replace"):
        line = raw.rstrip("\r\n")
        if line.startswith("===CASE "):
            case = line[8:].strip()
            out.setdefault(case, [])
            continue
        if case is None or not line.strip():
            continue
        if BANNER.search(line):
            continue
        if "LMBENCH TIMEOUT" in line or "LMBENCH EXEC-FAIL" in line:
            out[case].append(line.strip())
            continue
        if NUM.search(line):
            out[case].append(line.strip())
    return out


def best(case_lines):
    """The measurement line of a case (the last one wins — the runner
    prints one summary per case)."""
    meas = [l for l in case_lines if NUM.search(l) and "TIMEOUT" not in l]
    return meas[-1] if meas else None


def value(line):
    m = re.findall(r"[0-9]+\.[0-9]+", line)
    return float(m[0]) if m else None


def unit(line):
    m = re.search(r"(microseconds|usecs|MB/s|MB/sec|KB/s|KB/sec|bytes/sec)", line)
    return m.group(1) if m else ""


def main():
    rux = parse(RUX)
    lin = parse(LIN)
    cases = list(rux.keys())
    for c in lin.keys():
        if c not in cases:
            cases.append(c)

    print("| Case | Rux (x86_64+KVM) | Reference kernel | Ratio |")
    print("|---|---|---|---|")
    for c in cases:
        rl = best(rux.get(c, []))
        ll = best(lin.get(c, []))
        rto = any("TIMEOUT" in l for l in rux.get(c, []))
        lto = any("TIMEOUT" in l for l in lin.get(c, []))
        rv = value(rl) if rl else None
        lv = value(ll) if ll else None
        if rto and rv is None:
            rcell = "**TIMEOUT**"
        elif rl:
            rcell = "%s %s" % (rv, unit(rl))
        else:
            rcell = "n/a"
        if lto and lv is None:
            lcell = "**TIMEOUT**"
        elif ll:
            lcell = "%s %s" % (lv, unit(ll))
        else:
            lcell = "n/a"
        if rv is not None and lv is not None and lv > 0:
            ratio = "%.2fx" % (rv / lv)
            if rv < lv:
                ratio = "**%s (faster)**" % ratio
        else:
            ratio = "-"
        print("| `%s` | %s | %s | %s |" % (c, rcell, lcell, ratio))


if __name__ == "__main__":
    main()
