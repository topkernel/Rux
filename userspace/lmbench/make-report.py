#!/usr/bin/env python3
"""make-report.py — turn the two lmbench result files into the
comparison table in docs/test/lmbench-report.md.

Inputs (both produced by userspace/lmbench/run-{rux,linux}.sh):
  run/lmbench-rux.txt     Rux guest (x86_64 + KVM)
  run/lmbench-linux.txt   reference kernel, same binaries, same host CPU

Output: the markdown table body on stdout.

Extraction is per benchmark family — the raw line formats differ:
  micro() lines  "Label: X.XXXX microseconds"  -> float after the ':'
  lat_ctx        "<nprocs> <usecs>"            -> last float, select ^N float lines
  lat_mem_rd     "<range_MB> <ns/load>"        -> last float
  lat_mmap       "<size_MB> <usecs>"           -> last float
  bw_mem         "<size_MB> <MB/s>"            -> last float
  lat_fs         "0k\\t<n>\\t<create>\\t<del>"   -> 3rd integer field (create rate)
Hostnames inside labels (e.g. "127.0.0.1") match the generic float
regex, so values are always taken after the last ':' when one exists.
"""
import re
import sys

RUX = sys.argv[1] if len(sys.argv) > 1 else "run/lmbench-rux.txt"
LIN = sys.argv[2] if len(sys.argv) > 2 else "run/lmbench-linux.txt"

BANNER = re.compile(r"POSIX Compatible|v0\.1\.0")
# Kernel serial-log lines ("[    3.935471] reboot: Power down") carry a
# float timestamp and would otherwise displace the real measurement as
# the LAST line of a case block (seen: reference column turned n/a).
KLOG = re.compile(r"^\[\s*\d+\.\d+\]")
NUM = re.compile(r"[0-9]+\.[0-9]+")
CTX_LINE = re.compile(r"^\s*\d+\s+\d+\.\d+\s*$")
FS_LINE = re.compile(r"^\s*\d+k\s")


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
        if BANNER.search(line) or KLOG.match(line):
            continue
        if "LMBENCH TIMEOUT" in line or "LMBENCH EXEC-FAIL" in line:
            out[case].append(line.strip())
            continue
        if NUM.search(line) or FS_LINE.match(line):
            out[case].append(line.strip())
    return out


def best(case, lines):
    """The measurement line of a case, per family."""
    bench = case.split()[0]
    meas = [l for l in lines if "TIMEOUT" not in l and "EXEC-FAIL" not in l]
    if bench == "lat_ctx":
        picked = [l for l in meas if CTX_LINE.match(l)]
        return picked[-1] if picked else None
    if bench == "lat_fs":
        picked = [l for l in meas if FS_LINE.match(l)]
        return picked[-1] if picked else None
    return meas[-1] if meas else None


def value(case, line):
    if line is None:
        return None
    bench = case.split()[0]
    if bench == "lat_fs":  # "0k\t<n>\t<create>\t<delete>" -> create rate
        f = line.split()
        try:
            return float(f[2])
        except (IndexError, ValueError):
            return None
    part = line.rsplit(":", 1)[1] if ":" in line else line
    m = re.findall(r"[0-9]+\.[0-9]+", part)
    return float(m[-1]) if m else None


UNIT_BY_BENCH = {
    "lat_ctx": "us/ctxsw",
    "lat_mem_rd": "ns/load",
    "lat_fs": "files/s",
    "bw_mem": "MB/s",
    "bw_pipe": "MB/s",
    "bw_unix": "MB/s",
    "bw_tcp": "MB/s",
    "bw_udp": "MB/s",
    "bw_file_rd": "MB/s",
    "bw_mmap_rd": "MB/s",
}


def unit(case, line):
    bench = case.split()[0]
    if bench in UNIT_BY_BENCH:
        return UNIT_BY_BENCH[bench]
    m = re.search(r"(microseconds|usecs|MB/s|MB/sec|KB/s|KB/sec|bytes/sec)", line or "")
    return m.group(1) if m else "us"


def fmt(v):
    """Fixed-point, no scientific notation, trailing zeros trimmed."""
    s = "%.3f" % v
    s = s.rstrip("0").rstrip(".")
    return s or "0"


def is_throughput(case):
    return case.split()[0].startswith("bw_")


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
        rl = best(c, rux.get(c, []))
        ll = best(c, lin.get(c, []))
        rto = any("TIMEOUT" in l for l in rux.get(c, []))
        lto = any("TIMEOUT" in l for l in lin.get(c, []))
        rv = value(c, rl)
        lv = value(c, ll)
        if rto and rv is None:
            rcell = "**TIMEOUT**"
        elif rv is not None:
            rcell = "%s %s" % (fmt(rv), unit(c, rl))
        else:
            rcell = "n/a"
        if lto and lv is None:
            lcell = "**TIMEOUT**"
        elif lv is not None:
            lcell = "%s %s" % (fmt(lv), unit(c, ll))
        else:
            lcell = "n/a"
        ratio = "-"
        if rv is not None and lv is not None and rv > 0 and lv > 0:
            if is_throughput(c):
                # throughput: Rux/reference, >1 means Rux moves more
                # data per second (faster); printed inverted so that
                # "Nx" always reads "Rux is Nx slower"
                r = lv / rv
                ratio = "%.2fx slower" % r if r > 1.005 else (
                    "**%.2fx (faster)**" % (1 / r) if r < 0.995 else "1.00x")
            else:
                # latency: Rux/reference
                r = rv / lv
                ratio = "%.2fx" % r if r > 1.005 else (
                    "**%.2fx (faster)**" % r if r < 0.995 else "1.00x")
        print("| `%s` | %s | %s | %s |" % (c, rcell, lcell, ratio))


if __name__ == "__main__":
    main()
