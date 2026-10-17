#!/usr/bin/env python3
"""v3: tgid-aware binder correlation + context-window extraction.

Writes run/timeline/<tag>-ctx/<line>.txt with 20 lines before/after each
distinct corruption event (first occurrence of each cluster).
"""
import json, os, re
from collections import defaultdict

OUT = "/home/william/rux-agents/heap-forensics/run/timeline"
RUN = "/home/william/rux-agents/oh-display/run"
MUSL_BASE = 0x3fbf000000
CTX = 20

def load(tag):
    with open(os.path.join(OUT, f"{tag}-events.json")) as f:
        return json.load(f)

for tag in ["oh-b86", "oh-b87", "oh-b88", "oh-b89", "oh-b90"]:
    evs = load(tag)
    evs.sort(key=lambda e: e["t"])
    forks = defaultdict(list)
    for e in evs:
        if e["type"] == "fork":
            forks[int(e["child"])].append((e["t"], int(e["parent"]), e["flags"]))
    proc_of, pid_map = {}, {}
    for e in evs:
        if e["type"] == "bopen":
            proc_of[int(e["proc"])] = int(e["pid"])
        elif e["type"] == "bmap":
            p = proc_of.get(int(e["proc"]))
            if p:
                pid_map[p] = (e["t"], e["vaddr"], e["size"])
    tgid_of, comm_of = {}, {}
    for e in evs:
        if e["type"] == "death":
            tgid_of[int(e["pid"])] = int(e["tgid"])
            comm_of[int(e["pid"])] = e["comm"]
    # leader comm from its own name
    comm_of_tgid = {}
    for pid, t in tgid_of.items():
        if pid == t:
            comm_of_tgid[t] = comm_of.get(pid, "?")

    # which pids ever opened binder (any thread opening counts? only leader)
    binder_pids = set(pid_map.keys())

    print(f"\n===== {tag} (tgid-aware) =====")
    for e in evs:
        if e["type"] not in ("usegv", "gp", "bc0", "pte_ow", "segv"):
            continue
        pid = int(e.get("pid", e.get("tid", 0)))
        fl = [f for f in forks.get(pid, []) if f[0] <= e["t"]]
        age = e["t"] - fl[-1][0] if fl else -1
        tgid = tgid_of.get(pid, pid)
        ldr = comm_of_tgid.get(tgid, "?")
        bm = pid_map.get(tgid)
        print(f"L{e['t']:>7} {e['type']:>7} pid={pid:<4} tgid={tgid:<4} proc={ldr:<22} thr={comm_of.get(pid,'?'):<16} "
              f"age={age:<7} binder_tgid={'Y ' + bm[1] + ' sz=' + bm[2] if bm else 'N'}")
