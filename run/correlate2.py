#!/usr/bin/env python3
"""Correlate corruption events with victim lifecycle + binder mappings. v2

Fixes vs v1: pid keys normalized to int; "last fork at/before event" so pid
recycling does not lie; USEGV vma(cr2)= classification (NONE vs mapped);
cr2 shape (page-aligned / +4 / +8 / tiny).
"""
import json, os, re
from collections import defaultdict

OUT = "/home/william/rux-agents/heap-forensics/run/timeline"
MUSL_BASE = 0x3fbf000000

def load(tag):
    with open(os.path.join(OUT, f"{tag}-events.json")) as f:
        return json.load(f)

def off(rip):
    try:
        v = int(rip, 16)
        if MUSL_BASE <= v < MUSL_BASE + 0x200000:
            return f"ld.so+0x{v - MUSL_BASE:x}"
        return rip
    except Exception:
        return rip

def cr2_shape(cr2):
    if not cr2:
        return ""
    v = int(cr2, 16)
    if v < 0x1000:
        return f"near-NULL(0x{v:x})"
    offin = v & 0xfff
    return f"page+0x{offin:x}"

# vma(cr2) NONE classification requires reading log lines after usegv
VMA_RE = re.compile(r"^USEGV-FOR: vma\((0x[0-9a-f]+)\) = (NONE|\S.*)$")

for tag in ["oh-b86", "oh-b87", "oh-b88", "oh-b89", "oh-b90"]:
    evs = load(tag)
    evs.sort(key=lambda e: e["t"])
    forks = defaultdict(list)   # pid -> [(t, parent, flags)]
    for e in evs:
        if e["type"] == "fork":
            forks[int(e["child"])].append((e["t"], int(e["parent"]), e["flags"]))
    proc_of = {}   # proc -> pid
    pid_map = {}   # pid -> (t, vaddr, size)
    for e in evs:
        if e["type"] == "bopen":
            proc_of[int(e["proc"])] = int(e["pid"])
        elif e["type"] == "bmap":
            p = proc_of.get(int(e["proc"]))
            if p:
                pid_map[p] = (e["t"], e["vaddr"], e["size"])
    comm_of = {}
    for e in evs:
        if e["type"] == "death":
            comm_of[int(e["pid"])] = e["comm"]

    # second pass over raw log for vma(cr2) lines
    logp = f"/home/william/rux-agents/oh-display/run/{tag}-serial.log"
    vma_none = {}  # t(usegv) -> "NONE"/"mapped"
    with open(logp, errors="replace") as f:
        t = 0
        pending = None
        for line in f:
            t += 1
            m = VMA_RE.match(line)
            if m and pending is not None and t - pending <= 6:
                addr, desc = m.group(1), m.group(2)
                vma_none.setdefault(pending, []).append((addr, desc.split()[0] if desc != "NONE" else "NONE"))
                pending = None  # only first vma line after
            elif "USEGV-FOR: pid=" in line:
                pending = t

    print(f"\n===== {tag} =====")
    for e in evs:
        if e["type"] not in ("usegv", "gp", "bc0", "pte_ow", "segv"):
            continue
        pid = int(e.get("pid", e.get("tid", 0)))
        fl = [f for f in forks.get(pid, []) if f[0] <= e["t"]]
        age = e["t"] - fl[-1][0] if fl else None
        bm = pid_map.get(pid)
        comm = comm_of.get(pid, "?")
        rip = off(e.get("rip", e.get("va", "")))
        cr2 = e.get("cr2", "")
        vn = vma_none.get(e["t"], [])
        vma_desc = ",".join(f"{a}:{d}" for a, d in vn) if vn else ""
        print(f"L{e['t']:>7} {e['type']:>7} pid={pid:<4} {comm:<20} site={rip:<14} "
              f"cr2={cr2:<14} {cr2_shape(cr2):<12} vma={vma_desc:<22} "
              f"age={age if age is not None else -1:<7} binder={'Y' if bm else 'N'}")
