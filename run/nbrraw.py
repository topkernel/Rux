#!/usr/bin/env python3
"""nbrraw.py — offline analyzer for RWFRZ NBR-RAW stack-slot dumps.

The recursion holder's raw stack slots (values only) are matched against
candidate library files to (a) identify which libs map the fixed-base
shared region 0x3fbf000000-0x3fbf138000 (whose VMA offset bookkeeping is
zeroed by merged LOAD mappings) and (b) recover the recursive call chain.

Method:
  1. Parse NBR-RAW lines -> slot:value.
  2. Region values: those inside 0x3fbf000000..0x3fbf138000.
  3. For each lib X (PT_LOAD exec parsed), for each 4K-aligned mapping
     base B (in a window derived from the park rip 0x3fbf116dea landing
     inside X's exec LOAD), score:
       +3 for each region value that lands in X's exec LOAD at a file
          offset directly preceded by a 5-byte rel32 call (0xe8) whose
          displacement targets anywhere in X (plausible return addr),
       +1 for each region value merely landing in X's exec LOAD.
  4. Report the best (lib, base) and the resolved slots; repeated values
     (same value >= 2 slots) = the recursion cycle sites.

Usage: nbrraw.py <serial.log> [park_rip_hex]
"""
import glob
import struct
import sys
import re

PARK_RIP = 0x3FBF116DEA
REGION_LO = 0x3FBF000000
REGION_HI = 0x3FBF138000


def parse_libs():
    libs = []
    seen = set()
    pats = ["/tmp/oh-libs/syslib64/*.so", "/tmp/oh-libs/vendorlib64/*.so",
            "/tmp/oh-libs/cssp/*.so", "/tmp/oh-libs/subdirs/*",
            "/tmp/oh-libs/platformsdk/*.so", "/tmp/oh-libs/full-*/**/*.so"]
    for pat in pats:
        for f in glob.glob(pat, recursive=True):
            if f in seen:
                continue
            seen.add(f)
            try:
                d = open(f, "rb").read()
            except OSError:
                continue
            if d[:4] != b"\x7fELF" or d[4] != 2:
                continue
            e_phoff, = struct.unpack_from("<Q", d, 0x20)
            e_phentsize, e_phnum = struct.unpack_from("<HH", d, 0x36)
            loads = []
            for i in range(e_phnum):
                off = e_phoff + i * e_phentsize
                p_type, p_flags = struct.unpack_from("<II", d, off)
                p_offset, p_vaddr, _pa, p_filesz, _msz, _al = \
                    struct.unpack_from("<QQQQQQ", d, off + 8)
                if p_type == 1 and (p_flags & 1):  # PT_LOAD exec
                    loads.append((p_offset, p_filesz, p_vaddr))
            if loads:
                libs.append((f, d, loads))
    return libs


def va2fo(loads, va):
    for o, sz, v in loads:
        if v <= va < v + sz:
            return o + (va - v)
    return None


def main():
    log = sys.argv[1]
    park = int(sys.argv[2], 16) if len(sys.argv) > 2 else PARK_RIP
    slots = []
    for line in open(log, errors="replace"):
        m = re.match(r"RWFRZ: NBR-RAW( .*)?", line)
        if not m:
            continue
        for sm in re.finditer(r" (\d+):(\d+)", m.group(1) or ""):
            slots.append((int(sm.group(1)), int(sm.group(2))))
    if not slots:
        print("no NBR-RAW lines found")
        return
    print(f"{len(slots)} raw slots")
    region = [(i, v) for i, v in slots if REGION_LO <= v < REGION_HI]
    heap = [(i, v) for i, v in slots if 0x7FF000000000 <= v < 0x7FF100000000]
    print(f"region(0x3fbf...) values: {len(region)}, heap/other: {len(heap)}")
    rep = {}
    for i, v in slots:
        rep[v] = rep.get(v, 0) + 1
    print("repeated values (recursion cycle candidates):")
    for v, n in sorted(rep.items(), key=lambda x: -x[1]):
        if n >= 2:
            print(f"  {v:#x} x{n}")

    libs = parse_libs()
    print(f"{len(libs)} libs")
    best = None
    for f, d, loads in libs:
        # base window: park rip must land in an exec LOAD
        for o, sz, v in loads:
            # B + (va in [v, v+sz)) contains park -> B in (park - v - sz, park - v]
            blo = park - v - sz + 1
            bhi = park - v
            B = (blo + 0xFFF) & ~0xFFF
            while B <= bhi:
                score = 0
                hits = []
                for i, val in region:
                    va = val - B
                    fo = va2fo(loads, va)
                    if fo is None or fo < 5:
                        continue
                    score += 1
                    if d[fo - 5] == 0xE8:
                        # rel32 call target inside the lib (any exec load)
                        disp = struct.unpack_from("<i", d, fo - 4)[0]
                        tgt = (v + (fo - 5 - o) + 5 + disp) if False else None
                        # compute target vaddr: call site vaddr = va - 5
                        site_va = va - 5
                        tgt_va = site_va + 5 + disp
                        if any(vv <= tgt_va < vv + ss for _, ss, vv in loads):
                            score += 3
                            hits.append((i, val, "callret", hex(tgt_va)))
                if score >= 6 and (best is None or score > best[0]):
                    best = (score, f, B, hits)
                B += 0x1000
    if best:
        score, f, B, hits = best
        print(f"\nBEST region mapping: {f} @ base {B:#x} score {score}")
        for i, val, kind, extra in hits[:30]:
            print(f"  slot[{i}] {val:#x} {kind} {extra}")
    else:
        print("\nno consistent region mapping found")


if __name__ == "__main__":
    main()
