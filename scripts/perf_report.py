#!/usr/bin/env python3
# MIT License
# Copyright (c) 2026 Fei Wang

"""perf_report.py — per-function sample-count report from a Rux kernel
tick-sampling dump (test/perf_sample.c + the in-tree perf_event_open
sampler in kernel/core/src/perf/mod.rs).

Reads the dump file (reference perf.data container, "PERFILE2") plus the
booted kernel's own symbol table, and prints every kernel function with
its sample counts in two accounting modes:

  self     the tick's interrupted PC (sample IP) landed in the function
  total    self + every callchain frame that resolves to the function
           (deduplicated per sample, the reference "children" semantic)

Rows are sorted by self descending (tie-break total), or by total with
--sort total. Unknown PCs (no symbol) aggregate under "?".

Symbol-table sources (highest priority wins when several are given, and
the table must come from the kernel that was booted — the generated
table is embedded verbatim into the image at build time):

  --ksyms-dir DIR   build tree kernel/core/src/debug/: ksyms_generated.rs
                    (INDEX rows: (name_off, addr, size, type)) +
                    ksyms.bin (NUL-separated demangled names). This is
                    the exact table the booted kernel carries.
  --elf PATH        the linked kernel ELF, via nm -nS --defined-only
                    (mangled names; kept raw).
  --kallsyms PATH   a /proc/kallsyms-style text file
                    ("<addr> <type> <name>" per line).

Usage:
  perf_report.py DUMP --ksyms-dir kernel/core/src/debug [--top 20]
                  [--sort self|total] [--json] [--all]
  perf_report.py DUMP --elf target/x86_64-unknown-none/debug/rux ...
  perf_report.py DUMP --kallsyms kallsyms.txt ...

rc 0 on success, 1 on usage/parse failure.
"""
import argparse
import json
import os
import re
import subprocess
import sys

PERF_RECORD_SAMPLE = 9
PERF_RECORD_LOST = 2
# uapi context markers live in the top 256-byte band of the address
# space (KERNEL = (u64)-128, USER = (u64)-256, HARDWARE = (u64)-384):
# everything at or above (u64)-256 is a marker, NOT a frame. The
# threshold must stay above kernel text — x86_64 text lives in the
# high 4GiB window (0xFFFFFFFF8...) and riscv64 at 0xFFFFFFC0....
PERF_CONTEXT_KERNEL = 0xFFFFFFFFFFFFFF80
PERF_CONTEXT_USER = 0xFFFFFFFFFFFFFF00
CONTEXT_MARKER_FLOOR = 0xFFFFFFFFFFFFFF00

TEXT_TYPES = set("TtWw")


# ---------------------------------------------------------------- symbols
class Symtab:
    """Sorted (addr, size, name) text symbols with a binary search."""

    def __init__(self, syms):
        # (addr, size, name); keep the FIRST name per address (nm can
        # emit aliases) and drop non-text entries downstream callers
        # already filtered.
        merged = {}
        for addr, size, name in syms:
            if addr in merged and merged[addr][1]:
                continue
            merged[addr] = (addr, size, name)
        self.syms = sorted(merged.values(), key=lambda s: s[0])
        self.addrs = [s[0] for s in self.syms]
        # Kernel text range (for filtering callchain frames before
        # lookup — user/abs garbage addresses resolve nowhere anyway).
        self.lo = self.syms[0][0] if self.syms else 1
        self.hi = max(a for a, _s, _n in self.syms) if self.syms else 0

    def lookup(self, pc):
        """(name, addr, size) of the symbol containing pc, else None.

        Size-aware: a pc past a symbol's end is an inter-symbol gap
        (padding or unsymbolized code — entry stubs), NOT that symbol.
        Zero-size symbols occupy zero bytes: they never claim a pc other
        than their own address (the classic size-0 kallsyms snap would
        dump every unsymbolized frame into whatever tiny alias precedes
        it — observed poisoning inclusive counts with Debug::fmt)."""
        if pc < self.lo or pc > self.hi + (1 << 32):
            return None
        lo, hi = 0, len(self.syms)
        while lo < hi:
            mid = (lo + hi) // 2
            if self.addrs[mid] <= pc:
                lo = mid + 1
            else:
                hi = mid
        i = lo - 1
        while i >= 0:
            addr, size, name = self.syms[i]
            if size == 0:
                if pc == addr:
                    return (name, addr, size)
                i -= 1  # zero-size alias: walk back
                continue
            if pc >= addr + size:
                return None  # gap — unsymbolized code
            return (name, addr, size)
        return None


def load_ksyms_dir(d):
    """Parse ksyms_generated.rs + ksyms.bin (the build's embedded table
    — identical to what the booted kernel carries)."""
    rs = os.path.join(d, "ksyms_generated.rs")
    blob_path = os.path.join(d, "ksyms.bin")
    for p in (rs, blob_path):
        if not os.path.exists(p):
            raise SystemExit("perf_report: missing %s" % p)
    blob = open(blob_path, "rb").read()
    syms = []
    row = re.compile(r"\((\d+),\s*(0x[0-9a-fA-F]+),\s*(0x[0-9a-fA-F]+),\s*b'(.?)'\)")
    for line in open(rs, errors="replace"):
        m = row.search(line)
        if not m:
            continue
        off = int(m.group(1))
        addr = int(m.group(2), 16)
        size = int(m.group(3), 16)
        typ = m.group(4).encode("latin1").decode("unicode_escape")
        if typ not in TEXT_TYPES:
            continue
        start = off
        end = blob.index(b"\0", start)
        name = blob[start:end].decode("utf-8", "replace")
        syms.append((addr, size, name))
    if not syms:
        raise SystemExit("perf_report: no INDEX rows parsed from %s" % rs)
    return syms


def load_elf(elf):
    """nm -nS --defined-only over the linked kernel ELF (mangled names)."""
    for prefix in ("x86_64-linux-gnu-", "riscv64-linux-gnu-", ""):
        try:
            out = subprocess.run(
                [prefix + "nm", "-n", "--defined-only", "-S", elf],
                check=True, capture_output=True, text=True).stdout
            break
        except (OSError, subprocess.CalledProcessError):
            continue
    else:
        raise SystemExit("perf_report: nm failed on %s" % elf)
    syms = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 4 and len(parts[1]) == 16:
            addr_s, size_s, typ, name = parts[0], parts[1], parts[2], parts[3]
        elif len(parts) >= 3:
            addr_s, size_s, typ, name = parts[0], "0", parts[1], parts[2]
        else:
            continue
        if typ not in TEXT_TYPES:
            continue
        try:
            syms.append((int(addr_s, 16), int(size_s, 16), name))
        except ValueError:
            continue
    if not syms:
        raise SystemExit("perf_report: no text symbols in %s" % elf)
    return syms


def load_kallsyms(path):
    syms = []
    row = re.compile(r"^([0-9a-fA-F]+)\s+(\S)\s+(\S+)")
    for line in open(path, errors="replace"):
        m = row.match(line)
        if not m:
            continue
        if m.group(2) not in TEXT_TYPES:
            continue
        syms.append((int(m.group(1), 16), 0, m.group(3)))
    if not syms:
        raise SystemExit("perf_report: no symbols parsed from %s" % path)
    syms.sort(key=lambda s: s[0])
    return syms


# ---------------------------------------------------------------- dump
def parse_dump(path):
    """Yield (ip, callchain_list, misc) per SAMPLE record; also return
    the attr sample_type and LOST totals.

    Accepts the "PERFILE2" container (skips header + attr sections via
    the header's data offset/size) or a raw stream of ring records.
    A truncated tail (poweroff mid-write) parses up to the last whole
    record.
    """
    with open(path, "rb") as fh:
        data = fh.read()
    if len(data) < 8:
        raise SystemExit("perf_report: %s too small" % path)
    sample_type = None
    lost = 0
    bad = 0
    if data[0:8] == bytes.fromhex("50455246494c4532"):  # "PERFILE2" LE
        if len(data) < 104:
            raise SystemExit("perf_report: header truncated")
        attr_size = int.from_bytes(data[16:24], "little")
        data_off = int.from_bytes(data[48:56], "little")
        data_size = int.from_bytes(data[40:48], "little")
        sample_type = int.from_bytes(data[96:104], "little")
        # sample_type lives at header offset 96 (after ids words); the
        # writer also mirrors the attr — prefer the attr section.
        a_off = int.from_bytes(data[32:40], "little")  # attrs_offset
        if a_off and len(data) >= a_off + 32:
            sample_type = int.from_bytes(data[a_off + 24:a_off + 32],
                                         "little")
        end = min(len(data), data_off + data_size)
        stream = data[data_off:end]
    else:
        stream = data  # raw drained-record stream
    samples = []
    off = 0
    n = len(stream)
    while off + 8 <= n:
        rtype = int.from_bytes(stream[off:off + 4], "little")
        misc = int.from_bytes(stream[off + 4:off + 6], "little")
        size = int.from_bytes(stream[off + 6:off + 8], "little")
        if size < 8 or off + size > n:
            bad += 1
            break  # truncated/corrupt tail
        if rtype == PERF_RECORD_LOST:
            if size >= 24:
                lost += int.from_bytes(stream[off + 16:off + 24], "little")
        elif rtype == PERF_RECORD_SAMPLE:
            ip, chain = decode_sample(stream[off + 8:off + size], misc,
                                      sample_type)
            samples.append((ip, chain, misc))
        # other record types (FORK/EXIT/COMM/...) — skipped
        off += size
    return samples, lost, bad, sample_type


def decode_sample(body, misc, sample_type):
    """Decode a PERF_RECORD_SAMPLE body per the ABI field order
    (IDENTIFIER, IP, TID, TIME, ID, STREAM_ID, CPU, PERIOD, CALLCHAIN).
    Falls back to assuming IP|CALLCHAIN|TID|TIME|ID|CPU|PERIOD (the
    recorder's layout) when the container carried no attr."""
    if sample_type is None:
        sample_type = (0x1 | 0x2 | 0x4 | 0x40 | 0x80 | 0x100 | 0x20)
    ip = 0
    chain = []
    o = 0
    n = len(body)
    for bit in (0x10000, 0x1, 0x2, 0x4, 0x40, 0x200, 0x80, 0x100):
        if not (sample_type & bit):
            continue
        if o + 8 > n:
            return ip, chain
        val = int.from_bytes(body[o:o + 8], "little")
        o += 8
        if bit == 0x1:  # IP
            ip = val
    if sample_type & 0x20:  # CALLCHAIN
        if o + 8 > n:
            return ip, chain
        nr = int.from_bytes(body[o:o + 8], "little")
        o += 8
        for _ in range(min(nr, (n - o) // 8)):
            chain.append(int.from_bytes(body[o:o + 8], "little"))
            o += 8
    return ip, chain


# ---------------------------------------------------------------- report
def main():
    ap = argparse.ArgumentParser(
        description="Per-function sample counts from a Rux tick-sampler "
                    "dump (see module docstring).")
    ap.add_argument("dump", help="perf.data dump (container or raw)")
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--ksyms-dir", metavar="DIR",
                     help="kernel/core/src/debug with ksyms_generated.rs"
                          " + ksyms.bin (the booted kernel's own table)")
    src.add_argument("--elf", metavar="PATH",
                     help="kernel ELF; symbols via nm -nS")
    src.add_argument("--kallsyms", metavar="PATH",
                     help="/proc/kallsyms-style text symbol table")
    ap.add_argument("--top", type=int, default=20, metavar="N",
                    help="rows to print (default 20)")
    ap.add_argument("--sort", choices=["self", "total"], default="self",
                    help="sort key (default self)")
    ap.add_argument("--json", action="store_true",
                    help="machine-readable output instead of the table")
    ap.add_argument("--all", action="store_true",
                    help="print every function (overrides --top)")
    args = ap.parse_args()

    if args.ksyms_dir:
        syms = load_ksyms_dir(args.ksyms_dir)
    elif args.elf:
        syms = load_elf(args.elf)
    else:
        syms = load_kallsyms(args.kallsyms)
    tab = Symtab(syms)

    samples, lost, bad, sample_type = parse_dump(args.dump)
    if bad:
        print("perf_report: WARNING %d trailing garbage byte(s) skipped"
              % bad, file=sys.stderr)

    self_ct = {}
    total_ct = {}
    unknown_ip = 0
    for ip, chain, misc in samples:
        hit = tab.lookup(ip)
        leaf = hit[0] if hit else None
        if leaf is None:
            unknown_ip += 1
        if leaf:
            self_ct[leaf] = self_ct.get(leaf, 0) + 1
            total_ct[leaf] = total_ct.get(leaf, 0) + 1
        # Inclusive: every DISTINCT kernel function on the chain gets one
        # sample (dedup per sample — the reference children semantics).
        seen = {leaf} if leaf else set()
        for fr in chain:
            if fr >= CONTEXT_MARKER_FLOOR:
                continue  # context markers / non-canonical
            if fr < tab.lo or fr > tab.hi:
                continue
            hit2 = tab.lookup(fr)
            if not hit2:
                continue
            name = hit2[0]
            if name in seen:
                continue
            seen.add(name)
            total_ct[name] = total_ct.get(name, 0) + 1

    rows = []
    names = set(self_ct) | set(total_ct)
    for name in names:
        rows.append((self_ct.get(name, 0), total_ct.get(name, 0), name))
    rows.sort(key=lambda r: (-r[0 if args.sort == "self" else 1],
                             -r[1 if args.sort == "self" else 0], r[2]))

    total_samples = len(samples)
    if args.json:
        print(json.dumps({
            "dump": os.path.abspath(args.dump),
            "symbols": len(tab.syms),
            "samples": total_samples,
            "lost": lost,
            "truncated_records": bad,
            "unknown_ip": unknown_ip,
            "sort": args.sort,
            "functions": [
                {"name": n, "self": s, "total": t} for s, t, n in rows
            ],
        }, indent=1))
        return 0

    print("# Rux tick-sampler report: %s" % args.dump)
    print("# symbols=%d samples=%d lost=%d truncated=%d unknown_ip=%d "
          "functions=%d sort=%s" %
          (len(tab.syms), total_samples, lost, bad, unknown_ip,
           len(rows), args.sort))
    print("# %-9s %-9s  %s" % ("self", "total", "function"))
    shown = rows if args.all else rows[:max(args.top, 0)]
    for s, t, name in shown:
        print("%9d %9d  %s" % (s, t, name))
    return 0


if __name__ == "__main__":
    sys.exit(main())
